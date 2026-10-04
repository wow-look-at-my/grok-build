//! grok-build's "code agent" compaction subsystem.

pub mod assemble;
pub mod compact;
pub mod config;
pub mod failure;
pub mod observer;
pub mod prompt;
pub mod sample;
pub mod summary;

pub use assemble::{CompactedHistoryParts, assemble_compacted_history};
pub use compact::{
    FullReplaceContext, FullReplaceError, FullReplaceOutput, FullReplaceSummary,
    apply_full_replace_compaction, sample_full_replace_summary,
};
pub use config::{
    DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT, FullReplaceConfig, MIN_SUMMARY_SEED_CHARS,
};
pub use failure::is_context_length_error;
pub use observer::{FullReplaceAttemptOutcome, FullReplaceObserver};
pub use prompt::{
    SELF_SUMMARIZATION_PROMPT, SummaryPromptKind, build_summary_prompt, build_summary_prompt_kind,
};
pub use sample::{SampleRetryError, SampledSummary, sample_summary_with_retries};
pub use summary::{
    format_compact_summary, format_compact_summary_content, is_degenerate_summary, wrap_user_query,
};
