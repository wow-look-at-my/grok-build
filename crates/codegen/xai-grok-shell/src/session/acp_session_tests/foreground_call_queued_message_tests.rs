//! A user message queued while a foreground shell command or MCP call runs
//! moves that call to the background. The turn continues at once.

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

fn mcp_call(id: &str) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            SLOW_MCP_TOOL,
            serde_json::json!({}).to_string(),
        ),
        vendor: Default::default(),
    }
}

const SLOW_MCP_TOOL: &str = "slow_server__fetch";
const SLOW_MCP_RESULT: &str = "SLOW_MCP_RESULT_TEXT";

/// An MCP tool that answers only once `release` fires.
#[derive(Debug, Clone)]
struct SlowMcpTool {
    release: Arc<tokio::sync::Notify>,
}
impl xai_grok_tools::types::tool_metadata::ToolMetadata for SlowMcpTool {
    fn kind(&self) -> xai_grok_tools::types::tool::ToolKind {
        xai_grok_tools::types::tool::ToolKind::Other
    }
    fn tool_namespace(&self) -> xai_grok_tools::types::tool::ToolNamespace {
        xai_grok_tools::types::tool::ToolNamespace::MCP
    }
    fn description_template(&self) -> &str {
        "slow MCP tool"
    }
}
impl xai_tool_runtime::Tool for SlowMcpTool {
    type Args = serde_json::Value;
    type Output = xai_grok_tools::types::output::ToolOutput;
    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(SLOW_MCP_TOOL).expect("valid tool id")
    }
    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(SLOW_MCP_TOOL, "slow MCP tool")
    }
    async fn run(
        &self,
        _ctx: xai_tool_runtime::ToolCallContext,
        _args: serde_json::Value,
    ) -> Result<Self::Output, xai_tool_runtime::ToolError> {
        self.release.notified().await;
        Ok(xai_grok_tools::types::output::ToolOutput::Text(
            xai_grok_tools::types::output::TextOutput::from(SLOW_MCP_RESULT.to_owned()),
        ))
    }
}

/// A running turn with one client "A" row at the front, and a plain row from "A" ready to queue.
async fn running_actor(
    tools: Vec<xai_grok_tools::registry::types::ToolConfig>,
) -> (SessionActor, QueueInputRequest) {
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    *actor.agent.borrow_mut() = test_agent_with_tools(tools).await;
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
    let (respond_to, _prx) = tokio::sync::oneshot::channel();
    let mut request = queue_input_request(
        vec![acp::ContentBlock::Text(acp::TextContent::new("hello"))],
        "queued",
        respond_to,
    );
    // The harvest only takes rows owned by the running turn's client, which `user_item` sets to "A".
    request.client_identifier = Some("A".to_string());
    (actor, request)
}

/// Runs `call` on `actor`, queues `request` before or while it runs, and returns how long the call took and its result text.
async fn run_with_queued_message(
    actor: Arc<SessionActor>,
    call: crate::sampling::types::ToolCallResponse,
    request: QueueInputRequest,
    queue_before: bool,
) -> (Duration, String) {
    let call_id = call.id.clone();
    let mut request = Some(request);
    if queue_before && let Some(request) = request.take() {
        let _ = actor.queue_input(request).await;
    }
    let started = Instant::now();
    let call_actor = actor.clone();
    let call =
        tokio::task::spawn_local(
            async move { call_actor.execute_tool_calls(vec![call], None).await },
        );
    // The call is in flight once it raises the blocking wait.
    for _ in 0..40 {
        if actor.tool_context.blocking_wait_depth.depth() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if let Some(request) = request.take() {
        let _ = actor.queue_input(request).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(15), call)
        .await
        .expect("the call must return once a message is queued")
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
            ConversationItem::ToolResult(tr) if tr.tool_call_id == call_id => {
                Some(tr.content.to_string())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
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
					let (actor, request) = running_actor(vec![
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
					let actor = Arc::new(actor);
					let (elapsed, result) = run_with_queued_message(
						actor.clone(),
						command_call("cmd-1", false),
						request,
						queue_before,
					)
					.await;
					actor
						.agent
						.borrow()
						.tool_bridge()
						.kill_all_background_tasks()
						.await;
					let case = format!("steer={steer} queue_before={queue_before}");
					assert!(
						elapsed < Duration::from_secs(15),
						"the call must not wait for `sleep 30` to exit ({case}): took {elapsed:?}"
					);
					assert!(
						result.contains("to background") && result.contains("still running"),
						"the model must be told the command still runs in the background ({case}): {result:?}"
					);
				}
			}
		})
		.await;
}

