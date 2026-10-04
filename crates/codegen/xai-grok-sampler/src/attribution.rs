//! The caller wires an [`Auth401AttributionCallback`] into [`crate::SamplerConfig::attribution_callback`].

use std::sync::Arc;

pub use xai_grok_auth::bearer_fragment::BEARER_SUFFIX_LEN;

/// A 401-emitting site in [`crate::SamplingClient`]; its string identifier becomes the `consumer` field so queries can break 401s down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingConsumer {
    /// `chat_completion_stream`: OpenAI-compatible streaming OpenAI Chat Completions API.
    ChatCompletionsStream,
    /// `chat_completion`: OpenAI-compatible non-streaming OpenAI Chat Completions API.
    ChatCompletions,
    /// `create_response_stream`: Responses API streaming.
    ResponsesStream,
    /// `create_response`: Responses API non-streaming.
    Responses,
    /// `messages_stream`: Anthropic Messages API streaming.
    MessagesStream,
    /// `messages`: Anthropic Messages API non-streaming.
    Messages,
}

impl SamplingConsumer {
    /// Stable string identifier for this emit site.
    /// Callbacks typically combine this with a fixed prefix (e.g. the client type) when building the consumer field of the attribution event.
    pub fn as_endpoint(self) -> &'static str {
        match self {
            Self::ChatCompletionsStream => "chat_completions_stream",
            Self::ChatCompletions => "chat_completions",
            Self::ResponsesStream => "responses_stream",
            Self::Responses => "responses",
            Self::MessagesStream => "messages_stream",
            Self::Messages => "messages",
        }
    }
}

pub trait Auth401AttributionCallback: Send + Sync + std::fmt::Debug {
    /// `sent_bearer_suffix` is the [`BEARER_SUFFIX_LEN`]-char tail of the bearer sent on the wire.
    fn record_401(&self, consumer: SamplingConsumer, sent_bearer_suffix: Option<&str>);
}

/// Shared, cheap-to-clone alias for the attribution callback.
pub type SharedAttributionCallback = Arc<dyn Auth401AttributionCallback>;
