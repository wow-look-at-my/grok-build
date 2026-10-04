#[derive(Debug, Clone)]
pub struct CompactionPolicy {
    /// Auto-compaction triggers when this percentage of the context window is used.
    pub auto_compact_threshold_percent: u32,

    /// Model to use for generating the compaction summary. None means use the session's current model.
    pub compact_model: Option<String>,

    /// Whether to run a memory flush turn before each compaction.
    pub memory_flush_enabled: bool,

    /// Per-compaction wall-clock budget (seconds); a generation exceeding it is cut and retried.
    pub wall_clock_budget_secs: u64,

    pub two_pass_enabled: bool,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            auto_compact_threshold_percent: 85,
            compact_model: None,
            memory_flush_enabled: false,
            wall_clock_budget_secs: 300,
            two_pass_enabled: false,
        }
    }
}
