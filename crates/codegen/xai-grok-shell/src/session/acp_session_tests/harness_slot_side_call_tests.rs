//! A harness model slot brings its own sampler to a side call.

use super::support::*;
use super::*;
use crate::session::harness_models::ResolvedHarnessModels;

/// The session's own model, as the side call would use it unpinned.
async fn session_model(actor: &SessionActor) -> String {
    actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .map(|c| c.model)
        .unwrap_or_default()
}

fn catalog_entry(model_id: &str) -> crate::agent::config::ModelEntry {
    crate::agent::config::ModelEntry {
        info: crate::agent::config::ModelInfo {
            user_selectable: true,
            model: model_id.to_string(),
            context_window: std::num::NonZeroU64::new(128_000).unwrap(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            ..Default::default()
        },
        mtls_cert_dir: None,
        api_key: Some("test-key".to_string()),
        env_key: None,
        auth_provider: None,
        api_base_url: None,
    }
}

/// A slot change on a live session reaches its next compaction request.
/// The actor is never rebuilt here, which is the case a settings change
/// hits while a session runs.
#[tokio::test(flavor = "current_thread")]
async fn a_reload_moves_a_pinned_slot_on_a_live_session() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let tmp = tempfile::TempDir::new().unwrap();
            let mut catalog = indexmap::IndexMap::new();
            for id in ["slot-old", "slot-new"] {
                catalog.insert(id.to_string(), catalog_entry(id));
            }
            actor.models_manager = crate::agent::remote_config::ModelsManager::new(
                None,
                catalog,
                agent_client_protocol::ModelId::new("default"),
                std::sync::Arc::new(xai_grok_login::AuthManager::new(
                    tmp.path(),
                    Default::default(),
                )),
                crate::agent::config::Config::default(),
            );
            actor.harness_models = ResolvedHarnessModels::from_pairs(&[("compaction", "slot-old")]);

            let (_, cfg) = actor
                .resolve_slot_sampler("compaction")
                .await
                .expect("the pinned slot resolves to its own sampler");
            assert_eq!(cfg.model, "slot-old");

            let mut config = crate::agent::config::Config::default();
            config.models.compaction = Some("slot-new".to_owned());
            actor.harness_models.reload(&config);

            let (_, cfg) = actor
                .resolve_slot_sampler("compaction")
                .await
                .expect("the reloaded slot resolves to its own sampler");
            assert_eq!(
                cfg.model, "slot-new",
                "after a reload the running session must stop calling the old model"
            );
        })
        .await;
}

/// An unset slot changes nothing: the side call keeps the session's model
/// and the session's window, so a user who pinned no slot is unaffected.
#[tokio::test(flavor = "current_thread")]
async fn an_unset_slot_keeps_the_session_model_and_window() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;

            let expected = session_model(&actor).await;
            for slot in ["recap", "turn_summary", "side_note", "todo_capture"] {
                let setup = actor
                    .prepare_side_call(slot)
                    .await
                    .unwrap_or_else(|e| panic!("`{slot}` side call setup failed: {e}"));
                assert_eq!(
                    setup.model, expected,
                    "an unset `{slot}` must keep the session model"
                );
                assert_eq!(
                    setup.context_window, 256_000,
                    "an unset `{slot}` must keep the session window"
                );
            }
        })
        .await;
}

/// A pin the session cannot reach falls back to the session model. This is
/// the defect: the path paired the pinned id with the session's own client.
/// An unreachable model reached the session model's endpoint under a name
/// that endpoint does not serve.
#[tokio::test(flavor = "current_thread")]
async fn an_unreachable_pin_falls_back_to_the_session_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.harness_models =
                ResolvedHarnessModels::from_pairs(&[("side_note", "no-such-model-anywhere")]);

            let expected = session_model(&actor).await;
            let setup = actor
                .prepare_side_call("side_note")
                .await
                .expect("side call setup");
            assert_eq!(
                setup.model, expected,
                "an unreachable pin must fall back to the session model, not ride the \
                 session's client under a name it cannot serve"
            );
        })
        .await;
}

