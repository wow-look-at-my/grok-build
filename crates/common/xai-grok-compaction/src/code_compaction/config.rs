//! grok-build compaction configuration.

/// Default auto-compact threshold (% of context window) when no other source (env var, user config, remote per-model/global flags).
pub const DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT: u8 = 85;

/// Minimum character count for a cleaned summary seed. grok-build retries when the cleaned summary is shorter than this.
pub const MIN_SUMMARY_SEED_CHARS: usize = 500;

/// Tunables for the full-replace pass.
#[derive(Debug, Clone)]
pub struct FullReplaceConfig {
    /// Total LLM attempts (first try + retries) on transient failures.
    pub max_attempts: u32,
    /// Delay between transient retries.
    pub retry_delay_secs: u64,
    /// End-to-end timeout for each compaction LLM call.
    pub sampling_timeout_secs: u64,
}

impl Default for FullReplaceConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retry_delay_secs: 3,
            sampling_timeout_secs: 120,
        }
    }
}
