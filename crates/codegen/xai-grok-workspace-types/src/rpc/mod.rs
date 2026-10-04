//! Canonical wire types for hub-proxied `workspace.*` RPC methods.

use serde::Serialize;
use serde::de::DeserializeOwned;

pub mod agents_md;
pub mod code_nav;
pub mod envelope;
pub mod export;
pub mod export_github;
pub mod fs;
pub mod git;
pub mod hooks;
pub mod hunks;
pub mod presence;
pub mod repos;
pub mod search;
pub mod session;
pub mod skills;
pub mod workspace;
pub mod worktree;

pub use envelope::{RpcEnvelope, RpcError};

/// Tool ID for the `WorkspaceRpcHandler` (workspace method dispatch).
pub const WORKSPACE_RPC_TOOL_ID: &str = "workspace_rpc";

/// Tool ID used for `WorkspaceEvent` notification frames.
pub const WORKSPACE_EVENTS_TOOL_ID: &str = "workspace_events";

/// Tool ID used for `ToolNotification` forwarding frames.
pub const WORKSPACE_TOOL_NOTIFICATIONS_TOOL_ID: &str = "workspace_tool_notifications";

/// Tool ID used for workspace-originated client ext-notification frames (e.g. `x.ai/search/fuzzy/status`).
pub const WORKSPACE_CLIENT_EXT_NOTIFICATIONS_TOOL_ID: &str = "workspace_client_ext_notifications";

/// What a workspace RPC says about human presence, for idle-hibernation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcActivityClass {
    /// A client-driven write that counts as workspace activity.
    Mutation,
    /// Reads, polls, and non-activity mutations, which never count.
    Read,
}

/// Marker trait for typed workspace RPC requests. Client and server use the
/// same struct for the same method.
pub trait WorkspaceRpc: Serialize {
    /// Wire method name (e.g. `"workspace.git_status_ext"`).
    const METHOD: &'static str;
    /// Whether executing this method counts as human activity for idle hibernation. No default, so every method is classified explicitly.
    const ACTIVITY: RpcActivityClass;
    type Response: Serialize + DeserializeOwned + Send;
}
