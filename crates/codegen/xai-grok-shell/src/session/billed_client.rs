//! The way shell code builds a [`SamplingClient`], and the sink that bills its calls.

use std::sync::Arc;

use xai_grok_sampler::{BilledCall, SamplerConfig, SamplingClient, SharedUsageSink, UsageSink};
use xai_grok_sampling_types::{SamplingError, TokenUsage};

/// The refusal for a config that names no usage sink.
pub(crate) const MISSING_USAGE_SINK: &str =
    "this sampler config has no usage sink, so its calls would bill nowhere";

/// Build a client for `cfg`. A config with no usage sink is an error.
pub(crate) fn billed_sampling_client(cfg: SamplerConfig) -> Result<SamplingClient, SamplingError> {
    if cfg.usage_sink.is_none() {
        tracing::error!(model = %cfg.model, base_url = %cfg.base_url, "{MISSING_USAGE_SINK}");
        return Err(SamplingError::InvalidConfiguration(MISSING_USAGE_SINK));
    }
    SamplingClient::new(cfg)
}

/// The price of one call in USD ticks: the provider-reported cost, else `usage` at the rates
/// `model_pricing::resolve` finds for `model`. The sink and the per-response price both use it.
pub(crate) fn price_model_call(
    model: &str,
    usage: Option<&TokenUsage>,
    reported_cost_ticks: Option<i64>,
) -> Option<i64> {
    reported_cost_ticks.or_else(|| {
        let pricing = crate::agent::model_pricing::resolve(model);
        xai_grok_sampling_types::compute_cost_ticks(usage, &pricing)
    })
}

/// Folds every call a session's clients send into that session's prompt and session ledgers.
#[derive(Debug)]
pub(crate) struct SessionUsageSink {
    chat_state: xai_chat_state::ChatStateHandle,
}

impl UsageSink for SessionUsageSink {
    fn record_call(&self, call: BilledCall) {
        let BilledCall {
            model,
            usage,
            cost_usd_ticks,
            duration,
        } = call;
        let cost = price_model_call(&model, usage.as_ref(), cost_usd_ticks);
        self.chat_state.record_side_call_usage(
            Some(model),
            usage.unwrap_or_default(),
            Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
            cost,
        );
    }
}

/// The sink of the session that owns `chat_state`.
pub(crate) fn session_usage_sink(chat_state: &xai_chat_state::ChatStateHandle) -> SharedUsageSink {
    Arc::new(SessionUsageSink {
        chat_state: chat_state.clone(),
    })
}

#[cfg(test)]
#[path = "billed_client_tests.rs"]
mod tests;
