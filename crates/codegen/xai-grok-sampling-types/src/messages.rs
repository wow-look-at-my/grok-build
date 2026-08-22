//! Anthropic Messages API (`/v1/messages`) wire types.
//!
//! These types represent the request/response format for the `/v1/messages` API.

use serde::{Deserialize, Serialize};

// ============================================================================
// Request Types
// ============================================================================

/// POST /v1/messages request body
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolParam>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoiceParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormat {
    JsonSchema { schema: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemParam {
    Text(String),
    Blocks(Vec<TextBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    #[serde(rename = "type")]
    pub r#type: String, // always "text"
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub r#type: String, // "ephemeral"
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self {
            r#type: "ephemeral".to_owned(),
        }
    }
}

/// Content blocks used in both requests and responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Thinking {
        thinking: String,
        // Some Anthropic-compatible providers (e.g. Synthetic) omit the
        // encrypted `signature` on thinking blocks; tolerate its absence.
        #[serde(default)]
        signature: String,
    },
    /// Encrypted reasoning the model chose to redact. Carries only an opaque
    /// `data` blob (never plaintext). Added so a stream that includes one
    /// deserializes instead of failing the whole event parse; behavior-preserving
    /// for producers (never constructed by request-building or the sampler).
    RedactedThinking { data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

/// Tool definition (Anthropic Messages API format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParam {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Tool choice (Anthropic Messages API format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoiceParam {
    Auto,
    Any,
    Tool { name: String },
}

/// Extended thinking configuration
///
/// Three modes per the Anthropic Messages API:
/// - Adaptive: 4.6+ models, API decides budget
/// - Enabled: 4.0-4.5 models, explicit budget_tokens
/// - Disabled: pre-thinking models or thinking_budget=0
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingDisplay {
    Omitted,
    Summarized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Enabled {
        budget_tokens: u32,
    },
    Adaptive {
        // Newer thinking-capable models omit thinking content unless display = "summarized".
        // Older models ignore this field. Skip when None to stay back-compat.
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

// ============================================================================
// Response Types
// ============================================================================

/// Non-streaming response from POST /v1/messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String, // "message"
    pub role: String, // "assistant"
    /// `null` reads as no content blocks. `message_start` carries an empty
    /// content list by definition, and a gateway written in Go marshals that
    /// unset slice as `null` -- which would fail the opening event of every
    /// stream it relays.
    #[serde(default, deserialize_with = "crate::serde_helpers::null_as_default")]
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub stop_reason: Option<StopReason>,
    pub usage: MessagesUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Refusal,
    PauseTurn,
    ModelContextWindowExceeded,
    /// Catch-all for stop reasons this client does not know yet, so a new
    /// server-side value can never fail the terminal `message_delta` parse
    /// and discard an already-streamed response. Preserves the wire string
    /// for logging and faithful re-serialization; must stay the LAST variant
    /// (serde tries the tagged variants above first).
    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessagesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    /// What the call cost, in USD ticks (1 USD = 1e10). Anthropic itself
    /// reports no price; a gateway speaking this protocol does, and without
    /// these two fields its real number is thrown away for an estimate off
    /// the model's configured pricing. Ticks win over the float when a
    /// gateway sends both.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "cost_usd_ticks"
    )]
    pub cost_in_usd_ticks: Option<i64>,
    /// The same price as a USD float, the shape OpenRouter and Bifrost use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::UsageCost>,
}

// ============================================================================
// Streaming Event Types
// ============================================================================

/// Top-level streaming event (SSE `type` field determines variant)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageStreamEvent {
    MessageStart {
        message: MessagesResponse,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        usage: MessageDeltaUsage,
    },
    MessageStop,
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: StreamDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    Ping,
    Error {
        error: StreamError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDeltaBody {
    pub stop_reason: Option<StopReason>,
    /// The stop sequence that was matched, present only when
    /// `stop_reason == "stop_sequence"`; `None` otherwise. Previously discarded
    /// at parse — captured so consumers can echo the matched string (Messages
    /// API `message.stop_sequence`). Optional so its absence never fails the
    /// terminal parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// Provider detail for the stop; on `refusal`, `explanation` carries the
    /// reason the request was blocked (e.g. an Anthropic ToS auto-refusal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<StopDetails>,
}

/// Detail for a terminal `message_delta`, e.g.
/// `{"type":"refusal","category":"frontier_llm","explanation":"..."}`.
/// All fields optional so an unknown shape never fails the terminal parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StopDetails {
    #[serde(rename = "type", default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageDeltaUsage {
    pub output_tokens: u32,
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
    /// The terminal delta is where a gateway settles the price of the call —
    /// see [`MessagesUsage::cost_in_usd_ticks`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "cost_usd_ticks"
    )]
    pub cost_in_usd_ticks: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<crate::UsageCost>,
}