/// Every compaction summary samples through `compaction_sampler`. The
/// `dense_summarization` group model must reach it, with that model's window,
/// even over a `[models] compaction` pin.
#[tokio::test(flavor = "current_thread")]
async fn compaction_samples_with_the_dense_summarization_group_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let tmp = tempfile::TempDir::new().unwrap();
            let mut catalog = indexmap::IndexMap::new();
            for id in ["dense-model", "own-pin"] {
                catalog.insert(id.to_string(), catalog_entry(id));
            }
            actor.models_manager = crate::agent::remote_config::ModelsManager::new(
                None,
                catalog,
                agent_client_protocol::ModelId::new("default"),
                std::sync::Arc::new(xai_grok_login::AuthManager::new(
                    tmp.path(),
                    Default::default(),
                )),
                crate::agent::config::Config::default(),
            );
            let mut config = crate::agent::config::Config::default();
            config.models.compaction = Some("own-pin".to_owned());
            config.model_groups.insert(
                "dense_summarization".to_owned(),
                xai_grok_models::ModelGroupConfig {
                    model: Some("dense-model".to_owned()),
                    ..Default::default()
                },
            );
            actor.harness_models.reload(&config);

            let (_, cfg) = actor
                .compaction_sampler()
                .await
                .expect("the compaction sampler resolves");
            assert_eq!(cfg.model, "dense-model");
            assert_eq!(
                cfg.context_window, 128_000,
                "the window belongs to the dense model, not to the session"
            );
        })
        .await;
}

/// Two-pass prefire one pass is an auto-compaction sample too. It must go
/// to the dense model's endpoint, not to the session's.
#[tokio::test(flavor = "current_thread")]
async fn two_pass_prefire_samples_with_the_dense_summarization_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = xai_grok_test_support::MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let tmp = tempfile::TempDir::new().unwrap();
            let mut dense = catalog_entry("dense-model");
            dense.info.base_url = server.url();
            let mut catalog = indexmap::IndexMap::new();
            catalog.insert("dense-model".to_string(), dense);
            actor.models_manager = crate::agent::remote_config::ModelsManager::new(
                None,
                catalog,
                agent_client_protocol::ModelId::new("default"),
                std::sync::Arc::new(xai_grok_login::AuthManager::new(
                    tmp.path(),
                    Default::default(),
                )),
                crate::agent::config::Config::default(),
            );
            let mut config = crate::agent::config::Config::default();
            config.model_groups.insert(
                "dense_summarization".to_owned(),
                xai_grok_models::ModelGroupConfig {
                    model: Some("dense-model".to_owned()),
                    ..Default::default()
                },
            );
            actor.harness_models.reload(&config);
            {
                let mut agent_slot = actor.agent.borrow_mut();
                let agent = &*agent_slot;
                let mut policy = agent.compaction_policy().clone();
                policy.two_pass_enabled = true;
                *agent_slot = xai_grok_agent::Agent::new(
                    agent.definition().clone(),
                    agent.prompt_context().clone(),
                    agent.system_prompt().to_string(),
                    std::sync::Arc::clone(agent.tool_bridge()),
                    agent.reminder_policy().clone(),
                    policy,
                    vec![],
                    false,
                );
            }
            use xai_grok_sampling_types::ConversationItem;
            actor.chat_state_handle.replace_conversation(vec![
                ConversationItem::system("you are a coding agent"),
                ConversationItem::user("q1"),
                ConversationItem::assistant("a1"),
                ConversationItem::user("q2"),
                ConversationItem::assistant("a2"),
            ]);
            let actor = std::sync::Arc::new(actor);

            actor.run_prefire_pass1().await;

            let models: Vec<String> = server
                .requests()
                .into_iter()
                .filter_map(|r| r.body?.get("model")?.as_str().map(str::to_owned))
                .collect();
            assert!(
                !models.is_empty() && models.iter().all(|m| m == "dense-model"),
                "pass 1 must sample with the dense model, got {models:?}"
            );
        })
        .await;
}

/// With no dense model set, compaction keeps the session's model and window.
#[tokio::test(flavor = "current_thread")]
async fn compaction_without_a_dense_model_keeps_the_session_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let expected = session_model(&actor).await;
            let (_, cfg) = actor
                .compaction_sampler()
                .await
                .expect("the compaction sampler resolves");
            assert_eq!(cfg.model, expected);
            assert_eq!(cfg.context_window, 256_000);
        })
        .await;
}
