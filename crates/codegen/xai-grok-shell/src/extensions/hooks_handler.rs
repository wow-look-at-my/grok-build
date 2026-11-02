//! The `x.ai/hooks/*` handlers. The client-hook types live in `xai_grok_shell_core::extensions::hooks`.
use agent_client_protocol as acp;
use serde::Deserialize;

pub use xai_grok_shell_core::extensions::hooks::*;

use crate::agent::MvpAgent;

type ExtResult = Result<acp::ExtResponse, acp::Error>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRequest {
    session_id: String,
}

pub async fn handle(agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        "x.ai/hooks/list" => {
            let req: ListRequest = super::parse_params(args)?;
            let sid = acp::SessionId::new(req.session_id);

            let result = agent
                .list_hooks(&sid)
                .await
                .ok_or_else(|| anyhow::anyhow!("session not found"));
            super::to_ext_response(result)
        }
        "x.ai/hooks/action" => {
            let req: xai_hooks_plugins_types::HooksActionRequest = super::parse_params(args)?;
            let sid = acp::SessionId::new(req.session_id);

            let result = agent
                .execute_hooks_action(&sid, req.action)
                .await
                .ok_or_else(|| anyhow::anyhow!("session not found"));
            super::to_ext_response(result)
        }
        _ => Err(acp::Error::method_not_found()),
    }
}
