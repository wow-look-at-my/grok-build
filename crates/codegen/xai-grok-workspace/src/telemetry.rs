//! Stable `tracing` target for workspace telemetry events.

pub(crate) const TELEMETRY_TARGET: &str = "workspace::telemetry";

/// Emit a telemetry `tracing` event on [`TELEMETRY_TARGET`], pinned so a call
/// site cannot land elsewhere.
macro_rules! dc_log {
    ($level:ident, $($rest:tt)*) => {
        ::tracing::$level!(target: $crate::telemetry::TELEMETRY_TARGET, $($rest)*)
    };
}
pub(crate) use dc_log;
