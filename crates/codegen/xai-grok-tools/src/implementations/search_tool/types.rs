//! Types for the `search_tool`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input for the `search_tool` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchToolInput {
    /// Keywords to match against tool names, server names, and descriptions.
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: Option<u8>,
}

fn default_limit() -> Option<u8> {
    Some(5)
}
