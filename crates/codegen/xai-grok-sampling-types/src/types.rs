use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::num::NonZeroU64;

// ============================================================================
// TraceContext — cloneable, type-erased context for request tracing
// ============================================================================

/// Object-safe trait for opaque tracing context attached to requests. `Clone` is not object-safe, so we use a `clone_box`
/// method instead. Any concrete type that is `Clone + Send + Sync + Debug + 'static` gets a blanket impl, so callers just
/// do:
pub trait TraceContext: std::any::Any + Send + Sync + std::fmt::Debug {
    fn clone_box(&self) -> Box<dyn TraceContext>;

    /// Upcast to `&dyn Any` for downcasting back to the concrete type.
    fn as_any(&self) -> &dyn std::any::Any;
}

impl<T> TraceContext for T
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    fn clone_box(&self) -> Box<dyn TraceContext> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Clone for Box<dyn TraceContext> {
    fn clone(&self) -> Self {
        // Explicitly dereference to `&dyn TraceContext` so `clone_box()` dispatches through the vtable to the concrete type's
        // implementation. Without the deref, `self.clone_box()` resolves via auto-deref to the blanket impl on `Box<dyn
        // TraceContext>` itself. That impl calls `self.clone()`, which calls `clone_box()` again, recursing forever
        let inner: &dyn TraceContext = &**self;
        inner.clone_box()
    }
}

use crate::serde_helpers::null_as_default as deserialize_null_default;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<ChatRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_parameters: Option<SearchParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<crate::rs::ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// custom headers
    #[serde(skip)]
    pub x_grok_conv_id: Option<String>,
    #[serde(skip)]
    pub x_grok_req_id: Option<String>,
    #[serde(skip)]
    pub x_grok_session_id: Option<String>,
    #[serde(skip)]
    pub x_grok_turn_idx: Option<String>,
    #[serde(skip)]
    pub x_grok_transient_retry: Option<String>,
    #[serde(skip)]
    pub x_grok_agent_id: Option<String>,
    #[serde(skip)]
    pub x_grok_deployment_id: Option<String>,
    #[serde(skip)]
    pub x_grok_user_id: Option<String>,

    /// Optional opaque tracing context (e.g., where to persist the finalized request payload).
    /// Consumers downcast via `trace.as_ref().unwrap().as_any().downcast_ref::<T>()`.
    #[serde(skip)]
    pub trace: Option<Box<dyn TraceContext>>,
    /// Caller span's W3C `traceparent`; see [`crate::ConversationRequest::traceparent`].
    #[serde(skip)]
    pub traceparent: Option<String>,
}

impl ChatCompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatRequestMessage>) -> Self {
        Self {
            model: Some(model.into()),
            messages,
            temperature: None,
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            search_parameters: None,
            response_format: None,
            reasoning_effort: None,
            x_grok_conv_id: None,
            x_grok_req_id: None,
            x_grok_session_id: None,
            x_grok_turn_idx: None,
            x_grok_transient_retry: None,
            x_grok_agent_id: None,
            x_grok_deployment_id: None,
            x_grok_user_id: None,
            trace: None,
            traceparent: None,
        }
    }

    pub fn from_messages(messages: Vec<ChatRequestMessage>) -> Self {
        Self {
            model: None,
            messages,
            temperature: None,
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            user: None,
            tools: None,
            tool_choice: None,
            search_parameters: None,
            response_format: None,
            reasoning_effort: None,
            x_grok_conv_id: None,
            x_grok_req_id: None,
            x_grok_session_id: None,
            x_grok_turn_idx: None,
            x_grok_transient_retry: None,
            x_grok_agent_id: None,
            x_grok_deployment_id: None,
            x_grok_user_id: None,
            trace: None,
            traceparent: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub fn set_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ChatContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ChatContentBlock>),
}

impl MessageContent {
    pub fn is_empty(&self) -> bool {
        match self {
            MessageContent::Blocks(blocks) => blocks.is_empty(),
            MessageContent::Text(text) => text.is_empty(),
        }
    }

