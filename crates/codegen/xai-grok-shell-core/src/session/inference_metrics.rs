//! This module preserves the import path `crate::session::inference_metrics::InferenceLatencyStats` for call-sites that still spell it that way.

pub use xai_grok_sampler::{InferenceLatencyStats, compute_percentiles};

/// Env var that adds the per-chunk arrival curve to `shell.turn.inference_done`.
pub const STREAM_TIMING_ENV: &str = "GROK_LOG_STREAM_TIMING";

/// True when the operator asked for per-chunk stream timing. Read once.
pub fn log_stream_timing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var(STREAM_TIMING_ENV).is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "on"))
    })
}
