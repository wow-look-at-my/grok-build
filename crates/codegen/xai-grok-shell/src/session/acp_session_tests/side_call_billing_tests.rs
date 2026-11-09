//! A model call outside the main turn loop bills the session ledger as a side call.

use super::support::*;
use super::*;
use xai_grok_sampling_types::ConversationItem;
use xai_grok_test_support::MockInferenceServer;

const TICKS: i64 = 3_300_000;

/// Point the session's sampler at a Messages mock that prices every call at [`TICKS`].
async fn priced_messages_server(actor: &SessionActor, text: &str) -> MockInferenceServer {
    let server = MockInferenceServer::start().await.unwrap();
    server.set_response(text);
    server.set_messages_cost_usd_ticks(Some(TICKS));
    let mut cfg = actor.chat_state_handle.get_sampling_config().await.unwrap();
    cfg.base_url = server.url();
    cfg.api_backend = crate::sampling::ApiBackend::Messages;
    actor.chat_state_handle.update_sampling_config(cfg);
    server
}

async fn session_ledger(actor: &SessionActor) -> xai_chat_state::UsageLedger {
    actor
        .chat_state_handle
        .try_get_session_usage()
        .await
        .expect("session ledger")
}

fn seed_conversation(actor: &SessionActor) {
    actor.chat_state_handle.replace_conversation(vec![
        ConversationItem::system("you are a coding agent"),
        ConversationItem::user("fix the lexer"),
        ConversationItem::assistant("patched the offset"),
    ]);
}

/// `/btw` goes through `prepare_side_call` and the retrying aux collector.
#[tokio::test(flavor = "current_thread")]
async fn a_side_question_bills_the_session_ledger() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) = tokio::sync::mpsc::unbounded_channel();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;
            let server = priced_messages_server(&actor, "an answer").await;
            seed_conversation(&actor);

            actor
                .handle_side_question("what changed?", Vec::new())
                .await
                .expect("side question must succeed");
            assert_eq!(server.messages_request_count(), 1);

            let ledger = session_ledger(&actor).await;
            assert_eq!(ledger.totals.cost_usd_ticks, Some(TICKS));
            assert_eq!(ledger.totals.model_calls, 1);
            assert_eq!(
                ledger.main_loop_model_calls, 0,
                "a side call is not a main-loop round"
            );
        })
        .await;
}

/// The provider bills a length-cut response, so the call bills even though the side call fails.
#[tokio::test(flavor = "current_thread")]
async fn a_side_call_the_length_policy_rejects_still_bills() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) = tokio::sync::mpsc::unbounded_channel();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;
            let server = priced_messages_server(&actor, "a cut answer").await;
            server.set_messages_stop_reason("max_tokens");
            seed_conversation(&actor);

            let result = actor
                .handle_side_question("what changed?", Vec::new())
                .await;
            assert!(result.is_err(), "a length-cut side call must fail");

            let ledger = session_ledger(&actor).await;
            assert_eq!(ledger.totals.cost_usd_ticks, Some(TICKS));
            assert_eq!(ledger.main_loop_model_calls, 0);
        })
        .await;
}

/// The first auto title is a plain collect on its own client. It bills under the model the request named.
#[tokio::test(flavor = "current_thread")]
async fn the_first_title_call_bills_under_its_own_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) = tokio::sync::mpsc::unbounded_channel();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("Fix the lexer offset");
            server.set_messages_cost_usd_ticks(Some(TICKS));
            let client = xai_grok_sampler::SamplingClient::new(xai_grok_sampler::SamplerConfig {
                api_key: Some("test-key".into()),
                base_url: server.url(),
                model: "client-default-model".into(),
                api_backend: crate::sampling::ApiBackend::Messages,
                ..Default::default()
            })
            .unwrap();

            let _title = crate::session::helpers::session_summary::generate_session_summary(
                "fix the lexer".into(),
                client,
                "title-model",
                Some(&actor.chat_state_handle),
            )
            .await;
            assert_eq!(server.messages_request_count(), 1);

            let ledger = session_ledger(&actor).await;
            assert_eq!(ledger.totals.cost_usd_ticks, Some(TICKS));
            assert_eq!(ledger.main_loop_model_calls, 0);
            assert!(
                ledger.by_model.contains_key("title-model"),
                "the call bills under the model it sent: {:?}",
                ledger.by_model.keys().collect::<Vec<_>>()
            );
        })
        .await;
}

/// The auto-mode classifier side query runs in its own worker. It bills the session it was wired on.
#[tokio::test(flavor = "current_thread")]
async fn the_permission_classifier_side_query_bills_the_session_ledger() {
    use xai_grok_workspace::permission::{AccessKind, ClientType, PermissionRequest};

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) = tokio::sync::mpsc::unbounded_channel();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let cwd = xai_grok_paths::AbsPathBuf::new(std::path::PathBuf::from(
                actor.session_info.cwd.clone(),
            ))
            .unwrap();
            let (gw_tx, _gw_rx) = tokio::sync::mpsc::unbounded_channel();
            let (handle, _events) = xai_grok_workspace::permission::spawn_permission_manager(
                actor.session_info.id.clone(),
                xai_acp_lib::AcpAgentGatewaySender::new(gw_tx),
                cwd,
                ClientType::Generic,
                None,
                vec![],
                vec![],
                false,
                None,
            );
            actor.permissions = handle;
            actor.permissions.set_auto_mode(true);
            let server = priced_messages_server(
                &actor,
                r#"{"thinking":"fine","shouldBlock":false,"reason":"ok"}"#,
            )
            .await;
            let session = std::sync::Arc::new(actor);
            session.wire_permission_auto_llm_classifier().await;
            assert!(session.permissions.has_llm_side_query());

            // The heuristic pre-pass fast-Allows a routine command and never
            // asks the model. This command stays outside that allowlist: the
            // side query is what answers it, and that call must bill.
            let _ = session
                .permissions
                .request(PermissionRequest::new(
                    AccessKind::Bash("terraform apply -auto-approve".into()),
                    agent_client_protocol::ToolCallUpdate::new(
                        agent_client_protocol::ToolCallId::new(std::sync::Arc::from("tc-bill")),
                        Default::default(),
                    ),
                ))
                .await;
            assert!(
                server.messages_request_count() >= 1,
                "the classifier must reach the model: {}",
                server.request_log_summary()
            );

            let ledger = session_ledger(&session).await;
            assert_eq!(
                ledger.totals.cost_usd_ticks,
                Some(TICKS * server.messages_request_count() as i64)
            );
            assert_eq!(ledger.main_loop_model_calls, 0);
        })
        .await;
}
