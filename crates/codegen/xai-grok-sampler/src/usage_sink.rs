//! Every model request a [`crate::SamplingClient`] sends reports its usage here, once.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::stream::{BoxStream, Stream, StreamExt};
use xai_grok_sampling_types::ollama::OllamaChatChunk;
use xai_grok_sampling_types::{ChatCompletionChunk, Result, TokenUsage, UsageCost, messages, rs};

use crate::stream::usage::{CallUsage, MessagesUsageAccumulator};

/// One HTTP call to a model provider, as the client saw it end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilledCall {
    /// The model the provider named in its response, else the model the request named.
    pub model: String,
    pub usage: Option<TokenUsage>,
    /// The price the provider put on the call, in USD ticks. `None` when the provider reported none.
    pub cost_usd_ticks: Option<i64>,
    /// Time from the slot grant to the end of the response.
    pub duration: Duration,
}

/// Receives the usage of every call a client sends. It must be cheap and must not block.
pub trait UsageSink: Send + Sync + std::fmt::Debug {
    fn record_call(&self, call: BilledCall);
}

pub type SharedUsageSink = Arc<dyn UsageSink>;

/// The sink of a call that belongs to no session. The reason names the caller in the log.
#[derive(Debug)]
struct NoSessionSink {
    reason: &'static str,
}

impl UsageSink for NoSessionSink {
    fn record_call(&self, call: BilledCall) {
        tracing::debug!(
            caller = self.reason,
            model = %call.model,
            cost_usd_ticks = ?call.cost_usd_ticks,
            "model call outside a session; no session ledger records it"
        );
    }
}

/// A sink for a caller that has no session ledger. Use it only where no session exists.
pub fn no_session_sink(reason: &'static str) -> SharedUsageSink {
    Arc::new(NoSessionSink { reason })
}

/// The usage of one call while its response arrives. It reports once: at the end, on the
/// terminal item, or when it is dropped.
pub(crate) struct CallMeter {
    sink: Option<SharedUsageSink>,
    request_model: String,
    reported_model: Option<String>,
    usage: CallUsage,
    messages: MessagesUsageAccumulator,
    started: Instant,
    reported: bool,
}

impl CallMeter {
    pub(crate) fn new(sink: Option<SharedUsageSink>, request_model: String) -> Self {
        Self {
            sink,
            request_model,
            reported_model: None,
            usage: CallUsage::default(),
            messages: MessagesUsageAccumulator::default(),
            started: Instant::now(),
            reported: false,
        }
    }

    fn note_model(&mut self, model: &str) {
        if self.reported_model.is_none() && !model.is_empty() {
            self.reported_model = Some(model.to_owned());
        }
    }

    /// Report what arrived. A call with neither usage nor cost reports nothing.
    pub(crate) fn finish(&mut self) {
        if std::mem::replace(&mut self.reported, true) {
            return;
        }
        let Some(sink) = self.sink.as_ref() else {
            return;
        };
        let CallUsage {
            usage,
            cost_usd_ticks,
        } = std::mem::take(&mut self.usage);
        if usage.is_none() && cost_usd_ticks.is_none() {
            return;
        }
        let model = self
            .reported_model
            .take()
            .unwrap_or_else(|| std::mem::take(&mut self.request_model));
        sink.record_call(BilledCall {
            model,
            usage,
            cost_usd_ticks,
            duration: self.started.elapsed(),
        });
    }

    pub(crate) fn observe_chat_response(
        &mut self,
        response: &xai_grok_sampling_types::ChatCompletionResponse,
    ) {
        self.note_model(&response.model);
        if let Some(usage) = response.usage.as_ref() {
            self.usage.observe_chat_completions(usage);
        }
    }

    pub(crate) fn observe_responses_response(&mut self, response: &rs::Response) {
        self.note_model(&response.model);
        self.usage = CallUsage::of_responses(response);
    }

