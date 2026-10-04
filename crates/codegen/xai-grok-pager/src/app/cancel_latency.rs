//! User-cancel latency measurement.
use std::time::Instant;
use xai_grok_telemetry::events::CancellationScope;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CancelOrigin {
    UserGesture,
    #[allow(dead_code)]
    Programmatic,
}
/// How a turn ended, which decides whether a pending user-cancel anchor is
/// measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TurnEnd {
    Completed,
    Aborted,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct CancelLatency {
    pub(crate) requested_at: Instant,
    pub(crate) scope: CancellationScope,
}
impl CancelLatency {
    pub(crate) fn new(requested_at: Instant, scope: CancellationScope) -> Self {
        Self {
            requested_at,
            scope,
        }
    }
}