/// A queued message answers a slow MCP call at once. The call keeps running, and its result arrives later as a completion.
#[tokio::test(flavor = "current_thread")]
#[serial_test::serial(follow_up_steer_cache)]
async fn queued_message_backgrounds_a_foreground_mcp_call() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            crate::util::config::set_follow_up_steer_cache(false);
            let (mut actor, request) = running_actor(Vec::new()).await;
            let (notifications, mut notification_rx) =
                xai_grok_tools::notification::types::ToolNotificationHandle::channel();
            Arc::get_mut(&mut actor.rebuild_spec)
                .expect("the test actor holds the only rebuild spec")
                .tools_notification_handle = notifications;
            // Dispatch refuses an MCP call until the session's MCP init has settled.
            {
                let mut mcp = actor.mcp_state.lock().await;
                std::mem::forget(mcp.try_start_init().expect("fixture claims init"));
                mcp.finish_init();
                mcp.complete_init();
            }
            let release = Arc::new(tokio::sync::Notify::new());
            actor
                .agent
                .borrow()
                .tool_bridge()
                .register_mcp_tools(
                    SLOW_MCP_TOOL.to_owned(),
                    SlowMcpTool {
                        release: release.clone(),
                    },
                    Some(serde_json::json!({"type": "object"})),
                )
                .expect("slow MCP tool registration");
            let actor = Arc::new(actor);
            let (elapsed, result) =
                run_with_queued_message(actor.clone(), mcp_call("mcp-call-1"), request, false)
                    .await;
            assert!(
                elapsed < Duration::from_secs(15),
                "the MCP call must not hold the turn: took {elapsed:?}"
            );
            assert!(
                result.contains("moved to the background") && result.contains("bg-mcp-call-1"),
                "the model must be told the call still runs, under its task id: {result:?}"
            );

            release.notify_one();
            let completion =
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        match notification_rx.recv().await {
						Some(xai_grok_tools::notification::types::ToolNotification::TaskCompleted(
							snapshot,
						)) => return snapshot,
						Some(_) => continue,
						None => panic!("the notification channel closed before the completion"),
					}
                    }
                })
                .await
                .expect("the backgrounded MCP call must report its completion");
            assert_eq!(completion.task_id, "bg-mcp-call-1");
            assert!(completion.completed && completion.is_backgrounded);
            assert_eq!(completion.exit_code, Some(0));
            assert!(
                completion.output.contains(SLOW_MCP_RESULT),
                "the completion carries the result: {:?}",
                completion.output
            );
            let file = std::fs::read_to_string(&completion.output_file)
                .expect("the completion names a file that holds the result");
            assert!(file.contains(SLOW_MCP_RESULT), "{file:?}");
        })
        .await;
}

#[test]
fn foreground_handoff_predicates() {
    use super::tool_calls::{is_detachable_kind, is_foreground_command_call, is_mcp_tool_call};
    use xai_grok_tools::types::tool::ToolKind;
    let command =
        |kind: Option<ToolKind>, args: serde_json::Value| is_foreground_command_call(kind, &args);
    assert!(command(
        Some(ToolKind::Execute),
        serde_json::json!({"command": "ls"})
    ));
    assert!(command(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": false})
    ));
    assert!(!command(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": true})
    ));
    assert!(!command(
        Some(ToolKind::Execute),
        serde_json::json!({"is_background": "true"})
    ));
    assert!(!command(
        Some(ToolKind::Execute),
        serde_json::json!({"background": true})
    ));
    assert!(!command(Some(ToolKind::Read), serde_json::json!({})));
    assert!(!command(None, serde_json::json!({})));
    use xai_grok_tools::types::tool::ToolNamespace;
    assert!(is_mcp_tool_call(Some(ToolNamespace::MCP), None));
    assert!(is_mcp_tool_call(None, Some("slow_server__fetch")));
    assert!(!is_mcp_tool_call(None, None));
    assert!(!is_mcp_tool_call(None, Some("run_terminal_cmd")));
    for kind in [
        ToolKind::WebFetch,
        ToolKind::WebSearch,
        ToolKind::ImageGen,
        ToolKind::Lsp,
    ] {
        assert!(
            is_detachable_kind(Some(kind)),
            "{kind:?} reads or fetches, so it detaches"
        );
    }
    for kind in [
        ToolKind::Edit,
        ToolKind::Write,
        ToolKind::Delete,
        ToolKind::Move,
        ToolKind::AskUser,
        ToolKind::ExitPlan,
        ToolKind::Task,
        ToolKind::Execute,
    ] {
        assert!(
            !is_detachable_kind(Some(kind)),
            "{kind:?} must keep the turn waiting"
        );
    }
    assert!(!is_detachable_kind(None));
}