/// Content delta within a content_block_delta event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    #[serde(rename = "type")]
    pub r#type: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `message_start` opens every stream with an empty content list, so a
    /// gateway that writes an unset slice as `null` puts this shape on the
    /// wire for every turn it relays. The event is internally tagged, so serde
    /// buffers it and the failure arrives without a line/column -- exactly the
    /// bare "invalid type: null, expected a sequence" users see.
    #[test]
    fn message_start_deserializes_null_content() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "content": null,
                    "model": "test-model",
                    "stop_reason": null,
                    "usage": {"input_tokens": 18, "output_tokens": 0}
                }
            }"#,
        )
        .expect("a null `content` must not fail message_start");

        match event {
            MessageStreamEvent::MessageStart { message } => {
                assert!(message.content.is_empty());
                assert_eq!(message.usage.input_tokens, 18);
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_deserializes_all_known_values_and_catches_unknown() {
        let parse = |raw: &str| -> StopReason {
            serde_json::from_str(&format!("\"{raw}\""))
                .unwrap_or_else(|e| panic!("stop_reason {raw:?} must parse: {e}"))
        };
        assert!(matches!(parse("end_turn"), StopReason::EndTurn));
        assert!(matches!(parse("max_tokens"), StopReason::MaxTokens));
        assert!(matches!(parse("tool_use"), StopReason::ToolUse));
        assert!(matches!(parse("stop_sequence"), StopReason::StopSequence));
        assert!(matches!(parse("refusal"), StopReason::Refusal));
        assert!(matches!(parse("pause_turn"), StopReason::PauseTurn));
        assert!(matches!(
            parse("model_context_window_exceeded"),
            StopReason::ModelContextWindowExceeded
        ));
        match parse("some_future_stop_reason") {
            StopReason::Unknown(s) => assert_eq!(s, "some_future_stop_reason"),
            other => panic!("unknown value must preserve the wire string, got {other:?}"),
        }
        assert_eq!(
            serde_json::to_string(&StopReason::Unknown("some_future_stop_reason".into())).unwrap(),
            "\"some_future_stop_reason\"",
            "catch-all must re-serialize the wire string faithfully"
        );
        // The catch-all must also work through the Option<StopReason> field
        // it is parsed from in production.
        let delta: MessageDeltaBody =
            serde_json::from_str(r#"{"stop_reason":"mystery_reason"}"#).unwrap();
        match delta.stop_reason {
            Some(StopReason::Unknown(s)) => assert_eq!(s, "mystery_reason"),
            other => panic!("expected Unknown through Option, got {other:?}"),
        }
    }

    /// The terminal `message_delta` of a refusal-terminated stream must parse
    /// (the internally-tagged `MessageStreamEvent` wrapper is the actual
    /// production parse site, hence the full-event fixture).
    #[test]
    fn message_delta_with_refusal_stop_reason_parses() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":5,"input_tokens":10}}"#,
        )
        .expect("refusal message_delta must deserialize");
        match event {
            MessageStreamEvent::MessageDelta { delta, usage } => {
                assert!(matches!(delta.stop_reason, Some(StopReason::Refusal)));
                assert!(delta.stop_details.is_none(), "no stop_details on the wire");
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    /// A refusal `message_delta` carrying `stop_details` (as emitted by
    /// Anthropic ToS auto-refusals) must parse and preserve the explanation,
    /// and unknown keys inside `stop_details` must not fail the parse.
    #[test]
    fn message_delta_with_refusal_stop_details_parses() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_sequence":null,"stop_details":{"type":"refusal","category":"frontier_llm","explanation":"This request was blocked.","future_key":42}},"usage":{"output_tokens":0}}"#,
        )
        .expect("refusal message_delta with stop_details must deserialize");
        match event {
            MessageStreamEvent::MessageDelta { delta, .. } => {
                assert!(matches!(delta.stop_reason, Some(StopReason::Refusal)));
                let details = delta.stop_details.expect("stop_details must be captured");
                assert_eq!(details.r#type.as_deref(), Some("refusal"));
                assert_eq!(details.category.as_deref(), Some("frontier_llm"));
                assert_eq!(
                    details.explanation.as_deref(),
                    Some("This request was blocked.")
                );
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    /// A `stop_sequence`-terminated `message_delta` must parse and preserve the
    /// matched string (previously discarded), so consumers can echo it on the
    /// Messages API `message.stop_sequence`.
    #[test]
    fn message_delta_captures_matched_stop_sequence() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"stop_sequence","stop_sequence":"END"},"usage":{"output_tokens":7}}"#,
        )
        .expect("stop_sequence message_delta must deserialize");
        match event {
            MessageStreamEvent::MessageDelta { delta, .. } => {
                assert!(matches!(delta.stop_reason, Some(StopReason::StopSequence)));
                assert_eq!(delta.stop_sequence.as_deref(), Some("END"));
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }

        // Absent `stop_sequence` stays `None` and never fails the parse.
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
        )
        .expect("end_turn message_delta must deserialize");
        match event {
            MessageStreamEvent::MessageDelta { delta, .. } => {
                assert_eq!(delta.stop_sequence, None);
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    /// A `redacted_thinking` content block must deserialize into the dedicated
    /// variant (preserving the opaque `data`) instead of failing the whole
    /// `content_block_start` parse and discarding an already-streamed response.
    #[test]
    fn redacted_thinking_content_block_parses() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"EvwBCkgY...opaque"}}"#,
        )
        .expect("redacted_thinking content_block_start must deserialize");
        match event {
            MessageStreamEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlock::RedactedThinking { data } => {
                    assert_eq!(data, "EvwBCkgY...opaque");
                }
                other => panic!("expected RedactedThinking, got {other:?}"),
            },
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }

        // Round-trips to Claude's wire shape.
        let json =
            serde_json::to_value(ContentBlock::RedactedThinking { data: "abc".into() }).unwrap();
        assert_eq!(json["type"], "redacted_thinking");
        assert_eq!(json["data"], "abc");
    }

    /// Every wire shape a gateway prices a call with has to parse, and an
    /// Anthropic response that prices nothing has to stay priceless rather
    /// than read as free.
    #[test]
    fn usage_parses_every_cost_shape_a_gateway_sends() {
        let cases = [
            (r#""cost_in_usd_ticks":4160000"#, Some(4_160_000), None),
            (r#""cost_usd_ticks":4160000"#, Some(4_160_000), None),
            (r#""cost":0.0000416"#, None, Some(0.0000416)),
            (
                r#""cost":{"total_cost":0.0000416,"input_tokens_cost":0.00001}"#,
                None,
                Some(0.0000416),
            ),
            (r#""cache_read_input_tokens":0"#, None, None),
        ];

        for (field, ticks, cost) in cases {
            let event: MessageStreamEvent = serde_json::from_str(&format!(
                r#"{{"type":"message_delta","delta":{{"stop_reason":"end_turn"}},"usage":{{"output_tokens":5,{field}}}}}"#
            ))
            .unwrap_or_else(|e| panic!("message_delta with {field} must deserialize: {e}"));
            let MessageStreamEvent::MessageDelta { usage, .. } = event else {
                panic!("expected MessageDelta for {field}");
            };
            assert_eq!(usage.cost_in_usd_ticks, ticks, "{field}");
            assert_eq!(usage.cost.map(|c| c.as_usd_float()), cost, "{field}");

            let start: MessageStreamEvent = serde_json::from_str(&format!(
                r#"{{"type":"message_start","message":{{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"m","usage":{{"input_tokens":10,"output_tokens":0,{field}}}}}}}"#
            ))
            .unwrap_or_else(|e| panic!("message_start with {field} must deserialize: {e}"));
            let MessageStreamEvent::MessageStart { message } = start else {
                panic!("expected MessageStart for {field}");
            };
            assert_eq!(message.usage.cost_in_usd_ticks, ticks, "{field}");
        }
    }

    /// Anthropic-compatible providers without prompt caching / extended
    /// thinking signatures (e.g. Synthetic) send thinking blocks with no
    /// `signature`. The whole response must still parse; the signature falls
    /// back to empty instead of failing the request.
    #[test]
    fn thinking_block_without_signature_parses() {
        let event: MessageStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"pondering"}}"#,
        )
        .expect("signature-less thinking content_block_start must deserialize");
        match event {
            MessageStreamEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    assert_eq!(thinking, "pondering");
                    assert!(signature.is_empty());
                }
                other => panic!("expected Thinking, got {other:?}"),
            },
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }
    }

    #[test]
    fn output_format_json_schema_wire_shape() {
        let fmt = OutputFormat::JsonSchema {
            schema: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        };
        let json = serde_json::to_value(&fmt).unwrap();
        assert_eq!(json["type"], "json_schema");
        assert_eq!(json["schema"]["type"], "object");
        assert!(json.get("name").is_none());

        let config = OutputConfig {
            effort: None,
            format: Some(fmt),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert!(json.get("effort").is_none(), "effort omitted when None");
        assert_eq!(json["format"]["type"], "json_schema");
    }
}
