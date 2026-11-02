//! MCP extension wire types, method names and the org-policy refusal. `xai_grok_shell::extensions::mcp` holds the handlers.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use agent_client_protocol::{self as acp, Client};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as TokioMutex;
// rmcp is quarantined in xai-grok-mcp; see that crate's docs.
use xai_grok_mcp::rmcp;
// `wire::MCP_CALL` is the one cross-SDK contract literal; the agent-only siblings live in `mcp_methods` below
use xai_grok_mcp::wire;

/// Agent-only `x.ai/mcp/*` ACP method/notification names. Unlike [`wire::MCP_CALL`] (the cross-SDK contract, which stays in `xai_grok_mcp::wire`), these methods are NOT spoken by the SDK.
/// They are private to the channel between the agent and the client. They are centralized here only to avoid scattering the same string literal across dispatch and notification send sites.
pub mod mcp_methods {
    /// Shared prefix that routes every MCP ext method to this module's dispatcher.
    pub const PREFIX: &str = "x.ai/mcp/";

    pub const LIST: &str = "x.ai/mcp/list";
    pub const READ_RESOURCE: &str = "x.ai/mcp/read_resource";
    pub const AUTH_STATUS: &str = "x.ai/mcp/auth_status";
    pub const AUTH_TRIGGER: &str = "x.ai/mcp/auth_trigger";
    pub const SETUP: &str = "x.ai/mcp/setup";
    pub const TOGGLE: &str = "x.ai/mcp/toggle";
    pub const TOGGLE_TOOL: &str = "x.ai/mcp/toggle_tool";
    pub const UPSERT: &str = "x.ai/mcp/upsert";
    pub const DELETE: &str = "x.ai/mcp/delete";

    pub const SERVERS_UPDATED: &str = "x.ai/mcp/servers_updated";
    pub const TOOLS_CHANGED: &str = "x.ai/mcp/tools_changed";
    pub const INIT_PROGRESS: &str = "x.ai/mcp/init_progress";
}
use crate::session::mcp_servers::{MCP_TOOL_NAME_DELIMITER, McpClient, McpState, SharedMcpState};

// ── Wire types: mcp/list ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    /// When false, bypass cache and refetch from cli-chat-proxy, then sync into live sessions.
    #[serde(default = "default_true")]
    pub cache: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListResponse {
    pub servers: Vec<McpServerEntry>,
    /// Session-scoped: true when `is_initialized` is set. A missing client is then a failed handshake.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_mcp_resolved: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<xai_grok_mcp::servers::McpIcon>,
    pub source: McpServerSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<crate::util::config::McpSetupConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_values: Option<HashMap<String, String>>,
    #[serde(flatten)]
    pub config: McpServerConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<McpServerSessionState>,
}

