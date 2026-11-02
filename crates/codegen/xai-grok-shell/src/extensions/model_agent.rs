//! `x.ai/model_agent` extension handler: spawn a background subagent on a model family.

use agent_client_protocol as acp;
use tokio::sync::oneshot;

use super::{ExtResult, parse_params};
use crate::agent::MvpAgent;
use crate::session::SessionCommand;

pub const MODEL_AGENT_METHOD: &str = "x.ai/model_agent";

#[tracing::instrument(skip_all)]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelAgentRequest {
        session_id: String,
        family: String,
        #[serde(default)]
        effort: Option<String>,
        prompt: String,
    }

    let req: ModelAgentRequest = parse_params(args)?;
    let sid: acp::SessionId = req.session_id.clone().into();
    let Some(session) = agent.resident_handle(&sid) else {
        return Err(
            acp::Error::invalid_params().data(format!("session not found: {}", req.session_id))
        );
    };
    let (tx, rx) = oneshot::channel();
    let _ = session.cmd_tx.send(SessionCommand::SpawnModelAgent {
        family: req.family,
        effort: req.effort,
        prompt: req.prompt,
        respond_to: tx,
    });
    let result = rx
        .await
        .map_err(|_| acp::Error::internal_error().data("session failed to respond"))?;
    match result {
        Ok(spawn) => super::to_ext_response(Ok(serde_json::json!({
            "subagentId": spawn.subagent_id,
            "modelId": spawn.model_id,
        }))),
        Err(message) => Err(acp::Error::new(
            acp::ErrorCode::InvalidParams.into(),
            message,
        )),
    }
}
