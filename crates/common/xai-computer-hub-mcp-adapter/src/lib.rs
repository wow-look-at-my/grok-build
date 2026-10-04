#![allow(clippy::cast_possible_wrap)]

//! Unified MCP adapter for the xAI Computer Hub.

#![forbid(unsafe_code)]

pub mod bridge;
pub(crate) mod metrics;
pub mod transport;
pub mod types;

pub use bridge::{McpBridge, McpBridgeConfig, McpBridgeHandle, McpToolHandler};
pub use transport::McpTransport;
pub use types::{McpCallResult, McpContent, McpError, McpServerInfo, McpToolDefinition};
