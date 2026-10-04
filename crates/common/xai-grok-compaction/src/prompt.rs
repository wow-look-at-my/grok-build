//! The shared compaction prompt seam.

/// System + user prompt pair for the compaction LLM call.
#[derive(Debug, Clone)]
pub struct CompactionPrompt {
    pub system: String,
    pub user: String,
}
