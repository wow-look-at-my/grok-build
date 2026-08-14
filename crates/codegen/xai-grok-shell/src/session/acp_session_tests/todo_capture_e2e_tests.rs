//! `/todo` capture end to end against a scripted model.
//!
//! The unit tests around the capture gate prove the sanitizer in isolation.
//! This drives the real loop — model call, tool dispatch, todo-list write —
//! against a mock inference server, because the one guarantee `/todo` makes is
//! about the state the session is left in, and nothing short of running the
//! loop can show that.
//!
//! The scripted response is deliberately adversarial: reasoning followed by a
//! `todo_write` that asks for `merge: false` (a replace), targets an id the
//! main agent is using, and flips it to `completed`. Every one of those is one
//! JSON field away from destroying the main agent's list.

use super::support::*;
use super::*;
use std::sync::Arc;
use std::time::Duration;
use xai_grok_test_support::sse::responses_api_reasoning_then_tool_call_events;
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse};

/// What the capture agent asks for: a replace, over the main agent's own item,
/// marking it completed — plus the item it was actually asked to add.
const ADVERSARIAL_TODO_ARGS: &str = r#"{"merge":false,"todos":[{"id":"t1","content":"ship the release","status":"completed"},{"id":"t2","content":"Add a second remote to ci/push.sh","status":"in_progress"}]}"#;

/// The main agent's in-flight item, seeded through the real tool before the
/// capture runs.
const SEED_TODO_ARGS: &str =
    r#"{"merge":true,"todos":[{"id":"t1","content":"ship the release","status":"in_progress"}]}"#;

fn drain_gateway(mut rx: tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>) {
    tokio::task::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let xai_acp_lib::AcpClientMessage::SessionNotification(args) = msg {
                let _ = args.response_tx.send(Ok(()));
            }
        }
    });
}

fn drain_persistence(mut rx: tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>) {
    tokio::task::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let PersistenceMsg::FlushAndAck { respond_to } = msg {
                let _ = respond_to.send(Ok(()));
            }
        }
    });
}

/// Every todo on the session's list, as `(id, content, status)`.
async fn todo_list(actor: &SessionActor) -> Vec<(String, String, crate::tools::todo::TodoStatus)> {
    use crate::tools::todo::TodoState;
    use xai_grok_tools::types::resources::State;
    actor
        .agent
        .borrow()
        .tool_bridge()
        .read_resource::<State<TodoState>>()
        .await
        .map(|state| {
            state
                .0
                .todo_items_with_ids()
                .map(|(id, item)| (id.clone(), item.content.clone(), item.status))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "current_thread")]
async fn a_capture_appends_and_cannot_touch_the_main_agent_s_items() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_reasoning_then_tool_call_events(
                    "the push script is ci/push.sh",
                    "call_capture_1",
                    "todo_write",
                    ADVERSARIAL_TODO_ARGS,
                    "test",
                )),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;

            let mut cfg = actor
                .chat_state_handle
                .get_sampling_config()
                .await
                .expect("test actor has sampling config");
            cfg.base_url = server.url();
            cfg.api_backend = xai_grok_sampling_types::ApiBackend::Responses;
            cfg.model = "test".to_string();
            actor.chat_state_handle.update_sampling_config(cfg);
            let mut creds = actor.chat_state_handle.get_credentials().await;
            creds.api_key = Some("test-key".to_string());
            actor.chat_state_handle.update_credentials(creds);

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

            // The main agent's own item, written the way the main agent writes it.
            actor
                .workspace_ops
                .call_tool(
                    "todo_write",
                    serde_json::from_str(SEED_TODO_ARGS).unwrap(),
                    "seed",
                    Some(&actor.session_info.id.0),
                )
                .await
                .expect("seed todo");

            let outcome = tokio::time::timeout(
                Duration::from_secs(60),
                actor.handle_todo_capture("add a way to push changes to 2 git repos"),
            )
            .await
            .expect("capture must finish within timeout")
            .expect("capture must succeed");

            assert_eq!(
                outcome.added,
                vec![
                    "ship the release".to_string(),
                    "Add a second remote to ci/push.sh".to_string()
                ],
                "both items the model sent are appended as new items — including the one \
                 whose id collided with the main agent's, which is added rather than applied \
                 to `t1`"
            );

            let todos = todo_list(&actor).await;
            let (seed_id, seed_content, seed_status) = todos
                .iter()
                .find(|(id, _, _)| id == "t1")
                .expect("the main agent's item must still be on the list")
                .clone();
            assert_eq!(seed_id, "t1");
            assert_eq!(seed_content, "ship the release");
            assert_eq!(
                seed_status,
                crate::tools::todo::TodoStatus::InProgress,
                "the capture asked to complete this item; it must still be in progress"
            );

            let captured: Vec<_> = todos
                .iter()
                .filter(|(id, _, _)| id.starts_with("capture-"))
                .collect();
            assert_eq!(
                captured.len(),
                2,
                "each item the capture sent lands as its own new todo: {todos:?}"
            );
            for (_, _, status) in &captured {
                assert_eq!(
                    *status,
                    crate::tools::todo::TodoStatus::Pending,
                    "a capture cannot set a status; the model asked for completed and \
                     in_progress: {todos:?}"
                );
            }
            assert_eq!(
                todos.len(),
                3,
                "`merge: false` must not have replaced the list: {todos:?}"
            );
        })
        .await;
}

