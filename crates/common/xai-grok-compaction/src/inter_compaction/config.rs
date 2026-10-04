//! Configuration for inter-compaction.

use serde::{Deserialize, Serialize};

use crate::history::types::CompactionStrategy;

/// Runtime configuration for a single inter-compaction invocation.
///
/// Mirrors the fields used by the between-turn compaction service config,
/// without a harness-specific config-macro dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterCompactionConfig {
    /// The agent/scheduler name to use for the compaction model.
    pub compaction_model_name: String,
    /// End-to-end timeout for the compaction sampling in seconds.
    pub sampling_timeout_secs: u64,
    /// Which compaction strategy to use.
    pub compaction_strategy: CompactionStrategy,
    /// [DivideAndConquer] Max tokens per chunk before sending to the LLM.
    pub dnc_chunk_token_limit: u32,
    /// User messages with character count > this threshold are truncated (middle-cut).
    pub user_message_compact_threshold: u32,
}
