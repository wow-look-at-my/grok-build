//! A `/goal` role's configured model, resolved on a real session actor: what
//! it runs on, and what the user is told when it cannot run there.

use super::support::*;
use super::*;
use crate::agent::config::{ModelEntry, ModelInfo};
use crate::extensions::notification::SessionUpdate as XaiUpdate;

fn catalog_entry(model: &str, user_selectable: bool) -> ModelEntry {
    let mut info = ModelInfo::fallback(model);
    info.user_selectable = user_selectable;
    ModelEntry {
        info,
        api_key: None,
        env_key: None,
        auth_provider: None,
        api_base_url: None,
    }
}

/// Every `GoalRoleModelFallback` notice the session persisted.
fn fallback_notices(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>,
) -> Vec<XaiUpdate> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PersistenceMsg::Update(crate::session::storage::SessionUpdate::Xai(n)) = msg
            && matches!(n.update, XaiUpdate::GoalRoleModelFallback { .. })
        {
            out.push(n.update);
        }
    }
    out
}

/// `allowed_models` limits the chat picker and exempts subagents. A goal role
/// is a subagent the user configured, so a model outside that list still runs.
#[tokio::test(flavor = "current_thread")]
async fn a_goal_role_runs_on_a_model_outside_allowed_models() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, mut persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.models_manager.insert_test_entry(
                "outside-allowlist",
                catalog_entry("outside-allowlist", false),
            );

            let resolved = actor
                .resolve_goal_role_model_only("skeptic", Some(0), "outside-allowlist")
                .await;

            assert_eq!(
                resolved.model.as_deref(),
                Some("outside-allowlist"),
                "allowed_models must not move a goal role onto the session model"
            );
            assert!(fallback_notices(&mut persistence_rx).is_empty());
        })
        .await;
}

/// A model the session does not know falls back, and the user is told which
/// model, why, and how to fix it.
#[tokio::test(flavor = "current_thread")]
async fn an_unknown_goal_role_model_falls_back_and_says_how_to_fix_it() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, mut persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;

            let resolved = actor
                .resolve_goal_role_model_only("skeptic", Some(1), "no-such-model")
                .await;

            assert_eq!(
                resolved.model, None,
                "an unknown model runs on the session model"
            );
            let notices = fallback_notices(&mut persistence_rx);
            assert_eq!(notices.len(), 1, "one notice per fallback: {notices:?}");
            let XaiUpdate::GoalRoleModelFallback {
                role,
                skeptic_idx,
                requested_model,
                reason,
                detail,
                ..
            } = &notices[0]
            else {
                unreachable!()
            };
            assert_eq!(role, "skeptic");
            assert_eq!(*skeptic_idx, Some(1));
            assert_eq!(requested_model, "no-such-model");
            assert_eq!(reason, "model_unknown");
            let detail = detail.as_deref().unwrap_or_default();
            assert!(
                detail.contains("/model") && detail.contains("restart"),
                "the notice must say how to fix it: {detail}"
            );
        })
        .await;
}
