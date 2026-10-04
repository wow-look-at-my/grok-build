#![allow(clippy::cast_lossless)] // Hits predate the gate
#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)] // Hits predate the gate

//! Shared, transport-agnostic compaction engine.

pub mod code_compaction;
pub mod history;
pub mod inter_compaction;
pub mod intra_compaction;
pub mod item;
pub mod prompt;
pub mod reminder;
pub mod sampler;
pub mod select;
pub mod steps;
pub mod token;

/// Shared code default for the dedicated compaction model name.
pub use intra_compaction::DEFAULT_COMPACTION_MODEL_NAME;

// grok-build's full-replace subsystem now lives under `code_compaction`;
// re-exported at the crate root so consumers keep a stable public API.
pub use code_compaction::{
    CompactedHistoryParts, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT, FullReplaceAttemptOutcome,
    FullReplaceConfig, FullReplaceContext, FullReplaceError, FullReplaceObserver,
    FullReplaceOutput, FullReplaceSummary, MIN_SUMMARY_SEED_CHARS, SELF_SUMMARIZATION_PROMPT,
    SummaryPromptKind, apply_full_replace_compaction, assemble_compacted_history,
    build_summary_prompt, build_summary_prompt_kind, format_compact_summary,
    format_compact_summary_content, is_context_length_error, is_degenerate_summary,
    sample_full_replace_summary, wrap_user_query,
};
pub use item::{
    CompactionFileRef, CompactionItem, CompactionItemBuilder, CompactionItemFactory, CompactionRole,
};
pub use prompt::CompactionPrompt;
// Reminder types/formatters: import from `reminder::` (borrowed views).
pub use reminder::append_reminder_block;
pub use sampler::{CompactionSampleError, CompactionSampler, LlmCompactionOutput};
pub use select::{SplitPlan, select_turns_to_compact};
pub use steps::format_compaction_prompt;
pub use token::ItemTokenCounter;
