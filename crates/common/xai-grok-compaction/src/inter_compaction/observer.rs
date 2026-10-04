//! Observability seam for inter-compaction.

use std::time::Duration;

/// Receives inter-compaction pipeline events. All methods default to no-ops.
pub trait InterCompactionObserver: Send + Sync {
    /// A prior compaction summary was found in the input (re-compaction).
    fn on_recompaction(&self, _strategy: &'static str) {}

    /// One chunk's LLM call finished (success or error).
    fn on_chunk_sampled(&self, _success: bool, _elapsed: Duration) {}

    /// The whole pipeline finished assembling `num_chunks` chunk summaries.
    fn on_chunk_count(&self, _num_chunks: usize) {}
}

/// No-op observer for tests and harnesses without metrics.
impl InterCompactionObserver for () {}
