//! `x.ai/rewind/*` extension handlers.
//!
//! - `rewind/execute`: rewind a session to a target prompt index, optionally forcing past in-flight prompts and choosing a `RewindMode`.
//! - `rewind/points`: list the prompt indices that can be rewound to.
//!
//! Local mode dispatches [`handle`]. In gateway-bridge mode the agent's routing hook calls [`handle_bridge`].
//! That composes the server's conversation rewind with the local file half, so the pager-facing ACP response is identical either way.
use super::{ExtResult, parse_params, to_raw_response};
use crate::agent::MvpAgent;
use crate::session::handle::SessionHandle;
use crate::session::{RewindMode, RewindRequest, SessionCommand};
use agent_client_protocol as acp;
use serde::Deserialize;
use tokio::sync::oneshot;
#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    tracing::info!("handling rewind request: {}", args.method);
    match args.method.as_ref() {
        "x.ai/rewind/execute" => handle_execute(agent, args).await,
        "x.ai/rewind/points" => handle_points(agent, args).await,
        _ => Err(acp::Error::method_not_found()),
    }
}
#[derive(Debug, Deserialize)]
#[serde(try_from = "RewindSessionRequestWire")]
struct RewindSessionRequest {
    session_id: String,
    #[serde(default)]
    target_prompt_index: Option<usize>,
    #[serde(default)]
    target_response_id: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    mode: Option<RewindMode>,
}

impl RewindSessionRequest {
    /// The keys [`session_id`](Self::session_id) is read under.
    const SESSION_ID_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("session_id", &["sessionId"]);
    /// The keys [`target_prompt_index`](Self::target_prompt_index) is read under.
    const TARGET_PROMPT_INDEX_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("target_prompt_index", &["targetPromptIndex"]);
    /// The keys [`target_response_id`](Self::target_response_id) is read under.
    const TARGET_RESPONSE_ID_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("target_response_id", &["targetResponseId"]);

    fn prompt_index_for_local(&self) -> Result<usize, acp::Error> {
        if let Some(idx) = self.target_prompt_index {
            return Ok(idx);
        }
        if response_id_from_req(self).is_some() {
            return Err(
                acp::Error::invalid_params()
                    .data(
                        "targetResponseId rewind requires a chat/bridge session (use targetPromptIndex for local)",
                    ),
            );
        }
        Err(acp::Error::invalid_params().data("targetPromptIndex or targetResponseId is required"))
    }
}
/// `RewindSessionRequest` as an ACP client sends it, with each key spelling its
/// own field, so a request naming both folds them rather than tripping serde's
/// duplicate-field check. See the `*_KEYS` consts on
/// [`RewindSessionRequest`].
#[derive(Deserialize)]
struct RewindSessionRequestWire {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id_camel: Option<String>,
    #[serde(default)]
    target_prompt_index: Option<usize>,
    #[serde(default, rename = "targetPromptIndex")]
    target_prompt_index_camel: Option<usize>,
    #[serde(default)]
    target_response_id: Option<String>,
    #[serde(default, rename = "targetResponseId")]
    target_response_id_camel: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    mode: Option<RewindMode>,
}

impl TryFrom<RewindSessionRequestWire> for RewindSessionRequest {
    type Error = RewindParamsError;

    fn try_from(wire: RewindSessionRequestWire) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: RewindSessionRequest::SESSION_ID_KEYS
                .fold(vec![wire.session_id, wire.session_id_camel])?
                .ok_or(RewindParamsError::MissingSessionId)?,
            target_prompt_index: RewindSessionRequest::TARGET_PROMPT_INDEX_KEYS.fold(vec![
                wire.target_prompt_index,
                wire.target_prompt_index_camel,
            ])?,
            target_response_id: RewindSessionRequest::TARGET_RESPONSE_ID_KEYS
                .fold(vec![wire.target_response_id, wire.target_response_id_camel])?,
            force: wire.force,
            mode: wire.mode,
        })
    }
}

/// Why rewind params could not be read. `parse_params` turns it into an
/// `invalid_params` ACP error, so the wording reaches the client.
#[derive(Debug)]
enum RewindParamsError {
    Alias(xai_tool_types::AliasConflict),
    /// `session_id` stays required: it was required before the shadow existed.
    MissingSessionId,
}

impl std::fmt::Display for RewindParamsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alias(conflict) => conflict.fmt(f),
            Self::MissingSessionId => f.write_str("missing field `session_id`"),
        }
    }
}

impl std::error::Error for RewindParamsError {}

impl From<xai_tool_types::AliasConflict> for RewindParamsError {
    fn from(value: xai_tool_types::AliasConflict) -> Self {
        Self::Alias(value)
    }
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "RewindPointsRequestWire")]
struct RewindPointsRequest {
    session_id: String,
}

impl RewindPointsRequest {
    /// The keys [`session_id`](Self::session_id) is read under.
    const SESSION_ID_KEYS: xai_tool_types::Aliases = RewindSessionRequest::SESSION_ID_KEYS;
}

/// `RewindPointsRequest` as an ACP client sends it. See
/// [`RewindPointsRequest::SESSION_ID_KEYS`].
#[derive(Deserialize)]
struct RewindPointsRequestWire {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id_camel: Option<String>,
}

