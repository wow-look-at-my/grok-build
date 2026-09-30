//! `x.ai/debug/*` extension handlers for local client testing.
//!
//! These methods bypass heuristics, sampling, cooldowns, and enabled checks.
//! Client engineers can exercise a notification and its response without real experiments, real sessions, or real model inference.
//!
//! - `trigger_feedback`: fire a synthetic `FeedbackRequestNotification`.
//! - `arm_auto_compact`: make the next turn trigger auto-compaction unconditionally, regardless of context window usage.
//! - `agent`: agent-process diagnostics (registry counts).

use agent_client_protocol as acp;

use super::{ExtResult, parse_params};
use crate::agent::MvpAgent;
use crate::session::{ExtMethodResult, SessionCommand};

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        "x.ai/debug/trigger_feedback" => {
            tracing::info!("debug: triggering test feedback request");
            handle_trigger_feedback(agent, args).await
        }
        "x.ai/debug/arm_auto_compact" => handle_arm_auto_compact(agent, args),
        "x.ai/debug/agent" => handle_agent(agent).await,
        _ => Err(acp::Error::method_not_found()),
    }
}

async fn handle_agent(agent: &MvpAgent) -> ExtResult {
    let registries = agent.registry_snapshot().await;
    ExtMethodResult::success(serde_json::json!({ "registries": registries }))
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}

/// Params for `x.ai/debug/trigger_feedback`, as an ACP client sends them.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", try_from = "DebugTriggerParamsWire")]
struct DebugTriggerParams {
    session_id: String,
    /// "tier1" | "tier2" | "tier3" (default: "tier1")
    #[serde(default)]
    tier: Option<String>,
    /// "thumbs" | "stars" | "text" | "thumbs_text" | "stars_text" (default: "thumbs_text")
    #[serde(default)]
    mode: Option<String>,
}

impl DebugTriggerParams {
    /// The keys `session_id` is read under. ACP params are camelCase, which is
    /// what the container's own renaming reads the field as; the snake_case
    /// spelling arrives from shell-side callers.
    const SESSION_ID_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("sessionId", &["session_id"]);
}

/// `DebugTriggerParams` with each session-key spelling as its own field, so a
/// request naming both folds them instead of tripping serde's duplicate-field
/// check.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugTriggerParamsWire {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default, rename = "session_id")]
    session_id_snake: Option<String>,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

impl TryFrom<DebugTriggerParamsWire> for DebugTriggerParams {
    type Error = DebugTriggerParamsError;

    fn try_from(wire: DebugTriggerParamsWire) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: DebugTriggerParams::SESSION_ID_KEYS
                .fold(vec![wire.session_id, wire.session_id_snake])?
                .ok_or(DebugTriggerParamsError::MissingSessionId)?,
            tier: wire.tier,
            mode: wire.mode,
        })
    }
}

/// Why debug trigger params could not be read; `parse_params` turns it into an
/// `invalid_params` ACP error.
#[derive(Debug)]
enum DebugTriggerParamsError {
    Alias(xai_tool_types::AliasConflict),
    /// `session_id` was required before the shadow and stays required after it.
    MissingSessionId,
}

impl std::fmt::Display for DebugTriggerParamsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alias(conflict) => conflict.fmt(f),
            Self::MissingSessionId => f.write_str("missing field `session_id`"),
        }
    }
}

impl std::error::Error for DebugTriggerParamsError {}

impl From<xai_tool_types::AliasConflict> for DebugTriggerParamsError {
    fn from(value: xai_tool_types::AliasConflict) -> Self {
        Self::Alias(value)
    }
}

async fn handle_trigger_feedback(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    use crate::session::feedback::{FeedbackMode, FeedbackTier};

    let params: DebugTriggerParams = parse_params(args)?;

    let tier = match params.tier.as_deref() {
        Some("tier2") => FeedbackTier::Tier2,
        Some("tier3") => FeedbackTier::Tier3,
        Some("tier1") | None => FeedbackTier::Tier1,
        Some(other) => {
            return Err(acp::Error::invalid_params().data(format!(
                "unknown tier: {other:?} (expected tier1/tier2/tier3)"
            )));
        }
    };

    let mode = match params.mode.as_deref() {
        Some("thumbs") => FeedbackMode::Thumbs,
        Some("stars") => FeedbackMode::Stars,
        Some("text") => FeedbackMode::Text,
        Some("stars_text") => FeedbackMode::StarsText,
        Some("thumbs_text") | None => FeedbackMode::ThumbsText,
        Some(other) => {
            return Err(acp::Error::invalid_params().data(format!(
                "unknown mode: {other:?} (expected thumbs/stars/text/thumbs_text/stars_text)"
            )));
        }
    };

    let session_id = acp::SessionId::new(params.session_id.clone());
    let handle = agent.resident_handle(&session_id).ok_or_else(|| {
        acp::Error::invalid_params().data(format!("session not found: {}", params.session_id))
    })?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    handle
        .cmd_tx
        .send(SessionCommand::TriggerTestFeedback {
            tier,
            mode,
            respond_to: tx,
        })
        .map_err(|_| {
            acp::Error::internal_error().data("failed to dispatch debug trigger to session")
        })?;

    rx.await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(format!("Internal error: {e:?}")))
}

fn handle_arm_auto_compact(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let params: serde_json::Value = parse_params(args)?;

    let session_id_str = params
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .or_else(|| params.get("session_id").and_then(serde_json::Value::as_str))
        .ok_or_else(|| acp::Error::invalid_params().data("sessionId required"))?;
    let session_id = acp::SessionId::new(session_id_str);

    let handle = agent
        .resident_handle(&session_id)
        .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;

    handle
        .force_compact
        .store(true, std::sync::atomic::Ordering::Relaxed);

    tracing::info!(
        session_id = %session_id_str,
        "debug: armed auto-compact for next turn"
    );

    ExtMethodResult::success(serde_json::json!({ "armed": true }))
        .to_ext_response()
        .map_err(|e| acp::Error::internal_error().data(e.to_string()))
}

#[cfg(test)]
mod wire_alias_tests {
    use super::DebugTriggerParams;

    #[test]
    fn trigger_params_read_the_session_id_under_either_spelling() {
        for json in [
            r#"{"sessionId":"s1"}"#,
            r#"{"session_id":"s1"}"#,
            r#"{"session_id":"s1","sessionId":"s1"}"#,
        ] {
            let params: DebugTriggerParams =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(params.session_id, "s1");
        }
    }

    #[test]
    fn trigger_params_whose_session_id_spellings_disagree_error_naming_the_field() {
        let err =
            serde_json::from_str::<DebugTriggerParams>(r#"{"session_id":"a","sessionId":"b"}"#)
                .expect_err("two session ids must not resolve silently");
        let message = err.to_string();
        assert!(message.contains("sessionId"), "{message}");
        assert!(message.contains("session_id"), "{message}");
    }

    #[test]
    fn trigger_params_with_no_session_id_at_all_are_still_an_error() {
        let err = serde_json::from_str::<DebugTriggerParams>(r#"{"tier":"tier2"}"#)
            .expect_err("the handler addresses a session by id");
        assert!(err.to_string().contains("session_id"), "{err}");
    }
}
