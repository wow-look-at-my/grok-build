//! Ollama native `/api/chat` wire format.
//!
//! The OpenAI-compatible endpoint Ollama also serves drops the three fields an
//! agent needs most: `options.num_ctx` (the window the runner is loaded at),
//! `keep_alive` (residency between turns) and `truncate` (whether the server
//! may silently drop the head of the conversation). Its request struct has no
//! `options` member at all, and unknown body fields are discarded by Go's JSON
//! decoder, so there is no way to smuggle them through. This module is that
//! endpoint's replacement.
//!
//! The response is NDJSON, not SSE: one complete JSON object per line, and the
//! last one carries `done: true` plus the run's metrics. See
//! [`crate::ollama::OllamaChatChunk`].

use serde::{Deserialize, Serialize};

/// A request to `POST /api/chat`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    /// Always true here: the sampler drives a stream on every call.
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OllamaTool>>,
    /// `false`, `true`, or a model-defined level (`"low"`/`"medium"`/`"high"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<serde_json::Value>,
    /// `"json"` or a JSON schema. Ollama's structured-output knob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<serde_json::Value>,
    /// How long the model stays resident after this request (`"30m"`, `0`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<serde_json::Value>,
    /// When `false` the server refuses a prompt that overflows the context
    /// instead of dropping the oldest messages. An agent needs the error: a
    /// silently truncated head loses the system prompt and orphans tool calls,
    /// and nothing on the wire says it happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncate: Option<bool>,
    /// Runner and sampling options. `num_ctx` is the load-bearing one.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// One message in either direction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OllamaMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// The model's reasoning text. Unsigned plain text, unlike the Messages
    /// API's blob, so replaying it to another model is always safe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Base64 image payloads for a multimodal model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OllamaToolCall>,
    /// Names the tool a `role: "tool"` message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OllamaToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: OllamaToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OllamaToolCallFunction {
    pub name: String,
    /// An OBJECT on this wire, not the JSON-encoded string every
    /// OpenAI-shaped API uses. Conversion happens at the boundary in both
    /// directions.
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OllamaTool {
    pub r#type: String,
    pub function: OllamaToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OllamaToolFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

/// One NDJSON line of a `/api/chat` response.
///
/// Every line carries `done`. The final one carries the metrics, which is the
/// only place `load_duration` appears — the compat endpoint's `usage` cannot
/// express it, and it is what separates a cold model load from a stalled
/// engine.
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
pub struct OllamaChatChunk {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub message: Option<OllamaMessage>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    /// Ollama reports a mid-stream failure as a line with only this field.
    #[serde(default)]
    pub error: Option<String>,

    // ── Metrics, present on the final line only. Nanoseconds. ──
    #[serde(default)]
    pub total_duration: Option<u64>,
    /// Time spent loading the model into VRAM. Outside generation entirely.
    #[serde(default)]
    pub load_duration: Option<u64>,
    #[serde(default)]
    pub prompt_eval_count: Option<u32>,
    /// The prefix the runner reused rather than re-evaluated.
    #[serde(default)]
    pub prompt_eval_cached_count: Option<u32>,
    #[serde(default)]
    pub prompt_eval_duration: Option<u64>,
    #[serde(default)]
    pub eval_count: Option<u32>,
    #[serde(default)]
    pub eval_duration: Option<u64>,
}

/// One entry of `GET /api/tags` (models on disk).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaTagEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub details: Option<OllamaModelDetails>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaModelDetails {
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub parameter_size: Option<String>,
    #[serde(default)]
    pub quantization_level: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaTagsResponse {
    #[serde(default)]
    pub models: Vec<OllamaTagEntry>,
}

/// `POST /api/show` — capabilities and the architecture's true context length.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaShowResponse {
    /// `completion`, `tools`, `vision`, `thinking`, `insert`, `embedding`, …
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// GGUF metadata. The window lives at `<architecture>.context_length`,
    /// where the architecture is itself a value under `general.architecture`.
    #[serde(default)]
    pub model_info: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub details: Option<OllamaModelDetails>,
}

impl OllamaShowResponse {
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// The model's maximum context length, read through `general.architecture`.
    ///
    /// The key is architecture-qualified (`llama.context_length`,
    /// `qwen2.context_length`), so the architecture has to be read first. A
    /// scan for any `*.context_length` would also match a projector's or an
    /// adapter's, which describe a different tensor entirely.
    pub fn max_context_length(&self) -> Option<u64> {
        let arch = self.model_info.get("general.architecture")?.as_str()?;
        self.model_info
            .get(&format!("{arch}.context_length"))?
            .as_u64()
            .filter(|&v| v > 0)
    }
}

/// One entry of `GET /api/ps` (models currently resident).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaRunningModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    /// Bytes resident in VRAM. Zero means the runner is on the CPU.
    #[serde(default)]
    pub size_vram: Option<u64>,
    /// The window the runner was actually LOADED at, which is the number
    /// compaction has to respect. It is chosen at load time from VRAM
    /// (`OLLAMA_CONTEXT_LENGTH` defaults to "4k/32k/256k based on VRAM") and is
    /// unknowable from the model's own metadata.
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct OllamaPsResponse {
    #[serde(default)]
    pub models: Vec<OllamaRunningModel>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_read_through_the_architecture_key() {
        let show: OllamaShowResponse = serde_json::from_value(serde_json::json!({
            "capabilities": ["completion", "tools", "thinking"],
            "model_info": {
                "general.architecture": "qwen2",
                "qwen2.context_length": 131072,
                // A projector's own length must never be mistaken for the
                // model's.
                "clip.context_length": 77,
            },
        }))
        .expect("show response");

        assert_eq!(show.max_context_length(), Some(131072));
        assert!(show.has_capability("tools"));
        assert!(show.has_capability("thinking"));
        assert!(!show.has_capability("vision"));
    }

    #[test]
    fn a_model_info_without_its_architecture_names_no_window() {
        let show: OllamaShowResponse = serde_json::from_value(serde_json::json!({
            "model_info": { "llama.context_length": 8192 },
        }))
        .expect("show response");

        assert_eq!(
            show.max_context_length(),
            None,
            "an unqualified scan would take a window this document never claimed"
        );
    }

    #[test]
    fn a_final_chunk_carries_its_metrics() {
        let chunk: OllamaChatChunk = serde_json::from_str(
            r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,
                "done_reason":"stop","total_duration":4883583458,"load_duration":1334875,
                "prompt_eval_count":26,"eval_count":282}"#,
        )
        .expect("final chunk");

        assert!(chunk.done);
        assert_eq!(chunk.load_duration, Some(1_334_875));
        assert_eq!(chunk.prompt_eval_count, Some(26));
        assert_eq!(chunk.eval_count, Some(282));
    }

    #[test]
    fn an_error_line_parses_as_an_error() {
        let chunk: OllamaChatChunk =
            serde_json::from_str(r#"{"error":"model requires more system memory"}"#)
                .expect("error line");

        assert_eq!(
            chunk.error.as_deref(),
            Some("model requires more system memory")
        );
        assert!(!chunk.done);
    }
}
