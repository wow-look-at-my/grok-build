//! Token usage and provider-reported cost of one model call, read off its raw stream.
//!
//! The stream transforms and the shell's compaction loops both read usage through these helpers, so both agree.

use xai_grok_sampling_types::messages::{MessageDeltaUsage, MessagesUsage};
use xai_grok_sampling_types::ollama::OllamaChatChunk;
use xai_grok_sampling_types::{TokenUsage, Usage, UsageCost, rs};

/// The usage and the provider-reported cost of one model call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallUsage {
    pub usage: Option<TokenUsage>,
    /// The price the provider put on the call, in USD ticks. `None` when the provider reported none.
    pub cost_usd_ticks: Option<i64>,
}

/// The price a provider put on a call, in USD ticks.
/// Integer ticks win over the USD float, and a reported zero is unbilled.
/// A float with no `i64` tick form is logged and treated as unpriced.
pub fn wire_cost_ticks(ticks: Option<i64>, cost: Option<&UsageCost>) -> Option<i64> {
    xai_grok_sampling_types::reported_cost_ticks(ticks).or_else(|| {
        xai_grok_sampling_types::usd_float_to_ticks(cost.map(UsageCost::as_usd_float))
            .unwrap_or_else(|err| {
                tracing::error!(
                    error = %err,
                    "provider reported a usage cost with no tick form; leaving the call unpriced"
                );
                None
            })
    })
}

impl CallUsage {
    /// Fold one Chat Completions `usage` object.
    /// The wire values are cumulative for the response, so the last one wins.
    /// A later object with no cost keeps the earlier cost.
    pub fn observe_chat_completions(&mut self, usage: &Usage) {
        if let Some(ticks) = wire_cost_ticks(usage.cost_in_usd_ticks, usage.cost.as_ref()) {
            self.cost_usd_ticks = Some(ticks);
        }
        self.usage = Some(usage.clone().into());
    }

    /// Usage and cost of the terminal `response` of a Responses stream.
    /// This removes the cost ticks that the client keeps in the response metadata.
    pub fn from_responses(response: &mut rs::Response) -> Self {
        let call = Self::of_responses(response);
        if let Some(metadata) = response.metadata.as_mut() {
            metadata.remove(crate::client::COST_USD_TICKS_METADATA_KEY);
        }
        call
    }

    /// [`Self::from_responses`] that leaves the response as it is.
    pub fn of_responses(response: &rs::Response) -> Self {
        // The SSE decoder already set `total_tokens` to the live context length; the billing fields are the wire values.
        let usage = response.usage.as_ref().map(|u| TokenUsage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
            reasoning_tokens: u.output_tokens_details.reasoning_tokens,
            cached_prompt_tokens: u.input_tokens_details.cached_tokens,
            cache_creation_prompt_tokens: 0,
        });
        let cost_usd_ticks = response
            .metadata
            .as_ref()
            .and_then(|m| m.get(crate::client::COST_USD_TICKS_METADATA_KEY))
            .and_then(|s| s.parse::<i64>().ok());
        Self {
            usage,
            cost_usd_ticks,
        }
    }

    /// Usage of the Ollama `/api/chat` line that has `done: true`.
    pub fn from_ollama_done(chunk: &OllamaChatChunk) -> Self {
        let prompt_tokens = chunk.prompt_eval_count.unwrap_or(0);
        let completion_tokens = chunk.eval_count.unwrap_or(0);
        let usage = (prompt_tokens > 0 || completion_tokens > 0).then(|| TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
            reasoning_tokens: 0,
            // Ollama counts the prefix its runner reused, which is the same quantity a cache-read is elsewhere.
            cached_prompt_tokens: chunk.prompt_eval_cached_count.unwrap_or(0),
            cache_creation_prompt_tokens: 0,
        });
        Self {
            usage,
            cost_usd_ticks: None,
        }
    }
}

/// Accumulates the usage of a Messages stream from `message_start` and every `message_delta`.
#[derive(Debug, Clone, Default)]
pub struct MessagesUsageAccumulator {
    /// The uncached part of the prompt. Cache reads and writes are separate buckets.
    input_tokens: u32,
    cache_read_input_tokens: u32,
    cache_creation_input_tokens: u32,
    output_tokens: u32,
    cost_usd_ticks: Option<i64>,
}

impl MessagesUsageAccumulator {
    pub fn observe_start(&mut self, usage: &MessagesUsage) {
        self.input_tokens = usage.input_tokens;
        self.cache_read_input_tokens = usage.cache_read_input_tokens;
        self.cache_creation_input_tokens = usage.cache_creation_input_tokens;
        self.observe_cost(usage.cost_in_usd_ticks, usage.cost.as_ref());
    }

    /// The delta counts are cumulative. A field the delta omits keeps its `message_start` value.
    pub fn observe_delta(&mut self, usage: &MessageDeltaUsage) {
        self.output_tokens = usage.output_tokens;
        if let Some(input) = usage.input_tokens {
            self.input_tokens = input;
        }
        if let Some(cache_read) = usage.cache_read_input_tokens {
            self.cache_read_input_tokens = cache_read;
        }
        if let Some(cache_creation) = usage.cache_creation_input_tokens {
            self.cache_creation_input_tokens = cache_creation;
        }
        self.observe_cost(usage.cost_in_usd_ticks, usage.cost.as_ref());
    }

    /// The usage of a whole non-streaming Messages response.
    pub fn observe_response(&mut self, usage: &MessagesUsage) {
        self.observe_start(usage);
        self.output_tokens = usage.output_tokens;
    }

    /// A later event with no cost never erases an earlier reported cost.
    fn observe_cost(&mut self, ticks: Option<i64>, cost: Option<&UsageCost>) {
        self.cost_usd_ticks = wire_cost_ticks(ticks, cost).or(self.cost_usd_ticks);
    }

    /// `prompt_tokens` holds the full prompt and `cached_prompt_tokens` the cache hits, as on the other backends.
    pub fn finish(&self) -> CallUsage {
        let prompt_tokens = self
            .input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_input_tokens);
        let usage = (prompt_tokens > 0 || self.output_tokens > 0).then(|| TokenUsage {
            prompt_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: prompt_tokens.saturating_add(self.output_tokens),
            reasoning_tokens: 0,
            cached_prompt_tokens: self.cache_read_input_tokens,
            cache_creation_prompt_tokens: self.cache_creation_input_tokens,
        });
        CallUsage {
            usage,
            cost_usd_ticks: self.cost_usd_ticks,
        }
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
