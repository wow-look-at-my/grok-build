//! Input/output types for memory tools.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Input for the `memory_search` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchInput {
    /// The search query string. Use specific technical terms rather than conversational language.
    pub query: String,
    /// Maximum number of results to return.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Minimum relevance score threshold.
    #[serde(default)]
    pub min_score: Option<f64>,
}

/// Output schema for `memory_search` (used for JSON Schema generation only).
#[derive(Debug, JsonSchema)]
pub struct MemorySearchOutput {
    /// Formatted search results as markdown text.
    pub results: String,
}

/// Input for the `memory_get` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryGetInput {
    /// Path to the memory file to read.
    pub path: String,
    /// 1-based start line, matching the line numbers in the tool's output (default: beginning of file).
    #[serde(default)]
    pub from: Option<usize>,
    /// Maximum number of lines to return (default: all).
    #[serde(default)]
    pub lines: Option<usize>,
}

/// Output schema for `memory_get` (used for JSON Schema generation only).
#[derive(Debug, JsonSchema)]
pub struct MemoryGetOutput {
    /// File content (optionally line-limited).
    pub content: String,
}
