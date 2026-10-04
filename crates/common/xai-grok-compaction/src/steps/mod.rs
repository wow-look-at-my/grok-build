//! Steps compaction — prompt content for compacting accumulated step turns (tool calls + assistant responses).

pub mod prompt;

pub use prompt::format_compaction_prompt;
