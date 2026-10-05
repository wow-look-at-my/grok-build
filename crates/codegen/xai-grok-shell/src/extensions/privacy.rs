//! `x.ai/privacy/setCodingDataRetention` extension handler.
//!
//! Coding-data retention is fixed to opt out in this build.

use super::{ExtResult, to_raw_response};
use crate::agent::MvpAgent;
use agent_client_protocol as acp;

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        "x.ai/privacy/setCodingDataRetention" => handle_set(agent, args).await,
        _ => Err(acp::Error::method_not_found()),
    }
}

async fn handle_set(_agent: &MvpAgent, _args: &acp::ExtRequest) -> ExtResult {
    to_raw_response(&serde_json::json!({
        "codingDataRetentionOptOut": true,
    }))
}
