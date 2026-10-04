//! Observability seam for intra-compaction.

use std::time::Duration;

use super::traits::CompactionTarget;

/// Receives intra-compaction outcomes. All methods default to no-ops.
pub trait IntraCompactionObserver: Send + Sync {
    /// A pass ended in an error. `status` is the stable, low-cardinality label from [`super::error_status_label`].
    fn on_error(&self, _status: &'static str) {}

    /// A single pass succeeded (called once per successful pass — twice for
    /// a `HistoryThenSteps` run where both passes fire).
    fn on_success(
        &self,
        _target: CompactionTarget,
        _tokens_before: u32,
        _tokens_after: u32,
        _turns_compacted: u32,
        _elapsed: Duration,
    ) {
    }
}

/// No-op observer for tests and harnesses without metrics.
impl IntraCompactionObserver for () {}
