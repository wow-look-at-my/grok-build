//! A user message queued while a foreground `task` call runs ends that call and backgrounds its subagent.

use super::support::*;
use super::*;
use std::sync::Arc;
use std::time::Duration;
use xai_grok_tools::implementations::grok_build::task::backend::{
    SubagentBackend, SubagentBackendResource,
};
use xai_grok_tools::implementations::grok_build::task::types::{
    ActiveAgentMessageOutcome, ActiveAgentMessageRequest, SubagentCancelOutcome,
    SubagentDepthCounter, SubagentDescribeOutcome, SubagentEvent, SubagentHandOffForegroundRequest,
    SubagentRequest, SubagentResult, SubagentSnapshot, SubagentValidateTypeOutcome,
};

/// A subagent backend whose spawn never finishes: the foreground call stays blocked.
struct NeverFinishesBackend;

#[async_trait::async_trait]
impl SubagentBackend for NeverFinishesBackend {
    async fn spawn(
        &self,
        _: SubagentRequest,
        _: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<SubagentResult, xai_tool_runtime::ToolError> {
        std::future::pending().await
    }

    async fn query(&self, _: &str, _: bool, _: Option<u64>) -> Option<SubagentSnapshot> {
        None
    }

    async fn send_active_message(&self, _: ActiveAgentMessageRequest) -> ActiveAgentMessageOutcome {
        ActiveAgentMessageOutcome::Unsupported
    }

    async fn cancel(&self, _: &str) -> SubagentCancelOutcome {
        SubagentCancelOutcome::NotFound
    }

    async fn validate_type(&self, _: &str, _: &str) -> SubagentValidateTypeOutcome {
        SubagentValidateTypeOutcome::Ok
    }

    async fn describe_subagent_type(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
    ) -> SubagentDescribeOutcome {
        SubagentDescribeOutcome::Unavailable
    }
}

fn task_call(id: &str, run_in_background: bool) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            "task",
            serde_json::json!({
                "prompt": "scan the repository",
                "description": "scan src",
                "run_in_background": run_in_background,
            })
            .to_string(),
        ),
        vendor: Default::default(),
    }
}

/// Runs one `task` call on a backend that never finishes. A message is queued while the call runs.
/// Returns the hand-off requests the coordinator saw and whether the call returned.
async fn run_task_call_with_queued_message(
    run_in_background: bool,
    queue_before: bool,
) -> (Vec<String>, bool, String) {
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    *actor.agent.borrow_mut() = test_agent_with_tools(vec![
        xai_grok_tools::registry::types::ToolConfig::for_tool::<
            xai_grok_tools::implementations::grok_build::task::TaskTool,
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
    {
        let bridge = actor.agent.borrow().tool_bridge().clone();
        bridge.update_resource(SubagentDepthCounter(0)).await;
        bridge
            .update_resource(SubagentBackendResource(Arc::new(NeverFinishesBackend)))
            .await;
        bridge
            .update_resource(crate::tools::tool_context::subagent_foreground_wait(
                actor.tool_context.blocking_wait_depth.clone(),
            ))
            .await;
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::task::spawn_local(async move {
        while let Some(event) = event_rx.recv().await {
            if let SubagentEvent::HandOffForeground(SubagentHandOffForegroundRequest {
                prompt_id,
                respond_to,
                ..
            }) = event
            {
                let _ = seen_tx.send(prompt_id);
                let _ = respond_to.send(Vec::new());
            }
        }
    });
    actor.tool_context.subagent_event_tx = Some(event_tx);

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
    // Promotion only takes rows owned by the running turn's client, which `user_item` sets to "A".
    request.client_identifier = Some("A".to_string());
    let mut queue_request = Some(request);
    if queue_before && let Some(request) = queue_request.take() {
        let _ = actor.queue_input(request).await;
    }

    let call_actor = actor.clone();
    let call = tokio::task::spawn_local(async move {
        call_actor
            .execute_tool_calls(vec![task_call("task-1", run_in_background)], None)
            .await
    });
    // The call is in flight once the foreground wait is raised; a background call never raises it.
    for _ in 0..40 {
        if actor.tool_context.blocking_wait_depth.depth() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    if let Some(request) = queue_request.take() {
        let _ = actor.queue_input(request).await;
    }

    let returned = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .is_ok();
    let mut seen = Vec::new();
    while let Ok(prompt_id) = seen_rx.try_recv() {
        seen.push(prompt_id);
    }
    let snapshot = actor
        .chat_state_handle
        .snapshot()
        .await
        .expect("chat state alive");
    let result_text = snapshot
        .conversation
        .iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult(tr) if tr.tool_call_id == "task-1" => {
                Some(tr.content.to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (seen, returned, result_text)
}

/// Every follow-up mode and queue timing: the row arrives during the call, or sat queued before it began.
#[tokio::test(flavor = "current_thread")]
async fn queued_message_backgrounds_a_foreground_task_call() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            for steer in [true, false] {
                for queue_before in [false, true] {
                    crate::util::config::set_follow_up_steer_cache(steer);
                    let (handed_off, returned, result) =
                        run_task_call_with_queued_message(false, queue_before).await;
                    let case = format!("steer={steer} queue_before={queue_before}");
                    assert!(
                        returned,
                        "the foreground task call must return once a message is queued ({case})"
                    );
                    assert_eq!(
                        handed_off,
                        vec!["running".to_string()],
                        "the subagent must be handed off for the running prompt ({case}); the call's result was: {result:?}"
                    );
                    assert!(
                        result.contains("moved to the background"),
                        "the model must be told the subagent moved ({case}): {result:?}"
                    );
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_message_leaves_a_background_task_call_alone() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            for steer in [true, false] {
                crate::util::config::set_follow_up_steer_cache(steer);
                let (handed_off, _returned, _result) =
                    run_task_call_with_queued_message(true, false).await;
                assert!(
                    handed_off.is_empty(),
                    "a background call has no foreground wait to cut short (steer={steer}): {handed_off:?}"
                );
            }
        })
        .await;
}

#[test]
fn foreground_task_call_predicate_reads_the_flag_leniently() {
    use super::tool_calls::is_foreground_task_call;
    let call = |name: &str, args: serde_json::Value| is_foreground_task_call(name, &args);
    assert!(call(
        "task",
        serde_json::json!({"run_in_background": false})
    ));
    assert!(call(
        "Task",
        serde_json::json!({"run_in_background": "false"})
    ));
    assert!(!call(
        "task",
        serde_json::json!({"run_in_background": true})
    ));
    assert!(!call("task", serde_json::json!({})));
    assert!(!call(
        "bash",
        serde_json::json!({"run_in_background": false})
    ));
}
