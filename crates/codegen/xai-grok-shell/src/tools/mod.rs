//! All tool execution goes through `xai-grok-tools` via the `ToolBridge`.

pub mod bridge;
pub mod config;
pub mod notification_bridge;
pub mod retry;
pub(crate) mod task_completed_frame;
pub mod todo;
pub mod tool_context;

pub use self::{
    config::{BashToolConfig, FileToolset, ShellToolsetConfig},
    retry::{RetryConfig, execute_with_retry},
    tool_context::ToolContext,
};

// Re-export key types from xai-grok-tools for convenience
pub use self::todo::{TodoId, TodoItem, TodoPriority, TodoStatus};
pub use xai_grok_tools::types::output::ToolOutput;
pub use xai_grok_tools::types::{MCPToolInput, ToolInput};
