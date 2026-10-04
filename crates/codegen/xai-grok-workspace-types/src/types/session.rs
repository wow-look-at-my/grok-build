//! Session-related shapes referenced from `SessionChunk` and `WorkspaceEvent`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::identity::SessionId;
use crate::types::config::IsolationMode;

/// Snapshot of a session emitted by `SessionChunk::SessionInfo` (one per session in the response stream of `SessionLifecycleRequest::List`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionInfo {
    pub id: SessionId,
    /// Optional parent session id (set on subagents).
    #[serde(default)]
    pub parent: Option<SessionId>,
    /// Agent identifier (e.g. `"main"`, `"subagent-explore"`).
    #[serde(default)]
    pub agent_id: String,
    /// Filesystem isolation mode.
    #[serde(default)]
    pub isolation: IsolationMode,
    /// Wall-clock creation time.
    #[serde(default)]
    pub created_at: DateTime<Utc>,
}

/// Result of a `SessionLifecycleRequest::Rewind`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindResult {
    /// Session id rewound.
    pub session: SessionId,
    /// Prompt index that is now the head of the conversation.
    #[serde(default)]
    pub head_prompt_index: u64,
    /// Number of prompts dropped by the rewind.
    #[serde(default)]
    pub prompts_dropped: u64,
}

/// One rewind point returned in `SessionChunk::RewindPoints`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindPoint {
    /// Prompt index (monotonically increasing per session).
    pub prompt_index: u64,
    /// Wall-clock time the prompt was started.
    #[serde(default)]
    pub at: DateTime<Utc>,
    /// Optional summary of the prompt that occurred at this index.
    #[serde(default)]
    pub summary: String,
}

/// Filesystem event kind reported by `WorkspaceEvent::FsChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsEventKind {
    /// File or directory created.
    Created,
    /// File modified (content or metadata).
    #[default]
    Modified,
    /// File or directory removed.
    Removed,
    /// File renamed (path is the new path).
    Renamed,
}

/// Generic server-status enum used for both MCP and LSP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    /// Server is starting.
    Starting,
    /// Server is running normally.
    #[default]
    Running,
    /// Server stopped (clean shutdown).
    Stopped,
    /// Server failed (returns to caller via the event payload).
    Failed,
}

/// MCP server status reported by `WorkspaceEvent::McpServerStateChanged`.
pub type McpServerStatus = ServerStatus;

/// LSP server status reported by `WorkspaceEvent::LspServerStateChanged`.
pub type LspServerStatus = ServerStatus;