/// The capture never touches the conversation the main agent is working in —
/// that is what makes it safe to run mid-turn.
#[tokio::test(flavor = "current_thread")]
async fn a_capture_leaves_the_parent_conversation_alone() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_reasoning_then_tool_call_events(
                    "nothing to look up",
                    "call_capture_1",
                    "todo_write",
                    r#"{"todos":[{"id":"x","content":"Add a second remote to ci/push.sh"}]}"#,
                    "test",
                )),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;

            let mut cfg = actor
                .chat_state_handle
                .get_sampling_config()
                .await
                .expect("test actor has sampling config");
            cfg.base_url = server.url();
            cfg.api_backend = xai_grok_sampling_types::ApiBackend::Responses;
            cfg.model = "test".to_string();
            actor.chat_state_handle.update_sampling_config(cfg);
            let mut creds = actor.chat_state_handle.get_credentials().await;
            creds.api_key = Some("test-key".to_string());
            actor.chat_state_handle.update_credentials(creds);

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

            let before = actor.chat_state_handle.get_conversation().await;
            let outcome = tokio::time::timeout(
                Duration::from_secs(60),
                actor.handle_todo_capture("push to two remotes"),
            )
            .await
            .expect("capture must finish within timeout")
            .expect("capture must succeed");
            assert_eq!(outcome.added.len(), 1);

            let after = actor.chat_state_handle.get_conversation().await;
            assert_eq!(
                before.len(),
                after.len(),
                "the capture's own turns must not reach the parent conversation: \
                 before={before:#?} after={after:#?}"
            );
        })
        .await;
}

/// A session whose task-list tool cannot express an append is refused before
/// any model call, rather than written through with semantics that would
/// replace the main agent's list.
#[tokio::test(flavor = "current_thread")]
async fn a_replace_only_task_list_tool_is_refused() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            // opencode's `todowrite` is `ToolKind::Plan` too, and replaces the
            // whole list — the case the namespace check exists for.
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![
                xai_grok_tools::registry::types::ToolConfig::for_tool::<
                    xai_grok_tools::implementations::opencode::todowrite::TodoWriteTool,
                >(),
            ])
            .await;

            let err = actor
                .handle_todo_capture("add something")
                .await
                .expect_err("a replace-only task-list tool must be refused");
            assert!(
                matches!(err, TodoCaptureError::UnsupportedTodoTool(_)),
                "got {err:?}"
            );
            // No model was configured, so reaching one would have failed
            // differently — the refusal has to come first.
            assert!(
                err.to_string().contains("append-capable"),
                "the message must say what is missing: {err}"
            );
        })
        .await;
}
