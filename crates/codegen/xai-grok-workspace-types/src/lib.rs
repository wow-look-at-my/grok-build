//! Wire types for the `xai-grok-workspace` API.

#![deny(clippy::indexing_slicing)]

pub mod binding;
pub mod chunks;
pub mod error;
pub mod events;
pub mod identity;
pub mod metadata;
pub mod request;
pub mod requests;
pub mod rpc;
pub mod types;

/// MCP tool name delimiter: server names are qualified as `"server__tool"`.
pub const MCP_TOOL_NAME_DELIMITER: &str = "__";

pub use crate::chunks::{ChunkKind, OpsChunk, SessionChunk, ToolChunk, ToolResponse};
pub use crate::error::{IoKind, WorkspaceError};
pub use crate::events::{EventLag, WorkspaceEvent, WorkspaceTopic, WorkspaceTopicSet};
pub use crate::identity::{HunkId, SessionId, ToolCallId};
pub use crate::metadata::{
    META_CLIENT_ID, META_GRPC_TIMEOUT, META_PROMPT_INDEX, META_SESSION_ID, META_TRACEPARENT,
    META_TRACESTATE, Metadata, STANDARD_META_KEYS,
};
pub use crate::request::RequestMessage;
pub use crate::requests::{
    SessionLifecycleRequest, ToolCallArgs, ToolRequest, WorkspaceOpsRequest, WorkspaceRequest,
};
pub use crate::types::{
    AgentSessionConfig, AgentSessionInfo, CapabilityMode, ContentMatch, FileReference, FsEventKind,
    FuzzyMatch, FuzzySearchArgs, GitBranchInfo, GitDiff, GitDiffArgs, GitMetadata, GitStatus,
    GitStatusOpts, HookInfo, Hunk, HunkAction, IsolationMode, LspServerStatus, McpServerStatus,
    MemoryChunk, PermissionDecision, PermissionPolicy, PermissionRequest, PlanModeDecision,
    PlanModeTransition, PluginInfo, ProjectConfig, ResolvedFile, RewindPoint, RewindResult,
    RipgrepArgs, RipgrepStats, SkillInfo, ToolCallResult, ToolDef, ToolOutputChunk, ToolProgress,
    ToolServerConfig, UserAnswer, UserQuestion, UserQuestionOption, VcsKind,
};
