//! A harness model slot brings its own sampler to a side call.
//!
//! `prepare_side_call` used to take the session's client and write the
//! slot's model id onto it, which sends one model's id to another model's
//! endpoint. The slot now resolves its own client from the catalog, and a
//! slot that resolves to nothing keeps the session's client AND the session's
//! model — never the pinned id on a client that cannot serve it.

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

/// A pin the session cannot reach falls back to the session model.
///
/// This is the defect: the old path paired the pinned id with the session's
/// own client, so an unreachable model reached the session model's endpoint
/// under a name that endpoint does not serve.
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