    pub fn blocks(&self) -> Vec<ChatContentBlock> {
        match self {
            MessageContent::Blocks(blocks) => blocks.clone(),
            MessageContent::Text(text) => vec![ChatContentBlock::Text { text: text.clone() }],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatRequestMessage {
    pub role: Role,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The model used for this message (typically set on assistant responses)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// The reasoning/thinking content from the model (for models that support extended thinking)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ChatRequestMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content.into()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            model_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content.into()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            model_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(
        content: impl Into<String>,
        model_id: impl Into<String>,
        reasoning_content: Option<String>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
            model_id: Some(model_id.into()),
            reasoning_content,
        }
    }

    pub fn assistant_tool_call(tool_call: ToolCallRequest) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text("".into()),
            name: None,
            tool_calls: vec![tool_call],
            tool_call_id: None,
            model_id: None,
            reasoning_content: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: MessageContent::Text(content.into()),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            model_id: None,
            reasoning_content: None,
        }
    }

    pub fn is_system_message(&self) -> bool {
        self.role == Role::System
    }

    pub fn text_content(&self) -> String {
        self.content
            .blocks()
            .iter()
            .filter_map(|block| match block {
                ChatContentBlock::Text { text } => Some(text.clone()),
                ChatContentBlock::ImageUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Set text content, replacing all existing content blocks
    pub fn set_text_content(&mut self, text: impl Into<String>) {
        self.content = MessageContent::Text(text.into());
    }

    pub fn append_text_content(&mut self, text: impl Into<String>) {
        if self.content.is_empty() {
            self.set_text_content(text);
            return;
        }

        let new_content = match &self.content {
            MessageContent::Text(prev) => MessageContent::Text(format!("{}{}", prev, text.into())),
            MessageContent::Blocks(blocks) => {
                let mut blocks = blocks.clone();
                blocks.push(ChatContentBlock::Text { text: text.into() });
                MessageContent::Blocks(blocks)
            }
        };

        self.content = new_content;
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Calculate how many chat messages to keep for a given target prompt index (0-based, inclusive).
pub fn chat_truncate_for_prompt(
    chat_history: &[ChatRequestMessage],
    target_prompt_index: usize,
) -> usize {
    let mut user_count = 0;
    let mut keep_count = 0;

    for (i, msg) in chat_history.iter().enumerate() {
        if matches!(msg.role, Role::User) {
            user_count += 1;
            if user_count > target_prompt_index + 1 {
                keep_count = i;
                break;
            }
        }
        keep_count = i + 1;
    }

    keep_count
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Function,
}

pub use xai_tool_types::definition::{FunctionTool, ToolDefinition};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ToolChoice {
    Preset(String),
    Function {
        #[serde(rename = "type")]
        kind: ToolType,
        function: ToolChoiceFunction,
    },
}

impl ToolChoice {
    pub fn auto() -> Self {
        Self::Preset("auto".to_string())
    }

    pub fn none() -> Self {
        Self::Preset("none".to_string())
    }

    pub fn required() -> Self {
        Self::Preset("required".to_string())
    }

    pub fn function(name: impl Into<String>) -> Self {
        Self::Function {
            kind: ToolType::Function,
            function: ToolChoiceFunction { name: name.into() },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolChoiceFunction {
    pub name: String,
}

/// Keys a tool call carries through an OpenAI-shaped API that this client only
/// relays. Gemini 3 rejects a replayed function call whose thought signature is
/// missing ("Function call is missing a thought_signature in functionCall
/// parts"), and that signature reaches an OpenAI-shaped client only inside one
/// of these: `extra_content` is Google's own spelling, `provider_specific_fields`
/// the one a translating gateway uses. Nothing in here is read — what arrives on
/// a tool call goes back out unchanged, which is the only form the provider
/// accepts it in.
pub const TOOL_CALL_VENDOR_KEYS: [&str; 2] = ["extra_content", "provider_specific_fields"];

/// The [`TOOL_CALL_VENDOR_KEYS`] out of everything a tool call arrived with.
/// The rest is response-shaped bookkeeping (`index` and the like) that a
/// request has no place for.
pub fn tool_call_vendor_fields(
    all: &std::collections::BTreeMap<String, serde_json::Value>,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    TOOL_CALL_VENDOR_KEYS
        .iter()
        .filter_map(|key| Some(((*key).to_string(), all.get(*key)?.clone())))
        .collect()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCallRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: ToolType,
    pub function: ToolCallFunction,
    /// Relayed verbatim; see [`TOOL_CALL_VENDOR_KEYS`]. Empty flattens to
    /// nothing, so a provider that never sent one sees the same request as before.
    #[serde(flatten, default)]
    pub vendor: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ToolCallRequest {
    pub fn function(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            id: None,
            kind: ToolType::Function,
            function: ToolCallFunction::new(name, arguments),
            vendor: Default::default(),
        }
    }

    /// Carry a provider's own per-call fields back to it; see
    /// [`TOOL_CALL_VENDOR_KEYS`].
    pub fn with_vendor(
        mut self,
        vendor: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Self {
        self.vendor = vendor;
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    /// `null` reads as no choices: a gateway written in Go marshals an unset
    /// slice as `null`, and rejecting the response loses the usage that rides
    /// with it.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatResponseMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
    /// Provider-reported error finish (OpenRouter sends this when the
    /// upstream provider fails mid-generation). Maps to `StopReason::Stop`
    /// so the turn completes normally rather than surfacing a deserialization
    /// error to the user.
    Error,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatResponseMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// `null` reads as no tool calls, matching [`ChatChunkDelta::tool_calls`].
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_null_default"
    )]
    pub tool_calls: Vec<ToolCallResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCallResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
    /// Everything else the provider put on the call, kept so
    /// [`tool_call_vendor_fields`] can pick the part a replay has to carry back.
    #[serde(flatten, default)]
    pub vendor: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

impl ToolCallFunction {
    pub fn new(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    pub fn from_json(name: impl Into<String>, arguments: &Value) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    /// xAI extension: request price in USD ticks (1 USD = 1e10 ticks).
    /// The REST mapper backfills `0` for unbilled requests; capture sites normalize `0` to "unreported" (see `stream/chat_completions.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_in_usd_ticks: Option<i64>,
    /// Provider-reported request price in USD. OpenRouter and other
    /// OpenAI-compatible aggregators report this instead of `cost_in_usd_ticks`.
    /// Accepts both a bare float (e.g. `0.0000416`) and the Bifrost cost object
    /// (e.g. `{"total_cost": 0.0000416, "input_tokens_cost": ...}`); capture
    /// sites convert to ticks (×1e10) and prefer `cost_in_usd_ticks` when both
    /// are present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<UsageCost>,
}

/// A provider-reported USD cost that may arrive as either a bare float or a
/// Bifrost-style cost object. The float form is used by OpenRouter directly;
/// the object form (`{"total_cost": ...}`) is what Bifrost emits when it
/// re-serializes a passthrough `BifrostCost` struct.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageCost(f64);

impl UsageCost {
    /// The total cost as a USD float, regardless of which wire shape arrived.
    pub fn as_usd_float(&self) -> f64 {
        self.0
    }
}

impl From<f64> for UsageCost {
    fn from(v: f64) -> Self {
        UsageCost(v)
    }
}

impl Serialize for UsageCost {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Re-emit as the simpler float form; we only read cost, never forward it.
        s.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for UsageCost {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        struct CostObject {
            #[serde(default)]
            total_cost: Option<f64>,
            #[serde(default)]
            input_tokens_cost: Option<f64>,
            #[serde(default)]
            output_tokens_cost: Option<f64>,
            #[serde(default)]
            reasoning_tokens_cost: Option<f64>,
        }

        let v = serde_json::Value::deserialize(d)?;
        match &v {
            // Bare float: `"cost": 0.0000416`
            serde_json::Value::Number(n) => n
                .as_f64()
                .map(UsageCost)
                .ok_or_else(|| D::Error::custom("cost number is not a finite f64")),
            // Bifrost object: `"cost": {"total_cost": 0.0000416, ...}`
            serde_json::Value::Object(_) => {
                let obj = serde_json::from_value::<CostObject>(v.clone())
                    .map_err(|e| D::Error::custom(format!("invalid cost object: {e}")))?;
                // Prefer total_cost; fall back to the sum of the component
                // costs when the gateway omits the rollup (some providers only
                // report per-tier costs).
                let total = obj
                    .total_cost
                    .or_else(|| {
                        Some(
                            obj.input_tokens_cost.unwrap_or(0.0)
                                + obj.output_tokens_cost.unwrap_or(0.0)
                                + obj.reasoning_tokens_cost.unwrap_or(0.0),
                        )
                    })
                    .filter(|f| f.is_finite());
                total
                    .map(UsageCost)
                    .ok_or_else(|| D::Error::custom("cost object has no usable cost fields"))
            }
            _ => Err(D::Error::custom("cost must be a number or an object")),
        }
    }
}

/// Convert a USD float to integer ticks (1 USD = 1e10 ticks), rounding to
/// the nearest tick. Non-positive or NaN/inf values yield `None` ("unreported",
/// never "free"). Used by the capture sites that read `usage.cost`.
pub fn usd_float_to_ticks(usd: Option<f64>) -> Option<i64> {
    let v = usd?;
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let ticks = (v * 1e10).round() as i64;
    (ticks > 0).then_some(ticks)
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
    #[serde(default)]
    pub accepted_prediction_tokens: u32,
    #[serde(default)]
    pub rejected_prediction_tokens: u32,
}
// ============ Streaming types ============

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    /// `null` reads as no choices. Bifrost's `BifrostChatResponse.Choices` has
    /// no `omitempty`, so every chunk it builds without choices -- the trailing
    /// usage chunk among them -- arrives as `"choices": null`, and failing the
    /// parse kills the whole turn with a serialization error.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub choices: Vec<ChatChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::empty_string_as_none"
    )]
    pub system_fingerprint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

/// The first chunk carries `id`, `type`, `index`, `function.name`, and the start of `arguments`; Subsequent chunks only
/// carry `index` and a `function.arguments` fragment (no `id`, no `name`). All fields except `index` are therefore
/// optional so we can deserialize every chunk.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ToolCallDelta {
    /// The positional index that correlates delta chunks of the same tool call.
    #[serde(default)]
    pub index: u32,
    /// Only present in the first chunk for this tool call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Only present in the first chunk (usually "function").
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The function name and/or argument fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<ToolCallFunctionDelta>,
    /// Everything else the provider put on the call. Gemini's thought signature
    /// rides the chunk that opens the call, which is why this is captured on the
    /// delta and not only on the whole response.
    #[serde(flatten, default)]
    pub vendor: std::collections::BTreeMap<String, serde_json::Value>,
}

/// `name` is only present in the first chunk; `arguments` may arrive across many chunks.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ToolCallFunctionDelta {
    /// Only present in the first chunk for this tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Argument fragment (may be empty or partial JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Thinking/chain-of-thought text streamed by the model. Deserializes from
    /// either `reasoning_content` (OpenAI/xAI naming) or `reasoning`
    /// (synthetic.new's OpenAI-compatible naming) so both wire shapes feed the
    /// same accumulator; serializes as `reasoning_content` (the shape the
    /// resend path / providers accept).
    #[serde(skip_serializing_if = "Option::is_none", alias = "reasoning")]
    pub reasoning_content: Option<String>,
    /// A JSON `null` deserializes as an empty vec.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_null_default"
    )]
    pub tool_calls: Vec<ToolCallDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Parameters to control realtime data.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchParameters {
    /// Choose the mode to query realtime data: `off`: no search performed and no external sources will be considered; `on`
    /// (default): the model will search in every source for relevant data; `auto`: the model chooses whether to search data
    /// or not and where to search the data.
    pub mode: Option<String>,
    /// List of sources to search in. If no sources are specified, the model will look over the web and X by default.
    pub sources: Option<Vec<SearchSource>>,
    /// Date from which to consider the results in ISO-8601 YYYY-MM-DD.
    pub from_date: Option<String>,
    /// Date up to which to consider the results in ISO-8601 YYYY-MM-DD.
    pub to_date: Option<String>,
    /// Whether to return citations in the response or not.
    pub return_citations: Option<bool>,
    /// Maximum number of search results to use.
    pub max_search_results: Option<i32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum SearchSource {
    #[serde(rename = "x")]
    X {
        /// X Handles of the users from whom to consider the posts.
        included_x_handles: Option<Vec<String>>,
        /// DEPRECATED in favor of `included_x_handles`.
        x_handles: Option<Vec<String>>,
        /// List of X handles to exclude from the search results.
        excluded_x_handles: Option<Vec<String>>,
        /// The minimum favorite count of the X posts to consider.
        post_favorite_count: Option<i32>,
        /// The minimum view count of the X posts to consider.
        post_view_count: Option<i32>,
    },
    #[serde(rename = "web")]
    Web {
        /// List of website to exclude from the search results.
        excluded_websites: Option<Vec<String>>,
        /// List of website to allow in the search results.
        allowed_websites: Option<Vec<String>>,
        /// ISO alpha-2 code of the country.
        country: Option<String>,
        /// If set to true, mature content won't be considered during the search.
        safe_search: Option<bool>,
    },
    #[serde(rename = "news")]
    News {
        /// List of website to exclude from the search results.
        excluded_websites: Option<Vec<String>>,
        /// ISO alpha-2 code of the country.
        country: Option<String>,
        /// If set to true, mature content won't be considered during the search.
        safe_search: Option<bool>,
    },
    #[serde(rename = "rss")]
    Rss {
        /// Links of the RSS feeds.
        links: Vec<String>,
    },
}

/// Per-model config for the `x-compaction-at` request header (a token count). The remote-config value is polymorphic:
/// `true` enables the header with the value `context_window * auto_compact_threshold_percent / 100`. `false` (or absent)
/// disables it; an integer `N` sends the constant `N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum CompactionAtTokens {
    Enabled(bool),
    Fixed(u64),
}

impl CompactionAtTokens {
    /// Resolve the absolute token count to send, or `None` when disabled.
    pub fn resolve(self, context_window: u64, threshold_percent: u8) -> Option<u64> {
        match self {
            CompactionAtTokens::Enabled(false) => None,
            CompactionAtTokens::Enabled(true) => {
                Some(context_window * u64::from(threshold_percent) / 100)
            }
            CompactionAtTokens::Fixed(n) => Some(n),
        }
    }
}

/// Per-model config for the `x-compactions-remaining` request header. `true` sends the dynamic value (1 on the
/// uncompacted prefix, 0 once the session compacts). `false`/absent disables the header; an integer `N` sends the
/// constant `N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum CompactionsRemaining {
    Dynamic(bool),
    Fixed(u8),
}

impl CompactionsRemaining {
    /// Resolve the header value to send, or `None` when disabled.
    pub fn resolve(self, has_compaction_summary: bool) -> Option<u8> {
        match self {
            CompactionsRemaining::Dynamic(false) => None,
            CompactionsRemaining::Dynamic(true) => Some(u8::from(!has_compaction_summary)),
            CompactionsRemaining::Fixed(n) => Some(n),
        }
    }
}

/// `None`/`Minimal` are omitted on the Anthropic Messages API.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    strum::AsRefStr,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn to_responses_api(self) -> crate::rs::ReasoningEffort {
        match self {
            Self::None => crate::rs::ReasoningEffort::None,
            Self::Minimal => crate::rs::ReasoningEffort::Minimal,
            Self::Low => crate::rs::ReasoningEffort::Low,
            Self::Medium => crate::rs::ReasoningEffort::Medium,
            Self::High => crate::rs::ReasoningEffort::High,
            Self::Xhigh => crate::rs::ReasoningEffort::Xhigh,
            Self::Max => crate::rs::ReasoningEffort::Max,
        }
    }

    /// Inverse of [`to_responses_api`](Self::to_responses_api): the effort the Responses API echoes back on `response.reasoning.effort`.
    pub fn from_responses_api(effort: crate::rs::ReasoningEffort) -> Self {
        match effort {
            crate::rs::ReasoningEffort::None => Self::None,
            crate::rs::ReasoningEffort::Minimal => Self::Minimal,
            crate::rs::ReasoningEffort::Low => Self::Low,
            crate::rs::ReasoningEffort::Medium => Self::Medium,
            crate::rs::ReasoningEffort::High => Self::High,
            crate::rs::ReasoningEffort::Xhigh => Self::Xhigh,
            crate::rs::ReasoningEffort::Max => Self::Max,
        }
    }

    pub fn to_messages_api(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Minimal => None,
            _ => Some(self.into()),
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err(format!(
                "invalid reasoning effort: {s:?} (expected one of: none, minimal, low, medium, high, xhigh, max)"
            )),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

pub fn parse_canonical_effort_token(token: &str) -> Option<ReasoningEffort> {
    token.parse().ok()
}

/// The `reasoning.summary` requested on the Responses API.
/// `None` omits the field, for gateways that reject it (AWS Bedrock Mantle returns 400 for it as of 2026-09).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::AsRefStr,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "snake_case")]
pub enum ReasoningSummary {
    None,
    Auto,
    #[default]
    Concise,
    Detailed,
}

impl ReasoningSummary {
    pub fn to_responses_api(self) -> Option<crate::rs::ReasoningSummary> {
        match self {
            Self::None => None,
            Self::Auto => Some(crate::rs::ReasoningSummary::Auto),
            Self::Concise => Some(crate::rs::ReasoningSummary::Concise),
            Self::Detailed => Some(crate::rs::ReasoningSummary::Detailed),
        }
    }
}

pub const REASONING_EFFORT_META_KEY: &str = "reasoningEffort";
pub const SUPPORTS_REASONING_EFFORT_META_KEY: &str = "supportsReasoningEffort";
/// Set from the `favorite_models` globs. The picker reads it to decide what its
/// opening list holds.
pub const FAVORITE_META_KEY: &str = "favorite";

/// Whether this model's ACP meta marks it a favorite.
pub fn favorite_meta(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|m| m.get(FAVORITE_META_KEY))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Set only by a provider whose listing reports residency (Ollama's
/// `/api/ps`, LM Studio's `loaded_instances`). The picker draws a dot from it.
pub const LOADED_IN_VRAM_META_KEY: &str = "loadedInVram";

/// Whether this model is resident in VRAM, or `None` where nobody can say.
///
/// The three answers are distinct and the picker renders each differently: a
/// remote model has no dot at all, a local model that is loaded has a lit one,
/// and a local model that is not has a dim one. Collapsing the absent case
/// into `false` puts a cold dot beside every cloud model in the list.
pub fn loaded_in_vram_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<bool> {
    meta.and_then(|m| m.get(LOADED_IN_VRAM_META_KEY))
        .and_then(|v| v.as_bool())
}

/// The `[model_providers.<id>]` a model routes through. The picker shows it
/// where two rows would otherwise read the same.
pub const PROVIDER_META_KEY: &str = "provider";
/// The host and port a model's requests go to.
pub const ENDPOINT_META_KEY: &str = "endpoint";

fn string_meta<'a>(
    meta: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<&'a str> {
    meta.and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// The provider id in this model's ACP meta, if it routes through one.
pub fn provider_meta(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<&str> {
    string_meta(meta, PROVIDER_META_KEY)
}

/// The endpoint host in this model's ACP meta, if the shell sent one.
pub fn endpoint_meta(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<&str> {
    string_meta(meta, ENDPOINT_META_KEY)
}

/// The host and port of `url`, without the scheme or the path.
pub fn endpoint_host(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

pub fn supports_reasoning_effort_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    reasoning_effort_meta_state(meta) == ReasoningEffortMetaState::Supported
}

/// What the effort gate actually found when it read a model's ACP `meta`.
///
/// The gate is one key read, so every refusal below reaches the user as the same
/// "not supported". They have different causes and different fixes, and only
/// this distinction tells the two apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningEffortMetaState {
    /// `supportsReasoningEffort: true`.
    Supported,
    /// The model carries no `meta` object at all.
    NoMeta,
    /// `meta` is present and carries no `supportsReasoningEffort` key. This is
    /// what a model the shell never flagged looks like: the writer omits the key
    /// rather than writing `false`.
    KeyAbsent,
    /// `supportsReasoningEffort: false` — written by something that decided
    /// against support, not by an omission.
    ExplicitlyFalse,
    /// The key holds something that is not a bool, so the gate reads it as no.
    NotABool { found: String },
}

impl ReasoningEffortMetaState {
    /// One clause naming what the gate read, for an error the user must debug.
    pub fn describe(&self) -> String {
        match self {
            Self::Supported => format!("`{SUPPORTS_REASONING_EFFORT_META_KEY}` is true"),
            Self::NoMeta => "the catalog entry carries no `meta` object at all".to_string(),
            Self::KeyAbsent => format!(
                "`meta` is present and has no `{SUPPORTS_REASONING_EFFORT_META_KEY}` key \
                 (the shell omits the key rather than writing false)"
            ),
            Self::ExplicitlyFalse => {
                format!("`meta.{SUPPORTS_REASONING_EFFORT_META_KEY}` is explicitly false")
            }
            Self::NotABool { found } => format!(
                "`meta.{SUPPORTS_REASONING_EFFORT_META_KEY}` is {found}, not a bool, \
                 so the gate reads it as false"
            ),
        }
    }
}

/// Read the effort gate's input and report exactly what was there.
pub fn reasoning_effort_meta_state(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> ReasoningEffortMetaState {
    let Some(meta) = meta else {
        return ReasoningEffortMetaState::NoMeta;
    };
    match meta.get(SUPPORTS_REASONING_EFFORT_META_KEY) {
        None => ReasoningEffortMetaState::KeyAbsent,
        Some(serde_json::Value::Bool(true)) => ReasoningEffortMetaState::Supported,
        Some(serde_json::Value::Bool(false)) => ReasoningEffortMetaState::ExplicitlyFalse,
        Some(other) => ReasoningEffortMetaState::NotABool {
            found: other.to_string(),
        },
    }
}

/// Returns `None` on type-mismatch or unknown variant (logs a warn so we don't overwrite the user's persisted pref on the next save).
pub fn parse_reasoning_effort_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<ReasoningEffort> {
    let raw = meta?.get(REASONING_EFFORT_META_KEY)?;
    let s = match raw.as_str() {
        Some(s) => s,
        None => {
            tracing::warn!(value = %raw, "meta.reasoningEffort: expected string, ignoring");
            return None;
        }
    };
    match s.parse() {
        Ok(eff) => Some(eff),
        Err(err) => {
            tracing::warn!(value = %s, error = %err, "meta.reasoningEffort: parse failed, ignoring");
            None
        }
    }
}

pub fn reasoning_effort_meta_value(effort: ReasoningEffort) -> serde_json::Value {
    serde_json::Value::String(effort.as_ref().to_string())
}

pub const REASONING_EFFORTS_META_KEY: &str = "reasoningEfforts";

/// A single selectable reasoning-effort option for a model.
/// `id`/`label` are presentation and input; `value` is the canonical value sent on the wire.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ReasoningEffortOption {
    pub id: String,
    pub value: ReasoningEffort,
    pub label: String,
    pub description: Option<String>,
    pub default: bool,
}

/// Deserialization shape accepting either a bare canonical value string (`"xhigh"`) or a table with `value` required and everything else optional.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawReasoningEffortOption {
    Bare(String),
    Full {
        value: ReasoningEffort,
        id: Option<String>,
        label: Option<String>,
        description: Option<String>,
        #[serde(default)]
        default: bool,
    },
}

/// Display label for a known level; the bare-string menu shorthand and the shell's built-in effort picker share it.
pub fn effort_label(effort: ReasoningEffort) -> String {
    match effort {
        ReasoningEffort::None => "None",
        ReasoningEffort::Minimal => "Minimal",
        ReasoningEffort::Low => "Low",
        ReasoningEffort::Medium => "Medium",
        ReasoningEffort::High => "High",
        ReasoningEffort::Xhigh => "X-High",
        ReasoningEffort::Max => "Max",
    }
    .to_string()
}

/// Uppercase the first character of a custom id for a default label; `"deep"` becomes `"Deep"`.
fn humanize_effort_id(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl<'de> serde::Deserialize<'de> for ReasoningEffortOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match RawReasoningEffortOption::deserialize(deserializer)? {
            RawReasoningEffortOption::Bare(s) => {
                let value = s
                    .parse::<ReasoningEffort>()
                    .map_err(serde::de::Error::custom)?;
                ReasoningEffortOption {
                    id: value.as_ref().to_string(),
                    value,
                    label: effort_label(value),
                    description: None,
                    default: false,
                }
            }
            RawReasoningEffortOption::Full {
                value,
                id,
                label,
                description,
                default,
            } => {
                let label = label.unwrap_or_else(|| match &id {
                    Some(id) => humanize_effort_id(id),
                    None => effort_label(value),
                });
                let id = id.unwrap_or_else(|| value.as_ref().to_string());
                ReasoningEffortOption {
                    id,
                    value,
                    label,
                    description,
                    default,
                }
            }
        })
    }
}

/// Parse a JSON array of reasoning-effort options element-by-element, skipping and warning on any entry whose `value` fails to parse.
/// That keeps tiers a newer server introduces from breaking the whole list.
/// The meta reader and the remote `/models` parser both call this, so the skip rule lives in one place; `field` names the source key in the warn.
pub fn parse_reasoning_effort_options(
    arr: &[serde_json::Value],
    field: &str,
) -> Vec<ReasoningEffortOption> {
    arr.iter()
        .filter_map(
            |el| match serde_json::from_value::<ReasoningEffortOption>(el.clone()) {
                Ok(opt) => Some(opt),
                Err(err) => {
                    tracing::warn!(value = %el, error = %err, "{field}: skipping invalid entry");
                    None
                }
            },
        )
        .collect()
}

/// Parse the per-model reasoning-effort menu from a model's ACP `meta`.
/// Returns `None` when the key is absent, is not an array, or yields no usable options.
/// An absent key and an unusable one therefore collapse to the same fallback path in every consumer.
pub fn parse_reasoning_efforts_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<Vec<ReasoningEffortOption>> {
    let raw = meta?.get(REASONING_EFFORTS_META_KEY)?;
    let arr = match raw.as_array() {
        Some(arr) => arr,
        None => {
            tracing::warn!(value = %raw, "meta.reasoningEfforts: expected array, ignoring");
            return None;
        }
    };
    let options = parse_reasoning_effort_options(arr, REASONING_EFFORTS_META_KEY);
    (!options.is_empty()).then_some(options)
}

pub fn reasoning_efforts_meta_value(opts: &[ReasoningEffortOption]) -> serde_json::Value {
    serde_json::to_value(opts).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiBackend {
    /// Use the Chat Completions API (/v1/chat/completions)
    #[default]
    ChatCompletions,
    /// Use the Responses API (/v1/responses)
    Responses,
    /// Use the Anthropic Messages API (/v1/messages)
    Messages,
    /// Use Ollama's native chat API (/api/chat).
    ///
    /// Ollama also serves an OpenAI-compatible endpoint, and that one is the
    /// default for it. This backend exists for the three fields the compat
    /// endpoint cannot carry: `options.num_ctx` (the window the runner loads
    /// at), `keep_alive` (residency) and `truncate` (whether the server may
    /// silently drop the head of the conversation).
    Ollama,
}

impl ApiBackend {
    /// Whether the backend enforces a response JSON schema natively alongside tool calls.
    /// The Messages API does not (a schema there blocks tool use), so structured output there goes through the StructuredOutput tool.
    pub fn supports_native_schema(&self) -> bool {
        // Ollama's `format` takes a bare JSON schema and enforces it
        // alongside tool calls, so it belongs with the two that do.
        matches!(self, Self::ChatCompletions | Self::Responses | Self::Ollama)
    }

    /// Whether [`ConversationRequest::prompt_cache_key`] reaches the wire. Only the Responses mapping sends it, so a key set elsewhere is inert.
    ///
    /// [`ConversationRequest::prompt_cache_key`]: crate::conversation::ConversationRequest::prompt_cache_key
    pub fn forwards_prompt_cache_key(&self) -> bool {
        matches!(self, Self::Responses)
    }

    /// Request-body cap the hosts speaking this protocol enforce; the budget when a model sets no `max_request_bytes`.
    /// The xAI inference proxy rejects bodies over 50 MiB (nginx `proxy-body-size`); Messages API hosts reject bodies over 30 MB.
    pub const fn default_max_request_bytes(&self) -> NonZeroU64 {
        match self {
            Self::ChatCompletions | Self::Responses | Self::Ollama => {
                NonZeroU64::new(50 * 1024 * 1024).unwrap()
            }
            Self::Messages => NonZeroU64::new(30_000_000).unwrap(),
        }
    }
}

/// Stable identifier shared by every model request in one root conversation tree.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationGroupId(String);

impl AsRef<str> for ConversationGroupId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConversationGroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ConversationGroupId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ConversationGroupId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Which optional message-level properties a Chat Completions target's schema
/// accepts on replayed messages.
///
/// Most OpenAI-compatible providers ignore unknown message properties, so the
/// defaults here are permissive — [`Self::PERMISSIVE`], exactly the body this
/// crate sent before this type existed. A provider that validates its message
/// schema strictly (Cerebras answers an unrecognized property with
/// `wrong_api_format ... is unsupported`) needs [`Self::STRICT`], which omits
/// the properties entirely rather than sending them as null/empty.
///
/// Pure data: no I/O, no provider knowledge. The per-model config surface
/// selects it; the wire conversion consults it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatMessageProfile {
    /// Whether the target accepts `model_id` on replayed messages.
    pub accepts_model_id: bool,
    /// Whether the target accepts `reasoning_content` on replayed messages.
    pub accepts_reasoning_content: bool,
}

impl ChatMessageProfile {
    /// Today's behavior: emit both properties. This is what every provider
    /// except a strict-schema one expects.
    pub const PERMISSIVE: Self = Self {
        accepts_model_id: true,
        accepts_reasoning_content: true,
    };

    /// A target whose schema defines neither property: omit both.
    pub const STRICT: Self = Self {
        accepts_model_id: false,
        accepts_reasoning_content: false,
    };

    /// Whether this profile emits both properties (i.e. nothing is suppressed).
    pub fn is_permissive(&self) -> bool {
        self.accepts_model_id && self.accepts_reasoning_content
    }

    /// Narrow `self` by `other`: a property rejected by either side is dropped.
    ///
    /// Used to combine a request's profile with the per-model config's, so a
    /// model configured as strict cannot be re-widened by a caller that left
    /// the request at the permissive default.
    pub fn narrowed_by(self, other: Self) -> Self {
        Self {
            accepts_model_id: self.accepts_model_id && other.accepts_model_id,
            accepts_reasoning_content: self.accepts_reasoning_content
                && other.accepts_reasoning_content,
        }
    }

    /// Drop the properties named as unsupported by a provider error, if any.
    /// Returns `None` when nothing changed, so a retry loop can tell a
    /// productive strip from a no-op.
    pub fn strip_named(&mut self, names_model_id: bool, names_reasoning_content: bool) -> bool {
        let before = *self;
        if names_model_id {
            self.accepts_model_id = false;
        }
        if names_reasoning_content {
            self.accepts_reasoning_content = false;
        }
        *self != before
    }
}

impl Default for ChatMessageProfile {
    /// Permissive, deliberately: `ConversationRequest` and `SamplingConfig`
    /// both derive/lean on `Default`, so a strict default would silently
    /// reshape every existing provider's request body.
    fn default() -> Self {
        Self::PERMISSIVE
    }
}

/// Sampling client configuration (API key excluded; that stays in the client).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SamplingConfig {
    pub base_url: String,
    /// Local directory containing the mTLS client identity for this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtls_cert_dir: Option<std::path::PathBuf>,
    pub model: String,
    pub max_completion_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    /// Model-resolved general retry budget paired with the rate-limit ceiling below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Model-resolved total-attempt ceiling for rate-limited requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_retry_threshold: Option<u32>,
    /// Which API backend to use for this model
    #[serde(default)]
    pub api_backend: ApiBackend,
    /// Extra headers to send with requests (e.g., for bring-your-own-key (BYOK) scenarios).
    #[serde(default, skip_serializing_if = "indexmap::IndexMap::is_empty")]
    pub extra_headers: indexmap::IndexMap<String, String>,
    /// Root conversation group propagated across model changes and child sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_group_id: Option<ConversationGroupId>,
    /// Query parameters folded into every request URL (percent-encoded).
    #[serde(default, skip_serializing_if = "indexmap::IndexMap::is_empty")]
    pub query_params: indexmap::IndexMap<String, String>,
    /// Header name to environment variable; only the mapping persists, not the resolved secret.
    #[serde(default, skip_serializing_if = "indexmap::IndexMap::is_empty")]
    pub env_http_headers: indexmap::IndexMap<String, String>,
    /// Extra top-level fields merged into every request body for this model
    /// (`[model.<id>].extra_body`). Carries the per-deployment settings a
    /// closed request struct has no field for, such as a local runtime's
    /// residency and context-length knobs.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    /// Total context window size in tokens; auto-compact thresholds derive from it.
    pub context_window: NonZeroU64,
    /// Provider request-body cap, already defaulted from `api_backend` by model resolution; `None` budgets to 50 MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<NonZeroU64>,
    /// Reasoning effort level for reasoning models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Which optional message properties this target's schema accepts.
    /// Defaults to [`ChatMessageProfile::PERMISSIVE`] (today's behavior).
    #[serde(default)]
    pub chat_message_profile: ChatMessageProfile,
    /// Responses API `reasoning.summary`; `None` keeps the request builder's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_summary: Option<ReasoningSummary>,
    /// When true, inject `stream_tool_calls: true` into the Responses API request body so the upstream emits per-chunk argument deltas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_tool_calls: Option<bool>,
}

impl Default for SamplingConfig {
    /// Empty defaults so construction sites (tests especially) can use `..Default::default()` and new fields don't ripple through every literal.
    /// `context_window` defaults to the inert minimum; real configs must set it.
    fn default() -> Self {
        Self {
            base_url: String::new(),
            mtls_cert_dir: None,
            model: String::new(),
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            max_retries: None,
            rate_limit_retry_threshold: None,
            api_backend: ApiBackend::default(),
            extra_headers: indexmap::IndexMap::new(),
            conversation_group_id: None,
            query_params: indexmap::IndexMap::new(),
            env_http_headers: indexmap::IndexMap::new(),
            extra_body: serde_json::Map::new(),
            context_window: NonZeroU64::MIN,
            max_request_bytes: None,
            reasoning_effort: None,
            chat_message_profile: ChatMessageProfile::default(),
            reasoning_summary: None,
            stream_tool_calls: None,
        }
    }
}

// ============ Responses API wrapper ============

/// Wrapper around `async_openai::types::responses::CreateResponse` that adds custom header fields for xAI request tracking.
/// It mirrors the header fields on `ChatCompletionRequest`.
#[derive(Debug, Clone, Default)]
pub struct CreateResponseWrapper {
    /// The inner Responses API request.
    pub inner: crate::rs::CreateResponse,

    /// Custom header: conversation ID for tracking.
    pub x_grok_conv_id: Option<String>,

    /// Custom header: request ID for tracking.
    pub x_grok_req_id: Option<String>,

    pub x_grok_session_id: Option<String>,
    pub x_grok_turn_idx: Option<String>,
    pub x_grok_transient_retry: Option<String>,
    pub x_grok_agent_id: Option<String>,
    pub x_grok_deployment_id: Option<String>,
    pub x_grok_user_id: Option<String>,

    /// Optional tracing context (e.g., where to persist the finalized request payload).
    pub trace: Option<Box<dyn TraceContext>>,
    /// Caller span's W3C `traceparent`; see [`crate::ConversationRequest::traceparent`].
    pub traceparent: Option<String>,

    /// xAI-specific tool definitions that can't be expressed via `async_openai`'s `rs::Tool` enum (e.g., `x_search`).
    /// They are injected as raw JSON into the serialized request body's `tools` array.
    pub extra_tool_entries: Vec<serde_json::Value>,
}

impl CreateResponseWrapper {
    pub fn new(inner: crate::rs::CreateResponse) -> Self {
        Self {
            inner,
            x_grok_conv_id: None,
            x_grok_req_id: None,
            x_grok_session_id: None,
            x_grok_turn_idx: None,
            x_grok_transient_retry: None,
            x_grok_agent_id: None,
            x_grok_deployment_id: None,
            x_grok_user_id: None,
            trace: None,
            traceparent: None,
            extra_tool_entries: vec![],
        }
    }

    pub fn with_conv_id(mut self, conv_id: impl Into<String>) -> Self {
        self.x_grok_conv_id = Some(conv_id.into());
        self
    }

    pub fn with_req_id(mut self, req_id: impl Into<String>) -> Self {
        self.x_grok_req_id = Some(req_id.into());
        self
    }

    pub fn with_trace(mut self, trace: impl TraceContext + 'static) -> Self {
        self.trace = Some(Box::new(trace));
        self
    }
}

impl From<crate::rs::CreateResponse> for CreateResponseWrapper {
    fn from(inner: crate::rs::CreateResponse) -> Self {
        Self::new(inner)
    }
}

// ============ Messages API wrapper ============

/// Wrapper around `MessagesRequest` that adds custom header fields for xAI request tracking, analogous to `CreateResponseWrapper`.
#[derive(Debug, Clone, Default)]
pub struct MessagesRequestWrapper {
    /// The inner Messages API request.
    pub inner: crate::messages::MessagesRequest,

    /// Custom header: conversation ID for tracking.
    pub x_grok_conv_id: Option<String>,

    /// Custom header: request ID for tracking.
    pub x_grok_req_id: Option<String>,

    pub x_grok_session_id: Option<String>,
    pub x_grok_turn_idx: Option<String>,
    pub x_grok_transient_retry: Option<String>,
    pub x_grok_agent_id: Option<String>,
    pub x_grok_deployment_id: Option<String>,
    pub x_grok_user_id: Option<String>,

    /// Optional tracing context (e.g., where to persist the finalized request payload).
    pub trace: Option<Box<dyn TraceContext>>,
    /// Caller span's W3C `traceparent`; see [`crate::ConversationRequest::traceparent`].
    pub traceparent: Option<String>,
}

impl MessagesRequestWrapper {
    pub fn new(inner: crate::messages::MessagesRequest) -> Self {
        Self {
            inner,
            x_grok_conv_id: None,
            x_grok_req_id: None,
            x_grok_session_id: None,
            x_grok_turn_idx: None,
            x_grok_transient_retry: None,
            x_grok_agent_id: None,
            x_grok_deployment_id: None,
            x_grok_user_id: None,
            trace: None,
            traceparent: None,
        }
    }

    pub fn with_conv_id(mut self, conv_id: impl Into<String>) -> Self {
        self.x_grok_conv_id = Some(conv_id.into());
        self
    }

    pub fn with_req_id(mut self, req_id: impl Into<String>) -> Self {
        self.x_grok_req_id = Some(req_id.into());
        self
    }

    pub fn with_trace(mut self, trace: impl TraceContext + 'static) -> Self {
        self.trace = Some(Box::new(trace));
        self
    }
}

impl From<crate::messages::MessagesRequest> for MessagesRequestWrapper {
    fn from(inner: crate::messages::MessagesRequest) -> Self {
        Self::new(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoint_host_keeps_the_host_and_port_only() {
        assert_eq!(
            endpoint_host("http://LocalHost:18080/v1/").as_deref(),
            Some("localhost:18080")
        );
        assert_eq!(
            endpoint_host("https://user:pw@api.x.ai/v1?x=1").as_deref(),
            Some("api.x.ai")
        );
        assert_eq!(endpoint_host("").as_deref(), None);
    }

    #[test]
    fn reasoning_effort_serde_lowercase_round_trip() {
        for v in [
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, format!("\"{}\"", v.as_ref()), "serialize {v:?}");
            let back: ReasoningEffort = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v, "round-trip {v:?}");
        }
        assert!(serde_json::from_str::<ReasoningEffort>("\"BOGUS\"").is_err());
    }

    #[test]
    fn reasoning_effort_from_str_parses_max_and_xhigh_as_distinct_tiers() {
        assert_eq!(
            "max".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Max
        );
        assert_eq!(
            "xhigh".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Xhigh
        );
    }

    #[test]
    fn parse_canonical_effort_token_helper() {
        assert_eq!(
            parse_canonical_effort_token("max"),
            Some(ReasoningEffort::Max)
        );
        assert_eq!(
            parse_canonical_effort_token("high"),
            Some(ReasoningEffort::High)
        );
        assert!(parse_canonical_effort_token("deep").is_none());
        assert!(parse_canonical_effort_token("bogus").is_none());
    }

    #[test]
    fn reasoning_effort_option_deserializes_bare_string() {
        let opt: ReasoningEffortOption = serde_json::from_value(json!("xhigh")).unwrap();
        assert_eq!(
            opt,
            ReasoningEffortOption {
                id: "xhigh".to_string(),
                value: ReasoningEffort::Xhigh,
                label: "X-High".to_string(),
                description: None,
                default: false,
            }
        );
    }

    #[test]
    fn reasoning_effort_option_table_defaults_id_and_label_from_value() {
        let opt: ReasoningEffortOption =
            serde_json::from_value(json!({ "value": "high" })).unwrap();
        assert_eq!(opt.id, "high");
        assert_eq!(opt.label, "High");
        assert_eq!(opt.value, ReasoningEffort::High);
        assert!(!opt.default);
    }

    #[test]
    fn reasoning_effort_option_table_honors_explicit_fields() {
        let opt: ReasoningEffortOption = serde_json::from_value(json!({
            "id": "deep",
            "value": "xhigh",
            "label": "Deep",
            "description": "Maximum reasoning",
            "default": true,
        }))
        .unwrap();
        assert_eq!(opt.id, "deep");
        assert_eq!(opt.value, ReasoningEffort::Xhigh);
        assert_eq!(opt.label, "Deep");
        assert_eq!(opt.description.as_deref(), Some("Maximum reasoning"));
        assert!(opt.default);
    }

    #[test]
    fn parse_reasoning_efforts_meta_absent_is_none() {
        assert!(parse_reasoning_efforts_meta(None).is_none());
        assert!(
            parse_reasoning_efforts_meta(Some(json!({ "agentType": "grok" }).as_object().unwrap()))
                .is_none()
        );
    }

    #[test]
    fn parse_reasoning_efforts_meta_skips_invalid_value() {
        let meta = json!({
            REASONING_EFFORTS_META_KEY: [
                { "value": "high" },
                { "value": "quantum" },
                "low",
            ]
        })
        .as_object()
        .cloned()
        .unwrap();
        let parsed = parse_reasoning_efforts_meta(Some(&meta)).unwrap();
        let [high, low] = parsed.as_slice() else {
            panic!("expected two efforts: {parsed:?}");
        };
        assert_eq!(high.value, ReasoningEffort::High);
        assert_eq!(low.value, ReasoningEffort::Low);
    }

    #[test]
    fn parse_reasoning_efforts_meta_present_but_unusable_is_none() {
        // An empty array, a non-array, and an array whose every entry fails to parse all collapse to `None`
        // Consumers then fall back exactly as they do for an absent key
        for meta in [
            json!({ REASONING_EFFORTS_META_KEY: [] }),
            json!({ REASONING_EFFORTS_META_KEY: "nope" }),
            json!({ REASONING_EFFORTS_META_KEY: [{ "value": "quantum" }] }),
        ] {
            let meta = meta.as_object().cloned().unwrap();
            assert!(
                parse_reasoning_efforts_meta(Some(&meta)).is_none(),
                "expected None for {meta:?}"
            );
        }
    }

    #[test]
    fn reasoning_efforts_meta_value_round_trips() {
        let opts = vec![
            ReasoningEffortOption {
                id: "deep".to_string(),
                value: ReasoningEffort::Xhigh,
                label: "Deep".to_string(),
                description: Some("Maximum reasoning".to_string()),
                default: true,
            },
            ReasoningEffortOption {
                id: "balanced".to_string(),
                value: ReasoningEffort::Medium,
                label: "Balanced".to_string(),
                description: None,
                default: false,
            },
        ];
        let meta = json!({ REASONING_EFFORTS_META_KEY: reasoning_efforts_meta_value(&opts) })
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(parse_reasoning_efforts_meta(Some(&meta)).unwrap(), opts);
    }

    #[test]
    fn compactions_remaining_resolve_covers_all_variants() {
        assert_eq!(CompactionsRemaining::Dynamic(false).resolve(false), None);
        assert_eq!(CompactionsRemaining::Dynamic(false).resolve(true), None);
        assert_eq!(CompactionsRemaining::Dynamic(true).resolve(false), Some(1));
        assert_eq!(CompactionsRemaining::Dynamic(true).resolve(true), Some(0));
        assert_eq!(CompactionsRemaining::Fixed(1).resolve(false), Some(1));
        assert_eq!(CompactionsRemaining::Fixed(1).resolve(true), Some(1));
    }

    #[test]
    fn compactions_remaining_untagged_serde_prefers_dynamic_for_bools() {
        assert_eq!(
            serde_json::from_str::<CompactionsRemaining>("true").unwrap(),
            CompactionsRemaining::Dynamic(true)
        );
        assert_eq!(
            serde_json::from_str::<CompactionsRemaining>("false").unwrap(),
            CompactionsRemaining::Dynamic(false)
        );
        assert_eq!(
            serde_json::from_str::<CompactionsRemaining>("1").unwrap(),
            CompactionsRemaining::Fixed(1)
        );
    }

    #[test]
    fn parse_reasoning_effort_meta_handles_all_inputs() {
        let as_map = |v: serde_json::Value| v.as_object().cloned().unwrap();
        assert_eq!(parse_reasoning_effort_meta(None), None);
        let empty = as_map(serde_json::json!({}));
        assert_eq!(parse_reasoning_effort_meta(Some(&empty)), None);
        let ok = as_map(serde_json::json!({"reasoningEffort": "xhigh"}));
        assert_eq!(
            parse_reasoning_effort_meta(Some(&ok)),
            Some(ReasoningEffort::Xhigh)
        );
        let bad_type = as_map(serde_json::json!({"reasoningEffort": 3}));
        assert_eq!(parse_reasoning_effort_meta(Some(&bad_type)), None);
        let unknown = as_map(serde_json::json!({"reasoningEffort": "ULTRA"}));
        assert_eq!(parse_reasoning_effort_meta(Some(&unknown)), None);
    }

    #[test]
    fn test_chat_text_content_serialization() {
        let test = vec![ChatContentBlock::Text {
            text: "Hello World!".to_string(),
        }];

        let json = serde_json::to_string(&test).unwrap();
        assert_eq!(json, r#"[{"type":"text","text":"Hello World!"}]"#);
    }

    #[test]
    fn test_chat_all_content_serialization() {
        let test = vec![
            ChatContentBlock::ImageUrl {
                image_url: ImageUrl {
                    url: "https://www.test.com".to_string(),
                },
            },
            ChatContentBlock::Text {
                text: "Hello".to_string(),
            },
        ];

        let json = serde_json::to_string(&test).unwrap();
        assert_eq!(
            json,
            r#"[{"type":"image_url","image_url":{"url":"https://www.test.com"}},{"type":"text","text":"Hello"}]"#
        );
    }

    #[test]
    fn test_content_string_deserialization() {
        let expected_contents = vec!["", "Hello world!"];

        for expected_content in expected_contents {
            let json = format!(r#"{{"content":"{}","role":"assistant"}}"#, expected_content);

            let msg: ChatRequestMessage = serde_json::from_str(&json)
                .unwrap_or_else(|_| panic!("Should deserialize {}", expected_content));

            let blocks = msg.content.blocks();
            assert_eq!(blocks.len(), 1);
            match blocks.first() {
                Some(ChatContentBlock::Text { text }) => assert_eq!(text, expected_content),
                other => panic!("Expected empty Text block, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_chat_chunk_delta_deserialize_with_null_tool_calls() {
        let delta_json = r#"{
            "reasoning": null,
            "reasoning_details": [],
            "content": "",
            "function_call": null,
            "refusal": null,
            "role": "assistant",
            "tool_calls": null
        }"#;

        let result = serde_json::from_str::<ChatChunkDelta>(delta_json);
        assert!(result.is_ok(), "Failed to deserialize: {:?}", result.err());

        let delta = result.unwrap();
        assert_eq!(delta.role, Some(Role::Assistant));
        assert_eq!(delta.content, Some("".to_string()));
        assert!(delta.tool_calls.is_empty());
    }

    /// The exact chunk Bifrost sends to close a stream: usage arrives with no
    /// choices, and Go marshals the unset slice as `null`. Rejecting it failed
    /// every turn on every model behind the gateway with "Couldn't read the
    /// response -- serialization error: invalid type: null, expected a sequence".
    #[test]
    fn chat_completion_chunk_deserializes_null_choices() {
        let chunk: ChatCompletionChunk = serde_json::from_str(
            r#"{
                "id": "chatcmpl-1",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "grok-4",
                "choices": null,
                "system_fingerprint": "",
                "usage": {"prompt_tokens": 18, "completion_tokens": 10, "total_tokens": 28}
            }"#,
        )
        .expect("a null `choices` must not fail the chunk");

        assert!(chunk.choices.is_empty());
        // The usage riding on that chunk is the whole reason to keep it.
        assert_eq!(chunk.usage.map(|u| u.total_tokens), Some(28));
    }

    #[test]
    fn chat_completion_response_deserializes_null_choices_and_tool_calls() {
        let response: ChatCompletionResponse = serde_json::from_str(
            r#"{
                "id": "chatcmpl-2",
                "object": "chat.completion",
                "created": 1,
                "model": "grok-4",
                "choices": null
            }"#,
        )
        .expect("a null `choices` must not fail the response");
        assert!(response.choices.is_empty());

        let message: ChatResponseMessage =
            serde_json::from_str(r#"{"role": "assistant", "content": "hi", "tool_calls": null}"#)
                .expect("a null `tool_calls` must not fail the message");
        assert!(message.tool_calls.is_empty());
    }

    /// Regression test: cloning `Box<dyn TraceContext>` must not infinitely recurse. Without the dereference in `Clone for
    /// Box<dyn TraceContext>`, `self.clone_box()` resolves to the blanket impl's method via auto-deref. That skips vtable
    /// dispatch, so `clone()` calls `clone_box()` calls `clone()` until the stack overflows.
    #[test]
    fn clone_box_dyn_trace_context_does_not_recurse() {
        #[derive(Debug, Clone)]
        struct TestTrace(String);

        let trace: Box<dyn TraceContext> = Box::new(TestTrace("hello".into()));
        let cloned = trace.clone();

        // Verify the clone produced a valid TraceContext with the same data.
        // `as_any()` must be called through `&dyn TraceContext` (not on the Box directly) to use vtable dispatch rather than the blanket impl
        let inner: &dyn TraceContext = &*trace;
        let original = inner.as_any().downcast_ref::<TestTrace>().unwrap();

        let cloned_inner_ref: &dyn TraceContext = &*cloned;
        let cloned_inner = cloned_inner_ref
            .as_any()
            .downcast_ref::<TestTrace>()
            .unwrap();
        assert_eq!(original.0, cloned_inner.0);
    }

    // ========================================================================
    // usd_float_to_ticks — convert provider USD float to integer ticks
    // ========================================================================

    #[test]
    fn usd_float_to_ticks_converts_correctly() {
        // $0.0000416 → round(0.0000416 * 1e10) = 416_000
        assert_eq!(usd_float_to_ticks(Some(0.0000416)), Some(416_000));
        // $1.00 → 1e10 ticks
        assert_eq!(usd_float_to_ticks(Some(1.0)), Some(10_000_000_000));
    }

    #[test]
    fn usd_float_to_ticks_none_for_non_positive() {
        assert_eq!(usd_float_to_ticks(Some(0.0)), None);
        assert_eq!(usd_float_to_ticks(Some(-1.0)), None);
        assert_eq!(usd_float_to_ticks(None), None);
    }

    #[test]
    fn usd_float_to_ticks_none_for_nan_inf() {
        assert_eq!(usd_float_to_ticks(Some(f64::NAN)), None);
        assert_eq!(usd_float_to_ticks(Some(f64::INFINITY)), None);
    }

    #[test]
    fn usage_struct_deserializes_openrouter_cost_float() {
        // The wire shape OpenRouter emits: usage.cost as a USD float,
        // no cost_in_usd_ticks.
        let json = json!({
            "prompt_tokens": 18,
            "completion_tokens": 10,
            "total_tokens": 28,
            "cost": 6.92e-05
        });
        let usage: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.prompt_tokens, 18);
        assert_eq!(usage.completion_tokens, 10);
        assert_eq!(
            usage.cost.as_ref().map(|c| c.as_usd_float()),
            Some(6.92e-05)
        );
        assert_eq!(usage.cost_in_usd_ticks, None);
    }

    #[test]
    fn usage_struct_deserializes_bifrost_cost_object() {
        // Bifrost re-serializes the upstream `BifrostCost` as an object with
        // `total_cost` (and optional per-tier breakdown). The deserializer
        // must accept this shape and expose the same USD float.
        let json = json!({
            "prompt_tokens": 18,
            "completion_tokens": 10,
            "total_tokens": 28,
            "cost": {
                "input_tokens_cost": 0.0000012,
                "output_tokens_cost": 0.0000034,
                "reasoning_tokens_cost": 0.0,
                "total_cost": 0.0000046
            }
        });
        let usage: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(
            usage.cost.as_ref().map(|c| c.as_usd_float()),
            Some(0.0000046)
        );
    }

    #[test]
    fn usage_struct_deserializes_bifrost_cost_object_without_total() {
        // When the gateway omits `total_cost`, the deserializer falls back to
        // the sum of the per-tier component costs.
        let json = json!({
            "prompt_tokens": 18,
            "completion_tokens": 10,
            "total_tokens": 28,
            "cost": {
                "input_tokens_cost": 0.0000012,
                "output_tokens_cost": 0.0000034
            }
        });
        let usage: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(
            usage.cost.as_ref().map(|c| c.as_usd_float()),
            Some(0.0000046)
        );
    }

    /// Verify that cloning a `ChatCompletionRequest` with a trace does not recurse.
    #[test]
    fn clone_chat_completion_request_with_trace() {
        #[derive(Debug, Clone)]
        struct TestTrace(String);

        let mut request = ChatCompletionRequest::new("test-model", vec![]);
        request.trace = Some(Box::new(TestTrace("trace-data".into())));

        let cloned = request.clone();
        assert!(cloned.trace.is_some());

        let cloned_trace = cloned.trace.unwrap();
        let inner: &dyn TraceContext = &*cloned_trace;
        let downcast = inner.as_any().downcast_ref::<TestTrace>().unwrap();
        assert_eq!(downcast.0, "trace-data");
    }
}
