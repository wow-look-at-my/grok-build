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
            ..Default::default()
        },
        mtls_cert_dir: None,
        api_key: Some("test-key".to_string()),
        env_key: None,
        auth_provider: None,
        api_base_url: Some("http://127.0.0.1:9/v1".to_string()),
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