impl TryFrom<RewindPointsRequestWire> for RewindPointsRequest {
    type Error = RewindParamsError;

    fn try_from(wire: RewindPointsRequestWire) -> Result<Self, Self::Error> {
        Ok(Self {
            session_id: RewindPointsRequest::SESSION_ID_KEYS
                .fold(vec![wire.session_id, wire.session_id_camel])?
                .ok_or(RewindParamsError::MissingSessionId)?,
        })
    }
}
/// Look up a `SessionHandle` by id string, or return a `resource_not_found`
/// `acp::Error`. Used by both arms below.
fn lookup_session(agent: &MvpAgent, session_id: String) -> Result<SessionHandle, acp::Error> {
    agent
        .resident_handle(&acp::SessionId::new(session_id))
        .ok_or_else(|| acp::Error::resource_not_found(Some("session not found".into())))
}
async fn handle_execute(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let request: RewindSessionRequest = parse_params(args)?;
    let target_prompt_index = request.prompt_index_for_local()?;
    let handle = lookup_session(agent, request.session_id)?;
    let (tx, rx) = oneshot::channel();
    handle
        .cmd_tx
        .send(SessionCommand::Rewind {
            request: RewindRequest {
                target_prompt_index,
                force: request.force,
                mode: request.mode.unwrap_or(RewindMode::All),
            },
            respond_to: tx,
        })
        .map_err(|_| acp::Error::internal_error().data("failed to send rewind command"))?;
    let result = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?
        .map_err(|e| acp::Error::internal_error().data(format!("Rewind failed: {:?}", e)))?;
    to_raw_response(&result)
}
async fn handle_points(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    let request: RewindPointsRequest = parse_params(args)?;
    let handle = lookup_session(agent, request.session_id)?;
    let (tx, rx) = oneshot::channel();
    handle
        .cmd_tx
        .send(SessionCommand::GetRewindPoints { respond_to: tx })
        .map_err(|_| acp::Error::internal_error().data("failed to send command"))?;
    let result = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    to_raw_response(&result)
}
fn response_id_from_req(req: &RewindSessionRequest) -> Option<&str> {
    req.target_response_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod wire_alias_tests {
    use super::{RewindPointsRequest, RewindSessionRequest};

    #[test]
    fn rewind_params_read_the_session_id_under_either_spelling() {
        for json in [
            r#"{"session_id":"s1","target_prompt_index":0}"#,
            r#"{"sessionId":"s1","targetPromptIndex":0}"#,
        ] {
            let request: RewindSessionRequest =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(request.session_id, "s1");
            assert_eq!(request.target_prompt_index, Some(0));
        }

        for json in [r#"{"session_id":"s1"}"#, r#"{"sessionId":"s1"}"#] {
            let request: RewindPointsRequest =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
            assert_eq!(request.session_id, "s1");
        }
    }

    #[test]
    fn rewind_params_naming_both_spellings_under_one_value_parse_once() {
        let request: RewindSessionRequest = serde_json::from_str(
            r#"{"session_id":"s1","sessionId":"s1","target_prompt_index":2,"targetPromptIndex":2,
                "target_response_id":"r1","targetResponseId":"r1"}"#,
        )
        .expect("one value per key, under both spellings");
        assert_eq!(request.session_id, "s1");
        assert_eq!(request.target_prompt_index, Some(2));
        assert_eq!(request.target_response_id.as_deref(), Some("r1"));

        let request: RewindPointsRequest =
            serde_json::from_str(r#"{"session_id":"s1","sessionId":"s1"}"#)
                .expect("one session named twice");
        assert_eq!(request.session_id, "s1");
    }

    /// Differing session ids decide which session gets rewound, and differing
    /// prompt indices decide how far back it goes. Neither may resolve in silence.
    #[test]
    fn rewind_params_whose_spellings_disagree_error_naming_the_field() {
        for (json, field) in [
            (r#"{"session_id":"a","sessionId":"b"}"#, "session_id"),
            (
                r#"{"session_id":"a","target_prompt_index":1,"targetPromptIndex":2}"#,
                "target_prompt_index",
            ),
            (
                r#"{"session_id":"a","target_response_id":"x","targetResponseId":"y"}"#,
                "target_response_id",
            ),
        ] {
            let err = serde_json::from_str::<RewindSessionRequest>(json)
                .expect_err("{json} names one field twice with different values");
            let message = err.to_string();
            assert!(message.contains(field), "{message}");
        }

        let err =
            serde_json::from_str::<RewindPointsRequest>(r#"{"session_id":"a","sessionId":"b"}"#)
                .expect_err("two session ids must not resolve silently");
        assert!(err.to_string().contains("session_id"), "{err}");
    }

    /// `session_id` stayed required before the shadow and stays required after.
    #[test]
    fn rewind_params_with_no_session_id_at_all_are_still_an_error() {
        let err = serde_json::from_str::<RewindSessionRequest>(r#"{"targetPromptIndex":1}"#)
            .expect_err("a rewind with no session is not a thing to guess at");
        assert!(err.to_string().contains("session_id"), "{err}");

        let err = serde_json::from_str::<RewindPointsRequest>("{}")
            .expect_err("listing points needs a session to list them for");
        assert!(err.to_string().contains("session_id"), "{err}");
    }
}