    /// Read the `model` and `usage` of a JSON body from an endpoint that is not a chat API.
    /// Token counts and a reported cost are kept. A body without them leaves the meter empty.
    pub(crate) fn observe_endpoint_body(&mut self, body: &[u8]) {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return;
        };
        if let Some(model) = value.get("model").and_then(serde_json::Value::as_str) {
            self.note_model(model);
        }
        let Some(usage) = value
            .get("usage")
            .and_then(|u| serde_json::from_value::<EndpointUsage>(u.clone()).ok())
        else {
            return;
        };
        let counted = usage.prompt_tokens > 0 || usage.completion_tokens > 0;
        let tokens = counted.then(|| TokenUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage
                .total_tokens
                .max(usage.prompt_tokens.saturating_add(usage.completion_tokens)),
            reasoning_tokens: 0,
            cached_prompt_tokens: 0,
            cache_creation_prompt_tokens: 0,
        });
        self.usage = CallUsage {
            usage: tokens,
            cost_usd_ticks: crate::stream::usage::wire_cost_ticks(
                usage.cost_in_usd_ticks,
                usage.cost.as_ref(),
            ),
        };
    }

    pub(crate) fn observe_messages_response(&mut self, response: &messages::MessagesResponse) {
        self.note_model(&response.model);
        self.messages.observe_response(&response.usage);
        self.usage = self.messages.finish();
    }
}

/// The `usage` object of an endpoint body. Every field is optional: an embeddings body has no
/// completion count, and an image body can have only a cost.
#[derive(serde::Deserialize)]
struct EndpointUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
    #[serde(default)]
    cost_in_usd_ticks: Option<i64>,
    #[serde(default)]
    cost: Option<UsageCost>,
}

impl Drop for CallMeter {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Reads one stream item into the meter. `true` means the item carries the call's final usage.
pub(crate) type Observe<T> = fn(&mut CallMeter, &T) -> bool;

pub(crate) fn observe_chat_chunk(meter: &mut CallMeter, chunk: &ChatCompletionChunk) -> bool {
    meter.note_model(&chunk.model);
    if let Some(usage) = chunk.usage.as_ref() {
        meter.usage.observe_chat_completions(usage);
    }
    false
}

pub(crate) fn observe_responses_event(
    meter: &mut CallMeter,
    event: &rs::ResponseStreamEvent,
) -> bool {
    use rs::ResponseStreamEvent as E;
    let response = match event {
        E::ResponseCompleted(e) => &e.response,
        E::ResponseIncomplete(e) => &e.response,
        E::ResponseFailed(e) => &e.response,
        _ => return false,
    };
    meter.observe_responses_response(response);
    true
}

pub(crate) fn observe_messages_event(
    meter: &mut CallMeter,
    event: &messages::MessageStreamEvent,
) -> bool {
    use messages::MessageStreamEvent as E;
    match event {
        E::MessageStart { message } => {
            meter.note_model(&message.model);
            meter.messages.observe_start(&message.usage);
        }
        E::MessageDelta { usage, .. } => meter.messages.observe_delta(usage),
        E::MessageStop => {
            meter.usage = meter.messages.finish();
            return true;
        }
        _ => return false,
    }
    meter.usage = meter.messages.finish();
    false
}

pub(crate) fn observe_ollama_chunk(meter: &mut CallMeter, chunk: &OllamaChatChunk) -> bool {
    if let Some(model) = chunk.model.as_deref() {
        meter.note_model(model);
    }
    if !chunk.done {
        return false;
    }
    meter.usage = CallUsage::from_ollama_done(chunk);
    true
}

/// Meter `stream`. A meter with no sink leaves the stream as it is.
pub(crate) fn meter_stream<T: Send + 'static>(
    stream: BoxStream<'static, Result<T>>,
    meter: CallMeter,
    observe: Observe<T>,
) -> BoxStream<'static, Result<T>> {
    if meter.sink.is_none() {
        return stream;
    }
    Metered {
        inner: stream,
        meter,
        observe,
    }
    .boxed()
}

struct Metered<T> {
    inner: BoxStream<'static, Result<T>>,
    meter: CallMeter,
    observe: Observe<T>,
}

impl<T> Stream for Metered<T> {
    type Item = Result<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_next(cx);
        match &polled {
            Poll::Ready(Some(Ok(item))) => {
                if (this.observe)(&mut this.meter, item) {
                    this.meter.finish();
                }
            }
            Poll::Ready(None) => this.meter.finish(),
            Poll::Ready(Some(Err(_))) | Poll::Pending => {}
        }
        polled
    }
}

#[cfg(test)]
#[path = "usage_sink_tests.rs"]
mod tests;
