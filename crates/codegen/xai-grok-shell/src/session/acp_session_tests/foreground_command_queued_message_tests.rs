//! A user message queued while a foreground shell command runs moves the command to the background and ends the call.

use super::support::*;
use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn command_call(id: &str, is_background: bool) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            "run_terminal_cmd",
            serde_json::json!({
                "command": "sleep 30",
                "description": "wait a long time",
                "is_background": is_background,
            })
            .to_string(),
        ),
        vendor: Default::default(),
    }
}

/// Runs one `sleep 30` command call and queues a message before or while it runs.
/// Returns how long the call took and the tool result the model sees.
async fn run_command_call_with_queued_message(queue_before: bool) -> (Duration, String) {
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    *actor.agent.borrow_mut() = test_agent_with_tools(vec![
        xai_grok_tools::registry::types::ToolConfig::for_tool::<
            xai_grok_tools::implementations::grok_build::BashTool,
        >(),
        xai_grok_tools::registry::types::ToolConfig::for_tool::<
            xai_grok_tools::implementations::grok_build::task_output::TaskOutputTool,
        >(),
        xai_grok_tools::registry::types::ToolConfig::for_tool::<
            xai_grok_tools::implementations::grok_build::kill_task::KillTaskTool,
        >(),
    ])
    .await;
    actor
        .workspace_ops
        .bind_local_session(
            &actor.session_id_string(),
            actor.tool_context.cwd.as_path().to_path_buf(),
            actor.tool_context.hunk_tracker_handle.clone(),
            actor.agent.borrow().tool_bridge().toolset(),
            None,
        )
        .expect("bind_local_session");

    *actor
        .current_prompt_id
        .lock()
        .expect("current_prompt_id mutex poisoned") = Some("running".into());
    {
        let mut state = actor.state.lock().await;
        state.pending_inputs.push_back(user_item("running", "A"));
        state.running_task = Some(running_task_stub("running"));
        state.front_message_committed = true;
    }
    let actor = Arc::new(actor);

    let (respond_to, _prx) = tokio::sync::oneshot::channel();
    let mut request = queue_input_request(
        vec![acp::ContentBlock::Text(acp::TextContent::new("hello"))],
        "queued",
        respond_to,
    );
    // The harvest only takes rows owned by the running turn's client, which `user_item` sets to "A".
    request.client_identifier = Some("A".to_string());
    let mut queue_request = Some(request);
    if queue_before && let Some(request) = queue_request.take() {
        let _ = actor.queue_input(request).await;
    }

    let started = Instant::now();
    let call_actor = actor.clone();
    let call = tokio::task::spawn_local(async move {
        call_actor
            .execute_tool_calls(vec![command_call("cmd-1", false)], None)
            .await
    });
    // The command is in flight once the call raises the blocking wait.
    for _ in 0..40 {
        if actor.tool_context.blocking_wait_depth.depth() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if let Some(request) = queue_request.take() {
        let _ = actor.queue_input(request).await;
    }

    let _ = tokio::time::timeout(Duration::from_secs(15), call)
        .await
        .expect("the command call must return once a message is queued")
        .expect("call task");
    let elapsed = started.elapsed();
    let snapshot = actor
        .chat_state_handle
        .snapshot()
        .await
        .expect("chat state alive");
    let result_text = snapshot
        .conversation
        .iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult(tr) if tr.tool_call_id == "cmd-1" => {
                Some(tr.content.to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    actor
        .agent
        .borrow()
        .tool_bridge()
        .kill_all_background_tasks()
        .await;
    (elapsed, result_text)
}

/// Every follow-up mode and queue timing: the row arrives during the command, or sat queued before it began.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(follow_up_steer_cache)]
async fn queued_message_backgrounds_a_foreground_command() {
    let local = tokio::task::LocalSet::new();
    local
		.run_until(async {
			for steer in [true, false] {
				for queue_before in [false, true] {
					crate::util::config::set_follow_up_steer_cache(steer);
					let (elapsed, result) = run_command_call_with_queued_message(queue_before).await;
					let case = format!("steer={steer} queue_before={queue_before}");
					assert!(
						elapsed < Duration::from_secs(15),
						"the call must not wait for `sleep 30` to exit ({case}): took {elapsed:?}"
					);
					assert!(
						result.contains("moved to background"),
						"the model must be told the command still runs in the background ({case}): {result:?}"
					);
				}
			}
		})
		.await;
}

#[test]
fn foreground_command_predicate_reads_the_flag_leniently() {
    use super::tool_calls::is_foreground_command_call;
    use xai_grok_tools::types::tool::ToolKind;
    let call =
        |kind: Option<ToolKind>, args: serde_json::Value| is_foreground_command_call(kind, &args);
    assert!(call(
        Some(ToolKind::Execute),
        serde_json::json!({"command": "ls"})
    ));
    assert!(call(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": false})
    ));
    assert!(!call(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": true})
    ));
    assert!(!call(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": "true"})
    ));
    assert!(!call(
        Some(ToolKind::Execute),
        serde_json::json!({"background": true})
    ));
    assert!(!call(Some(ToolKind::Read), serde_json::json!({})));
    assert!(!call(None, serde_json::json!({})));
}
