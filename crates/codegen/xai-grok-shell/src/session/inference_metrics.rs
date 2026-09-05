//! Per-response inference latency metrics.
//!
//! This module is a thin re-export of the sampler crate's canonical
//! `InferenceLatencyStats` and `compute_percentiles` helpers. It
//! preserves the import path
//! `crate::session::inference_metrics::InferenceLatencyStats` for any
//! call-sites that still spell it that way.

pub(crate) use xai_grok_sampler::{InferenceLatencyStats, compute_percentiles};

/// Env var that adds the per-chunk arrival curve to `shell.turn.inference_done`.
pub(crate) const STREAM_TIMING_ENV: &str = "GROK_LOG_STREAM_TIMING";

/// True when the operator asked for per-chunk stream timing.
///
/// Read once. A session that starts without it keeps the cheap log for its
/// whole run, so a long stream cannot start paying for the offsets halfway
/// through and make one run's entries disagree with each other.
pub(crate) fn log_stream_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var(STREAM_TIMING_ENV).is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "on"))
    })
}
