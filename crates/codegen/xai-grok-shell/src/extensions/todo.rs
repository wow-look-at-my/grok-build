//! `x.ai/todo` extension handler.
//!
//! Dispatches a `/todo` capture to the active session via
//! [`SessionCommand::TodoCapture`] and returns the items it appended. Unlike
//! `x.ai/recap` this one waits for the answer: the client shows what landed on
//! the list, so there is nothing to report until the run finishes.

use agent_client_protocol as acp;
use tokio::sync::oneshot;

use super::{ExtResult, parse_params};
use crate::agent::MvpAgent;
use crate::session::{SessionCommand, TodoCaptureError};

#[tracing::instrument(skip_all)]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TodoRequest {
        session_id: String,
        request: String,
        /// `/TODO`, not `/todo`. Defaulted so an older client keeps the
        /// ordinary, non-urgent capture it has always sent.
        #[serde(default)]
        urgent: bool,
        /// Names the client's task row for this capture, so progress updates
        /// reach it. An older client sends none and gets a minted one, which
        /// matches no row — its transcript then only lands in the run's file.
        #[serde(default)]
        capture_id: Option<String>,
    }

    let req: TodoRequest = parse_params(args)?;
    tracing::info!("handling /todo capture request");

    let sid: acp::SessionId = req.session_id.clone().into();
    let Some(session) = agent.resident_handle(&sid) else {
        return Err(
            acp::Error::invalid_params().data(format!("session not found: {}", req.session_id))
        );
    };

    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::TodoCapture {
        request: req.request,
        urgent: req.urgent,
        capture_id: req
            .capture_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        respond_to: tx,
    });
    let result = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;

    match result {
        Ok(outcome) => super::to_ext_response(Ok(serde_json::json!({
            "added": outcome.added,
            "toolsUsed": outcome.tools_used,
        }))),
        // Model errors take the canonical mapping so a rate limit keeps its
        // typed code and copy, same as `/btw`.
        Err(TodoCaptureError::Sampling(e)) => {
            Err(crate::sampling::error::map_sampling_err_to_acp(e))
        }
        // Everything else is already a readable sentence; `message` alone
        // keeps it out of the `Internal error: "…"` rendering that `data`
        // produces.
        Err(e) => Err(acp::Error::new(
            acp::ErrorCode::InternalError.into(),
            e.to_string(),
        )),
    }
}
