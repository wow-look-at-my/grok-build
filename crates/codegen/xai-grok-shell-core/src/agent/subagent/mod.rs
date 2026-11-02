//! Subagent state that the session layer reads: the attempt store, receipts
//! and resume windows.
pub mod attempt_store;
pub mod child_runtime;
mod meta;
pub use meta::{SubagentMeta, SubagentSessionMetadata, cancellation_error_message};
pub mod prompt_turn_receipt;
pub mod prompt_turn_result;
pub mod resume_window;

pub use prompt_turn_receipt::PromptTurnReceipt;

/// Clamp for a resolved sampling-limit override; `Semaphore::new` panics past `Semaphore::MAX_PERMITS`.
pub const MAX_SUBAGENT_SAMPLING_LIMIT: usize = 512;
