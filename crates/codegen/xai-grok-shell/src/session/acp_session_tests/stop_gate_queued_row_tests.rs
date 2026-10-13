//! A queued user message ends a turn that the todo stop gate would extend.

use super::support::*;
use super::todo_capture_e2e_tests::{drain_gateway, drain_persistence};
use super::*;
use std::sync::Arc;
use std::time::Duration;
use xai_grok_test_support::MockInferenceServer;

const IN_PROGRESS_TODO: &str =
    r#"{"merge":true,"todos":[{"id":"t1","content":"ship the release","status":"in_progress"}]}"#;

/// A session with one unfinished todo and a running front row, against `server`.
async fn actor_with_unfinished_todo(server: &MockInferenceServer) -> Arc<SessionActor> {
    let (gateway_tx, gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    drain_gateway(gateway_rx);
    let (persistence_tx, persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    drain_persistence(persistence_rx);

    let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
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
    actor
        .workspace_ops
        .call_tool(
            "todo_write",
            serde_json::from_str(IN_PROGRESS_TODO).unwrap(),
            "seed",
            Some(&actor.session_info.id.0),
        )
        .await
        .expect("seed todo");

    {
        let mut state = actor.state.lock().await;
        state.pending_inputs.push_back(user_item("running", "A"));
        state.running_task = Some(running_task_stub("running"));
    }
    *actor
        .current_prompt_id
        .lock()
        .expect("current_prompt_id mutex poisoned") = Some("running".into());
    Arc::new(actor)
}

async fn run_running_turn(actor: &Arc<SessionActor>) {
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        actor.handle_prompt(
            "running",
            vec![acp::ContentBlock::Text(acp::TextContent::new("do the work"))],
            PromptMode::Agent,
            None,
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("the turn must finish within the timeout");
    eprintln!("TURN RESULT: {result:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn unfinished_todo_extends_the_turn_when_nothing_is_queued() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let actor = actor_with_unfinished_todo(&server).await;

            run_running_turn(&actor).await;

            assert!(
                server.request_count() > 1,
                "the todo stop gate must send the model back; got {} request(s)",
                server.request_count()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_user_row_ends_a_turn_the_todo_gate_would_extend() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let actor = actor_with_unfinished_todo(&server).await;
            // Queued before the turn began: the mid-turn harvest leaves it for its own turn.
            actor
                .state
                .lock()
                .await
                .pending_inputs
                .push_back(user_item("queued", "A"));
            actor
                .queued_at_turn_start
                .borrow_mut()
                .insert("queued".to_string());

            run_running_turn(&actor).await;

            assert_eq!(
                server.request_count(),
                1,
                "the turn must end after one round so the queued row runs next"
            );
            assert!(
                actor
                    .state
                    .lock()
                    .await
                    .pending_inputs
                    .iter()
                    .any(|item| item.prompt_id == "queued"),
                "the queued row must stay queued for its own turn"
            );
        })
        .await;
}