/// MCP server config for the `mcp/list` catalog response.
/// Distinct from `acp::McpServer` (session/new input) because: HTTP: exposes `scope`, `scope_id`, and `scope_name` for connector selection, NOT headers (auth tokens stay private)
/// Stdio: same structure but optimized for JSON wire format
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpServerConfig {
    #[serde(rename = "http")]
    Http {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(rename = "scopeId", skip_serializing_if = "Option::is_none")]
        scope_id: Option<String>,
        #[serde(rename = "scopeName", skip_serializing_if = "Option::is_none")]
        scope_name: Option<String>,
    },
    #[serde(rename = "stdio")]
    Stdio {
        command: std::path::PathBuf,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env: Vec<McpEnvVar>,
    },
    #[serde(rename = "managedGateway")]
    ManagedGateway,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpEnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpServerSource {
    Managed,
    Local,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSessionState {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<McpSessionStatus>,
    /// Why the server is unavailable, when the session knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpToolEntry>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auth_required: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub setup_required: bool,
    /// Managed-policy verdict for a server the merge dropped, so `/mcps` can say "blocked by policy" instead of a generic "unavailable".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpSessionStatus {
    Ready,
    Initializing,
    SetupRequired,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icons: Vec<xai_grok_mcp::servers::McpIcon>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

// ── Wire types: mcp/call ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallRequest {
    /// When present: session pool. When absent: agent pool (config.toml only).
    #[serde(default)]
    pub session_id: Option<String>,
    pub server: String,
    /// Endpoint URL; disambiguates when multiple servers share a name.
    #[serde(default)]
    pub server_url: Option<String>,
    pub tool: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallResponse {
    pub content: Vec<McpContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

// ── Internal types (not serialized to wire) ─────────────────────────

#[derive(Debug, Clone, Default)]
pub struct McpStatusSnapshot {
    pub configs: Vec<acp::McpServer>,
    pub clients: Vec<McpClientStatus>,
    pub auth_required: std::collections::HashSet<String>,
    /// Mirrors `McpState::is_initialized`. A missing client then means handshake failed.
    pub is_resolved: bool,
}

#[derive(Debug, Clone)]
pub struct McpClientStatus {
    pub name: String,
    pub status: McpSessionStatus,
    pub tools: Vec<McpToolEntry>,
    /// Why an `Unavailable` server is unavailable, as recorded in [`McpState::init_failed`].
    pub error: Option<String>,
    pub icons: Vec<xai_grok_mcp::servers::McpIcon>,
}

// ── Notification: mcp/servers_updated ────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServersUpdated {
    pub mcp_servers: Vec<McpServerEntry>,
}

/// Per-server tool-list change push. The `session_id` field lets the pager
/// route the push to the owning agent via `find_session_match`. Falling back
/// to `app.active_view` was a latent multi-agent bug.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsChanged {
    /// Session that owns this push.
    pub session_id: String,
    /// MCP server whose tool list changed. unread by the pager.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub server_name: String,
    /// New tool entries for the named server. unread by the pager for the same reason as `server_name` above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpToolEntry>,
}

// Re-export the `x.ai/mcp/server_status` schema and method constant from the
// dispatcher module External callers then have a single import point
// alongside the other `x.ai/mcp/*` types.
pub use crate::session::mcp_dispatcher::{
    McpServerStatus, McpServerStatusPayload, McpServerStatusReason, SERVER_STATUS_METHOD,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReadResourceRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    pub server: String,
    pub uri: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReadResourceResponse {
    pub contents: Vec<McpReadResourceContent>,
}

/// A single resource content block from `resources/read`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpReadResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

pub const MANAGED_GATEWAY_ENTRY_PREFIX: &str = "managed_gateway:";
#[derive(serde::Serialize)]
pub struct McpAuthStatusEntry {
    pub server_name: String,
    pub status: &'static str,
}
/// The org-policy refusal for enabling or adding a server (one wording for both).
pub fn org_policy_message(
    name: &str,
    reason: &xai_grok_workspace::permission::resolution::McpBlockReason,
) -> String {
    // The name is a case-sensitive identifier (long ones middle-truncate).
    let path = reason.user_facing_source();
    format!(
        "The server {} is blocked by an organization policy ({path}).",
        clamped_server_name(name)
    )
}

/// Server names are unbounded user input; middle-truncate long ones so the
/// reason clause survives the pager's ~200-char error truncation. Short names
/// print verbatim.
pub fn clamped_server_name(name: &str) -> std::borrow::Cow<'_, str> {
    const MAX_CHARS: usize = 40;
    if name.chars().count() <= MAX_CHARS {
        return name.into();
    }
    let head: String = name.chars().take(MAX_CHARS / 2).collect();
    let tail_rev: Vec<char> = name.chars().rev().take(MAX_CHARS / 2 - 1).collect();
    let tail: String = tail_rev.into_iter().rev().collect();
    format!("{head}…{tail}").into()
}

/// User-facing refusal for enabling/spawning a policy-blocked server (`None`
/// when it passes) — the chokepoint for the toggle, setup, and upsert
/// paths.
pub fn policy_enable_error(
    ms: &xai_grok_workspace::permission::resolution::ManagedSettings,
    server: &acp::McpServer,
    subject: xai_grok_workspace::permission::resolution::McpSubject,
) -> Option<String> {
    match ms.mcp_verdict(server, subject) {
        xai_grok_workspace::permission::resolution::McpVerdict::Allowed => None,
        xai_grok_workspace::permission::resolution::McpVerdict::Blocked(reason) => {
            Some(org_policy_message(
                crate::session::mcp_servers::mcp_server_name(server),
                &reason,
            ))
        }
    }
}
