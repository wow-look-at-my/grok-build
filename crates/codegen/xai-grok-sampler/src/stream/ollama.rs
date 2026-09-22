//! Layer-2 transform for Ollama's native `/api/chat` stream.
//!
//! The wire is NDJSON, not SSE: one whole JSON object per line, and the last
//! one carries `done: true` with the run's metrics. There is no `[DONE]`
//! sentinel and no event framing, so the line IS the event.
//!
//! One shape differs from every other backend here and drives most of this
//! file: a tool call arrives WHOLE, in one line, with its arguments as a JSON
//! object rather than as a string of fragments. There is nothing to
//! accumulate, so the call is emitted as a single delta carrying its complete
//! arguments — which is what the pager's streaming preview renders.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::{BoxStream, Stream};
use xai_grok_sampling_types::ollama::OllamaChatChunk;
use xai_grok_sampling_types::{
    AssistantItem, ConversationItem, ConversationResponse, ResponseModelMetadata, SamplingError,
    StopReason, TokenUsage, ToolCall, rs,
};

use crate::events::{SamplingChannel, SamplingErrorInfo, SamplingEvent};
use crate::metrics::InferenceLatencyStats;
use crate::types::RequestId;

/// Turn a stream of `/api/chat` lines into sampling events.
pub fn stream_ollama<'a>(
    raw_stream: BoxStream<'a, Result<OllamaChatChunk, SamplingError>>,
    model_metadata: Option<ResponseModelMetadata>,
    request_id: RequestId,
    idle_timeout: Duration,
) -> impl Stream<Item = SamplingEvent> + Send + 'a {
    async_stream::stream! {
        let stream_start = Instant::now();
        let mut chunk_timestamps: Vec<Instant> = Vec::new();

        yield SamplingEvent::StreamStarted {
            request_id: request_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };

        if let Some(metadata) = model_metadata {
            yield SamplingEvent::ModelMetadata {
                request_id: request_id.clone(),
                metadata,
            };
        }

        let mut assistant_text = String::new();
        let mut assistant_thinking = String::new();
        let mut assistant_tool_calls: Vec<ToolCall> = Vec::new();

        let mut final_model: Option<String> = None;
        let mut final_done_reason: Option<String> = None;
        let mut prompt_tokens: u32 = 0;
        let mut cached_prompt_tokens: u32 = 0;
        let mut completion_tokens: u32 = 0;

        let mut chunk_index: u64 = 0;
        let mut message_chunk_count: u64 = 0;
        let mut first_token_emitted = false;
        let mut next_tool_index: u32 = 0;
        let mut response_started = false;

        let mut stream = raw_stream;
        loop {
            let chunk = match tokio::time::timeout(idle_timeout, stream.next()).await {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(err))) => {
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    let err = SamplingError::IdleTimeout {
                        elapsed_secs: idle_timeout.as_secs(),
                    };
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
            };

            // Ollama reports a mid-stream failure as a line carrying only
            // `error`. Ending the stream quietly there would hand the turn a
            // truncated answer as if the model had finished.
            if let Some(message) = chunk.error {
                let err = SamplingError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    message,
                    model_metadata: None,
                    retry_after_secs: None,
                    should_retry: None,
                };
                yield SamplingEvent::Failed {
                    request_id: request_id.clone(),
                    error: SamplingErrorInfo::from(&err),
                };
                return;
            }

            if let Some(model) = chunk.model.clone() {
                if !response_started {
                    response_started = true;
                    final_model = Some(model.clone());
                    // Ollama mints no message id; the request id is the only
                    // stable name this response has.
                    yield SamplingEvent::ResponseStarted {
                        request_id: request_id.clone(),
                        message_id: request_id.to_string(),
                        model,
                        input_tokens: 0,
                        cache_read_input_tokens: 0,
                        cache_creation_input_tokens: 0,
                    };
                } else {
                    final_model = Some(model);
                }
            }

            if let Some(message) = chunk.message {
                if let Some(thinking) = message.thinking.filter(|t| !t.is_empty()) {
                    assistant_thinking.push_str(&thinking);
                    if !first_token_emitted {
                        first_token_emitted = true;
                        yield SamplingEvent::FirstToken {
                            request_id: request_id.clone(),
                        };
                    }
                    chunk_index += 1;
                    chunk_timestamps.push(Instant::now());
                    yield SamplingEvent::ChannelToken {
                        request_id: request_id.clone(),
                        channel: SamplingChannel::Reasoning,
                        text: thinking,
                        chunk_index,
                    };
                }

                if !message.content.is_empty() {
                    assistant_text.push_str(&message.content);
                    if !first_token_emitted {
                        first_token_emitted = true;
                        yield SamplingEvent::FirstToken {
                            request_id: request_id.clone(),
                        };
                    }
                    chunk_index += 1;
                    message_chunk_count += 1;
                    chunk_timestamps.push(Instant::now());
                    yield SamplingEvent::ChannelToken {
                        request_id: request_id.clone(),
                        channel: SamplingChannel::Text,
                        text: message.content,
                        chunk_index,
                    };
                }

                for call in message.tool_calls {
                    let tool_index = next_tool_index;
                    next_tool_index += 1;
                    // The arguments arrive as an object and every consumer
                    // downstream carries them as a JSON string, so this is the
                    // one place the two representations meet.
                    let arguments = serde_json::to_string(&call.function.arguments)
                        .unwrap_or_else(|_| "{}".to_owned());
                    // A call with no id of its own still has to pair with its
                    // result, and the index is the only thing that
                    // distinguishes two calls to the same tool in one message.
                    let id = call
                        .id
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| format!("{}-call-{tool_index}", request_id));

                    chunk_timestamps.push(Instant::now());
                    yield SamplingEvent::ToolCallDelta {
                        request_id: request_id.clone(),
                        tool_index,
                        id: Some(id.clone()),
                        name: Some(call.function.name.clone()),
                        // Whole, not a fragment: there is no second line to
                        // append, so a consumer that waits for one waits
                        // forever.
                        arguments_delta: Some(arguments.clone()),
                    };

                    assistant_tool_calls.push(ToolCall {
                        id: std::sync::Arc::from(id),
                        name: call.function.name,
                        arguments: std::sync::Arc::from(arguments),
                        vendor: Default::default(),
                    });
                }
            }

            if chunk.done {
                final_done_reason = chunk.done_reason.clone();
                prompt_tokens = chunk.prompt_eval_count.unwrap_or(prompt_tokens);
                cached_prompt_tokens = chunk
                    .prompt_eval_cached_count
                    .unwrap_or(cached_prompt_tokens);
                completion_tokens = chunk.eval_count.unwrap_or(completion_tokens);
                // The model load is not generation, and it is the one number
                // the OpenAI-compatible endpoint's `usage` cannot express.
                // Logging it is what separates a cold 40-second load from an
                // engine that stalled.
                if let Some(load_ns) = chunk.load_duration.filter(|&ns| ns > 0) {
                    tracing::debug!(
                        load_ms = load_ns / 1_000_000,
                        total_ms = chunk.total_duration.unwrap_or(0) / 1_000_000,
                        prompt_eval_ms = chunk.prompt_eval_duration.unwrap_or(0) / 1_000_000,
                        eval_ms = chunk.eval_duration.unwrap_or(0) / 1_000_000,
                        "ollama run metrics"
                    );
                }
                break;
            }
        }

        let stop_reason = if !assistant_tool_calls.is_empty() {
            Some(StopReason::ToolCalls)
        } else {
            match final_done_reason.as_deref() {
                Some("stop") => Some(StopReason::Stop),
                // The runner hit `num_predict`; the same truncation class the
                // other backends report as Length.
                Some("length") => Some(StopReason::Length),
                Some("load") => Some(StopReason::Stop),
                Some(other) => {
                    tracing::warn!(
                        wire_done_reason = %other,
                        "unrecognized ollama done_reason; treating as stop"
                    );
                    Some(StopReason::Stop)
                }
                None => Some(StopReason::Stop),
            }
        };

        if stop_reason == Some(StopReason::Length) {
            yield SamplingEvent::Failed {
                request_id: request_id.clone(),
                error: SamplingErrorInfo::from(&SamplingError::MaxTokensTruncation),
            };
            return;
        }

        let usage = (prompt_tokens > 0 || completion_tokens > 0).then(|| TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
            reasoning_tokens: 0,
            // Ollama counts the prefix its runner reused, which is the same
            // quantity a cache-read is elsewhere.
            cached_prompt_tokens,
            cache_creation_prompt_tokens: 0,
        });

        let mut items: Vec<ConversationItem> = Vec::new();
        if !assistant_thinking.trim().is_empty() {
            items.push(ConversationItem::Reasoning(rs::ReasoningItem {
                id: String::new(),
                summary: vec![rs::SummaryPart::SummaryText(rs::SummaryTextContent {
                    text: assistant_thinking,
                })],
                content: None,
                // Deliberately unsigned: Ollama's thinking is plain text, so
                // there is no blob binding it to the model that wrote it, and
                // a replay to another model is always safe.
                encrypted_content: None,
                status: None,
            }));
        }
        items.push(ConversationItem::Assistant(AssistantItem {
            content: std::sync::Arc::from(assistant_text),
            tool_calls: assistant_tool_calls,
            model_id: final_model.clone(),
            model_fingerprint: None,
            reasoning_effort: None,
        }));

        let stream_end = Instant::now();
        let metrics =
            InferenceLatencyStats::from_timestamps(stream_start, &chunk_timestamps, stream_end);

        let response = ConversationResponse {
            items,
            stop_reason,
            usage,
            // A local runtime bills nothing, so there is no price to report and
            // the shell's own pricing fallback answers for it.
            cost_usd_ticks: None,
            message_chunks_emitted: message_chunk_count,
            doom_loop_signals: Vec::new(),
            stop_message: None,
            message_id: None,
            raw_stop_reason: final_done_reason,
            stop_sequence: None,
        };

        yield SamplingEvent::Completed {
            request_id: request_id.clone(),
            response: Box::new(response),
            metrics,
        };
    }
}

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
