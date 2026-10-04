//! Telemetry hook trait for [`crate::CircuitBreaker`].

use crate::state::{BreakerState, Outcome};

/// Telemetry hooks. Default implementations are no-ops; consumers
/// implement only the methods they care about.
pub trait Observer: Send + Sync {
    /// Called when the breaker transitions between states.
    fn on_state_change(&self, _old: BreakerState, _new: BreakerState, _reason: &str) {}

    /// Called from `check()` when the breaker is `HalfOpen` and a caller attempts to claim a probe slot.
    fn on_probe_admission(&self, _allowed: bool) {}

    /// Called from `record()` after the sample is added to the window and any resulting state transition has landed.
    fn on_outcome(&self, _outcome: Outcome, _status: BreakerState) {}
}

/// No-op observer used by [`crate::CircuitBreaker::new`].
#[derive(Debug, Default)]
pub struct NoopObserver;

impl Observer for NoopObserver {}
