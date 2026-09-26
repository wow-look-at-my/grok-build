//! HTTP client for the xAI sampling APIs.
//!
//! Owns the `reqwest::Client`, default request headers, and per-method defaults.
//! Talks to three backend shapes:
//!
//! * Chat Completions (`/chat/completions`)
//! * Responses API (`/responses`)
//! * Anthropic Messages API (`/messages`)
//!
//! All trace-upload and URL-based header injection is intentionally *not* here.
//! The session puts per-request headers (proxy auth, OTel context, etc.) into [`SamplerConfig::extra_headers`] before constructing the client.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use futures_util::stream::{BoxStream, Stream};
use indexmap::IndexMap;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue,
    USER_AGENT,
};
use serde::Serialize;
use tracing::Instrument;

use xai_grok_sampling_types::error::{
    api_error_message_for_endpoint, error_chain, parse_error_code, try_parse_stream_error,
    user_facing_api_error_message,
};
use xai_grok_sampling_types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, ConversationRequest,
    ConversationResponse, CreateResponseWrapper, DEFAULT_EXACT_REPETITION_MIN_TOKENS,
    DOOM_LOOP_CHECK_HEADER, EXACT_REPETITION_CHECK_HEADER, MessagesRequestWrapper,
    ResponseModelMetadata, Result, SamplingError, SentCredential, build_messages_request,
    is_check_event, messages, rs,
};

use crate::config::{AuthScheme, OriginClientInfo, RequestCompression, SamplerConfig};
use crate::events::SamplingErrorInfo;
use crate::request_compression::{compress_body, should_compress};
use crate::span_timing::{ERROR, STATUS_CODE, SUCCESS, StreamSpanTiming};
use crate::stream_classify::{chat_chunk_class, message_event_class, responses_event_class};
use xai_grok_auth::bearer_suffix;

pub use xai_grok_sampling_types::ApiBackend;

/// Process-level fallback for the `x-grok-client-identifier` header.
const DEFAULT_CLIENT_IDENTIFIER: &str = "grok-shell";

/// Product identifier baked into User-Agent strings.
const AGENT_PRODUCT: &str = "grok-shell";
const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 128_000;

/// Per-request `x-grok-*` headers. Optional fields are skipped when empty/`None`.
struct GrokRequestHeaders<'a> {
    conv_id: &'a str,
    req_id: &'a str,
    model_id: &'a str,
    session_id: &'a str,
    turn_idx: Option<&'a str>,
    /// Turn-level resubmit attempt; the proxy counts retry traffic by it.
    transient_retry: Option<&'a str>,
    agent_id: &'a str,
    deployment_id: Option<&'a str>,
    user_id: Option<&'a str>,
}

impl GrokRequestHeaders<'_> {
    fn apply(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut b = builder
            .header("x-grok-conv-id", self.conv_id)
            .header("x-grok-req-id", self.req_id)
            .header("x-grok-model-override", self.model_id)
            .header("x-grok-session-id", self.session_id)
            .header("x-grok-agent-id", self.agent_id);
        if let Some(idx) = self.turn_idx {
            b = b.header("x-grok-turn-idx", idx);
        }
        if let Some(attempt) = self.transient_retry {
            b = b.header("x-grok-transient-retry", attempt);
        }
        if let Some(id) = self.deployment_id.filter(|s| !s.is_empty()) {
            b = b.header("x-grok-deployment-id", id);
        }
        if let Some(id) = self.user_id.filter(|s| !s.is_empty()) {
            b = b.header("x-grok-user-id", id);
        }
        b
    }
}

/// Deserialize a Responses SSE event, stripping unknown tools and rewriting terminal `total_tokens` from `context_details`.
pub(crate) fn deserialize_response_event(data: &str) -> Result<rs::ResponseStreamEvent> {
    let mut event = match from_sse_payload::<rs::ResponseStreamEvent>(data) {
        Ok(event) => event,
        Err(first_err) => {
            // Try sanitizing: parse as Value, strip unknown tools, retry.
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(data) {
                null_lists_as_empty(&mut value);
                // Strip tools that async_openai's rs::Tool can't deserialize (e.g., xAI-specific "x_search")
                // Instead of maintaining a hardcoded allowlist, try deserializing each tool entry; if it fails, drop it
                if let Some(tools) = value
                    .pointer_mut("/response/tools")
                    .and_then(|v| v.as_array_mut())
                {
                    tools.retain(|t| serde_json::from_value::<rs::Tool>(t.clone()).is_ok());
                }
                if let Ok(mut event) = serde_json::from_value::<rs::ResponseStreamEvent>(value) {
                    apply_terminal_event_overrides(&mut event, data);
                    return Ok(event);
                }
            }
            tracing::error!(
                error = %first_err,
                raw_data = %data,
                "Failed to deserialize ResponseStreamEvent from stream"
            );
            return Err(first_err);
        }
    };
    apply_terminal_event_overrides(&mut event, data);
    Ok(event)
}

/// Keys the wire schemas type as a list, so `null` there is a producer bug
/// rather than a value we would lose by rewriting it.
///
/// Every entry is a slice field Bifrost declares without `omitempty` (Go
/// marshals an unset slice as `null`), across its chat-completions, Responses
/// and Anthropic surfaces. The list must stay keys-that-are-lists only: the
/// same payloads carry plenty of legitimately-null pointer fields (`error`,
/// `instructions`, `reasoning`, …) that a blanket null-to-`[]` rewrite would
/// corrupt into a parse failure of its own.
const NULL_TOLERANT_LIST_KEYS: &[&str] = &[
    "annotations",
    "bytes",
    "choices",
    "command",
    "content",
    "data",
    "env",
    "filters",
    "logprobs",
    "outputs",
    "output",
    "queries",
    "summary",
    "tool_calls",
    "tools",
    "top_logprobs",
    "vector_store_ids",
];

/// Rewrite `null` to `[]` at [`NULL_TOLERANT_LIST_KEYS`], recursively. Reports
/// whether anything changed, so a caller can skip a retry that cannot differ.
///
/// A gateway written in Go marshals an unset slice as `null`, so
/// `response.created` -- whose output list is empty by definition -- arrives as
/// `"output": null` and fails the parse. Because serde buffers the internally
/// tagged event, that failure carries no line or column AND no field path: it
/// reads as a bare "invalid type: null, expected a sequence" and takes the whole
/// turn with it.
///
/// Only reached after the strict parse already failed, so a well-formed server
/// never meets this.
fn null_lists_as_empty(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for (key, child) in map.iter_mut() {
                if child.is_null() && NULL_TOLERANT_LIST_KEYS.contains(&key.as_str()) {
                    *child = serde_json::Value::Array(Vec::new());
                    changed = true;
                } else {
                    changed |= null_lists_as_empty(child);
                }
            }
            changed
        }
        serde_json::Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| null_lists_as_empty(item) || changed),
        _ => false,
    }
}

/// Dotted paths of every `null` in the payload, for a failure that named no
/// field of its own. Keys only, never values: this text reaches the user's
/// screen, and the payload is their generated content.
fn null_key_paths(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    const MAX: usize = 8;
    if out.len() >= MAX {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if child.is_null() {
                    out.push(path);
                    if out.len() >= MAX {
                        return;
                    }
                } else {
                    null_key_paths(child, &path, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                null_key_paths(item, &format!("{prefix}[{index}]"), out);
            }
        }
        _ => {}
    }
}

/// Deserialize an SSE payload, naming the field that failed.
///
/// serde_json alone renders a rejected payload as "invalid type: null, expected
/// a sequence" — and for a buffered (internally tagged) event it carries no
/// line/column either, so the message names neither the field nor the offset.
/// That is the whole error a user gets, and there is nothing in it to act on.
///
/// `serde_path_to_error` supplies the field path on a derived struct
/// (`choices[0].delta.content`). It cannot on a `#[serde(tag = "type")]` event,
/// because serde buffers the content before the variant is known and the
/// tracker never sees those keys — measured, not assumed. That is exactly the
/// shape gateways break, so for an empty path the message falls back to listing
/// where the payload's nulls are.
fn from_sse_payload<T: serde::de::DeserializeOwned>(data: &str) -> Result<T> {
    let deserializer = &mut serde_json::Deserializer::from_str(data);
    serde_path_to_error::deserialize(deserializer).map_err(|err| {
        let path = err.path().to_string();
        let inner = err.into_inner();
        if !path.is_empty() && path != "." {
            return SamplingError::serialization_message(format!("{path}: {inner}"));
        }
        let mut nulls = Vec::new();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            null_key_paths(&value, "", &mut nulls);
        }
        if nulls.is_empty() {
            SamplingError::Serialization(inner)
        } else {
            SamplingError::serialization_message(format!(
                "{inner} (null in this payload: {})",
                nulls.join(", ")
            ))
        }
    })
}

/// Strict parse, then one retry with the payload's null lists read as empty.
///
/// The retry only runs when the strict parse failed and there was something to
/// rewrite, and a retry that also fails reports the STRICT error — so a
/// malformed payload is never described in terms of the rewrite.
fn parse_sse_event<T: serde::de::DeserializeOwned>(data: &str) -> Result<T> {
    let strict = match from_sse_payload::<T>(data) {
        Ok(event) => return Ok(event),
        Err(err) => err,
    };
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(data)
        && null_lists_as_empty(&mut value)
        && let Ok(event) = serde_json::from_value::<T>(value)
    {
        return Ok(event);
    }
    Err(strict)
}

/// On `response.completed` / `response.incomplete`, rewrite `usage.total_tokens` to the live context length from `context_details`.
/// Billing fields stay on the cumulative wire values, so telemetry is unaffected.
fn apply_terminal_event_overrides(event: &mut rs::ResponseStreamEvent, data: &str) {
    let response = match event {
        rs::ResponseStreamEvent::ResponseCompleted(e) => &mut e.response,
        rs::ResponseStreamEvent::ResponseIncomplete(e) => &mut e.response,
        _ => return,
    };
    // Re-parse for fields async_openai's types omit (context total, cost ticks).
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };
    // Stash cost ticks in metadata for stream_responses. Two wire forms are
    // supported: xAI `cost_in_usd_ticks` (integer, authoritative) and the
    // standard `usage.cost` USD float (OpenRouter, etc.) converted to ticks.
    // The `usage.cost` value may be a bare float or a Bifrost cost object
    // (`{"total_cost": ...}`); both are handled via `UsageCost`.
    let ticks = xai_grok_sampling_types::reported_cost_ticks(
        value
            .pointer("/response/usage/cost_in_usd_ticks")
            .and_then(|v| v.as_i64()),
    )
    .or_else(|| {
        xai_grok_sampling_types::usd_float_to_ticks(
            value
                .pointer("/response/usage/cost")
                .and_then(|v| {
                    serde_json::from_value::<xai_grok_sampling_types::UsageCost>(v.clone()).ok()
                })
                .map(|c| c.as_usd_float()),
        )
    });
    if let Some(ticks) = ticks {
        response
            .metadata
            .get_or_insert_with(Default::default)
            .insert(COST_USD_TICKS_METADATA_KEY.to_owned(), ticks.to_string());
    }
    let Some(usage) = response.usage.as_mut() else {
        return;
    };
    let Some(total) = extract_context_total(&value) else {
        return;
    };
    usage.total_tokens = total;
}

/// Metadata key that carries cost ticks through the typed Response events, which have no field for them.
pub(crate) const COST_USD_TICKS_METADATA_KEY: &str = "xai.cost_usd_ticks";

/// Read `response.usage.context_details.{input_tokens, output_tokens}` from the parsed terminal-event JSON and return their sum.
/// Returns `None` if either field is missing or out of `u32` range.
fn extract_context_total(value: &serde_json::Value) -> Option<u32> {
    let cd = value.pointer("/response/usage/context_details")?;
    let i = u32::try_from(cd.get("input_tokens")?.as_u64()?).ok()?;
    let o = u32::try_from(cd.get("output_tokens")?.as_u64()?).ok()?;
    Some(i.saturating_add(o))
}

/// Splice the raw-JSON hosted-tool entries for `web_search` and `x_search` into a serialized Responses request body's `tools` array.
/// `x_search` has no `rs::Tool` variant, and `web_search`'s typed filters cannot carry `excluded_domains`, so both travel as raw JSON.
/// Neither may also be emitted as a typed `rs::Tool`; the API rejects the duplicate.
fn splice_extra_tool_entries(
    request_body: &mut serde_json::Value,
    entries: Vec<serde_json::Value>,
) {
    if entries.is_empty() {
        return;
    }
    if let Some(tools) = request_body.get_mut("tools").and_then(|v| v.as_array_mut()) {
        tools.extend(entries);
    } else {
        if let Some(obj) = request_body.as_object_mut() {
            obj.insert("tools".to_owned(), serde_json::Value::Array(entries));
        }
    }
}

/// Parse `Retry-After` as integer seconds, capped at 120; HTTP-dates yield `None`.
fn extract_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let retry_after = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    // A token bucket answers a breach with the WHOLE window ("Retry-After:
    // 60" on a per-minute limit), while its own reset header says when this
    // caller's tokens actually come back. Take whichever is sooner. A
    // provider runs SEVERAL token buckets (total and uncached, per minute,
    // hour and day) and spells each reset differently, so the match is the
    // reset prefix plus the word that names the resource. A request bucket
    // is left out: it is not what a token breach waits on. A wait that turns
    // out to be short earns another 429, which the budget covers.
    let bucket_reset = headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            name.starts_with("x-ratelimit-reset-") && name.contains("tokens")
        })
        .filter_map(|(_, value)| parse_reset_seconds(value.to_str().ok()?))
        .min();

    match (retry_after, bucket_reset) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (one, None) | (None, one) => one,
    }
    .map(|s| s.min(120))
}

/// Seconds from a rate-limit reset header, which is written as a bare number
/// or with a unit (`1.5`, `1.5s`, `30s`). A fractional value rounds UP: a
/// wait shorter than the reset earns the same 429 back.
fn parse_reset_seconds(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let digits = raw.strip_suffix('s').unwrap_or(raw).trim();
    let secs = digits.parse::<f64>().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(secs.ceil() as u64)
}

fn extract_should_retry(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    headers
        .get("x-should-retry")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            if s.eq_ignore_ascii_case("true") {
                Some(true)
            } else if s.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None // unknown value, treat as absent
            }
        })
}

fn extract_model_metadata(headers: &reqwest::header::HeaderMap) -> Option<ResponseModelMetadata> {
    let context_window = headers
        .get("x-grok-context-window")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let max_completion_tokens = headers
        .get("x-grok-max-completion-tokens")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok());

    let models_etag = headers
        .get("x-models-etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if context_window.is_some() || max_completion_tokens.is_some() || models_etag.is_some() {
        Some(ResponseModelMetadata {
            context_window,
            max_completion_tokens,
            models_etag,
        })
    } else {
        None
    }
}

/// Wrapper for streaming chat completion requests that adds `stream` and `stream_options` without modifying the original `ChatCompletionRequest`.
#[derive(Serialize)]
struct StreamingChatRequest<'a> {
    #[serde(flatten)]
    inner: &'a ChatCompletionRequest,
    stream: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn append_response_includes(body: &mut serde_json::Value, extra_includes: &[String]) {
    if extra_includes.is_empty() {
        return;
    }
    let Some(body) = body.as_object_mut() else {
        return;
    };
    let include = body.entry("include").or_insert(serde_json::Value::Null);
    if include.is_null() {
        *include = serde_json::Value::Array(Vec::new());
    }
    let Some(include) = include.as_array_mut() else {
        return;
    };
    for value in extra_includes {
        if !include
            .iter()
            .any(|existing| existing.as_str() == Some(value.as_str()))
        {
            include.push(serde_json::Value::String(value.clone()));
        }
    }
}

/// Resolve `env_http_headers` (`header -> env var`) into `headers` via `getenv`, skipping unset/blank/invalid entries and trimming values.
fn apply_env_http_headers(
    env_http_headers: &IndexMap<String, String>,
    getenv: impl Fn(&str) -> Option<String>,
    headers: &mut HeaderMap,
) {
    for (key, env_var) in env_http_headers {
        let Some(value) = getenv(env_var) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (Ok(name), Ok(header_value)) = (
            HeaderName::try_from(key.as_str()),
            HeaderValue::from_str(value),
        ) else {
            tracing::warn!(
                header = %key,
                env_var = %env_var,
                "skipping env_http_header with an invalid header name or value"
            );
            continue;
        };
        headers.insert(name, header_value);
    }
}

/// HTTP client for sampling. Cheap to clone.
/// Carries an `Arc`-backed `reqwest::Client` and the default headers/request-defaults computed from a [`SamplerConfig`] at construction time.
#[derive(Clone)]
pub struct SamplingClient {
    http: reqwest::Client,
    default_headers: HeaderMap,
    base_url: String,
    /// Extra top-level body fields merged into every request this client
    /// sends; see [`SamplerConfig::extra_body`].
    extra_body: serde_json::Map<String, serde_json::Value>,
    defaults: ClientDefaults,
    /// Optional 401-attribution hook.
    /// The shell wires this to emit a structured event at every UNAUTHORIZED arm so 401s can be bucketed by stale-snapshot vs. live-token-rejected.
    /// `None` for sampler-only callers and tests.
    attribution_callback: Option<crate::attribution::SharedAttributionCallback>,
    /// Per-request bearer override. See `SamplerConfig::bearer_resolver`.
    bearer_resolver: Option<crate::config::SharedBearerResolver>,
    /// Per-request header injection (OTel traceparent).
    header_injector: Option<crate::config::SharedHeaderInjector>,
    /// Endpoint URL builder, resolved once from `base_url` and `query_params`.
    endpoint: EndpointTemplate,
    first_use_noted: Arc<AtomicBool>,
}

impl std::fmt::Debug for SamplingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamplingClient")
            .field("base_url", &self.base_url)
            .field("defaults", &self.defaults)
            .field(
                "has_attribution_callback",
                &self.attribution_callback.is_some(),
            )
            .field("has_bearer_resolver", &self.bearer_resolver.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
struct ClientDefaults {
    model: String,
    max_completion_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    /// The window `max_completion_tokens` shares with the prompt. `0` means
    /// unknown, which claims nothing about either.
    context_window: u64,
    api_backend: ApiBackend,
    auth_scheme: AuthScheme,
    request_compression: RequestCompression,
    stream_tool_calls: bool,
    reasoning_summary: Option<xai_grok_sampling_types::ReasoningSummary>,
    extra_response_includes: Vec<String>,
    doom_loop_recovery: Option<xai_grok_sampling_types::DoomLoopRecoveryPolicy>,
    /// Per-model message-schema profile, applied to every conversation request
    /// this client sends (see [`Self::apply_conversation_defaults`]).
    chat_message_profile: xai_grok_sampling_types::ChatMessageProfile,
}

/// The refusal for a model with no URL. No default URL exists to fall back to.
pub const MISSING_BASE_URL: &str = "This model has no URL, so no request was sent. \
     Set base_url in its [model.<id>] block, or on the [model_providers.<id>] block it names.";

/// Endpoint URL builder, resolved once at client construction so each request only appends its path.
#[derive(Clone, Debug)]
enum EndpointTemplate {
    /// No query params and no query on the base URL (or an unparseable base): append the path to the base verbatim.
    Plain(String),
    /// Query params configured: `{prefix}/{path}{suffix}`.
    /// `suffix` starts with `?` and folds any base-URL params; a configured key wins over the same key in `base_url`.
    /// Pairs are percent-encoded with no duplicates.
    WithQuery { prefix: String, suffix: String },
}

impl EndpointTemplate {
    fn new(base_url: &str, query_params: &IndexMap<String, String>) -> Self {
        let base = base_url.trim_end_matches('/').to_string();
        // The fast path is safe only when there is nothing to fold: no configured params and no query already on the base
        // A base query would otherwise land before the appended path
        if query_params.is_empty() && !base.contains('?') {
            return Self::Plain(base);
        }
        let mut url = match reqwest::Url::parse(&base) {
            Ok(url) => url,
            Err(error) => {
                tracing::warn!(
                    url = %base,
                    %error,
                    "failed to parse base URL for endpoint; sending without folded query"
                );
                return Self::Plain(base);
            }
        };
        let overridden: std::collections::HashSet<&str> =
            query_params.keys().map(String::as_str).collect();
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(k, _)| !overridden.contains(k.as_ref()))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let prefix = {
            let mut prefix_url = url.clone();
            prefix_url.set_query(None);
            prefix_url.as_str().trim_end_matches('/').to_string()
        };
        {
            let mut pairs = url.query_pairs_mut();
            pairs.clear();
            for (key, value) in &kept {
                pairs.append_pair(key, value);
            }
            for (key, value) in query_params {
                pairs.append_pair(key, value);
            }
        }
        let suffix = url.query().map(|q| format!("?{q}")).unwrap_or_default();
        Self::WithQuery { prefix, suffix }
    }

    fn url_for_path(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        match self {
            Self::Plain(base) => format!("{base}/{path}"),
            Self::WithQuery { prefix, suffix } => format!("{prefix}/{path}{suffix}"),
        }
    }
}

// =============================================================================
// User-Agent helpers
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlatformInfo {
    os: String,
    arch: String,
}

impl PlatformInfo {
    fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            other => other,
        }
        .to_string();

        let arch = match std::env::consts::ARCH {
            "arm64" => "aarch64",
            "x86_64" => "x86_64",
            other => other,
        }
        .to_string();

        Self { os, arch }
    }
}

fn agent_version() -> String {
    xai_grok_version::version().to_string()
}

/// Render a User-Agent string for the given origin client.
/// Mirrors the shell's `user_agent_string_for` but uses sampler-local constants.
/// The session typically owns the canonical User-Agent rendering for process-wide HTTP clients.
pub fn user_agent_string_for(origin: &OriginClientInfo) -> String {
    let agent_version = agent_version();
    let platform = PlatformInfo::current();

    if origin.product == AGENT_PRODUCT && origin.version.as_deref() == Some(agent_version.as_str())
    {
        return format!(
            "{}/{} ({}; {})",
            AGENT_PRODUCT, agent_version, platform.os, platform.arch
        );
    }

    match origin.version.as_deref() {
        Some(origin_version) => format!(
            "{}/{} {}/{} ({}; {})",
            origin.product,
            origin_version,
            AGENT_PRODUCT,
            agent_version,
            platform.os,
            platform.arch
        ),
        None => format!(
            "{} {}/{} ({}; {})",
            origin.product, AGENT_PRODUCT, agent_version, platform.os, platform.arch
        ),
    }
}

/// A request builder coupled to the credential state it was built with, so a 401 arm cannot classify from anything but the build-time capture.
/// The wire default (`SentCredential::Unknown`, which charges the retry budget) stays the fail-closed one.
/// Only an explicit `sent_bearer: None` (a send the builder provably stamped no credential onto) reaches the uncharged lane via [`auth_rejected`].
struct SentRequest {
    builder: reqwest::RequestBuilder,
    /// Tail fragment of the credential in the built headers (`None` means no credential header).
    sent_bearer: Option<String>,
}

/// The one way a 401 becomes a `SamplingError::Auth` with a wire-derived credential classification: from the fragment its [`SentRequest`] captured.
fn auth_rejected(message: String, sent_bearer: Option<&str>) -> SamplingError {
    SamplingError::Auth {
        message,
        credential: SentCredential::from_sent_fragment(sent_bearer),
    }
}

// =============================================================================
// SamplingClient
// =============================================================================

impl SamplingClient {
    /// The same client on the shared HTTP/1.1 transport, which never pools.
    /// A caller uses it after a transport failure, because a bad HTTP/2
    /// connection fails every request that the pool sends on it.
    pub fn with_http1(&self) -> Result<Self> {
        let mut client = self.clone();
        client.http = crate::shared_http::client_http1().map_err(SamplingError::Http)?;
        Ok(client)
    }

    /// Uses an identity-specific client for configured mTLS; otherwise grabs the process-wide shared client.
    /// This does not perform any network I/O.
    pub fn new(config: SamplerConfig) -> Result<Self> {
        if config.base_url.trim().is_empty() {
            return Err(SamplingError::InvalidConfiguration(MISSING_BASE_URL));
        }
        xai_grok_extra_ca::endpoint_allowlist::check(&config.base_url)
            .map_err(|refusal| SamplingError::EndpointNotAllowed(refusal.to_string()))?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(ref api_key) = config.api_key {
            match config.auth_scheme {
                AuthScheme::XApiKey => {
                    let header_value = HeaderValue::from_str(api_key).map_err(|_| {
                        tracing::debug!(
                            "Invalid api_key: cannot be converted to a valid HTTP header"
                        );
                        SamplingError::auth_unknown(
                            "Invalid api_key: cannot be converted to a valid HTTP header",
                        )
                    })?;
                    headers.insert(HeaderName::from_static("x-api-key"), header_value);
                }
                AuthScheme::Bearer => {
                    let bearer = format!("Bearer {}", api_key);
                    let header_value = HeaderValue::from_str(&bearer).map_err(|_| {
                        tracing::debug!(
                            "Invalid api_key: cannot be converted to a valid HTTP Authorization header"
                        );
                        SamplingError::auth_unknown(
                            "Invalid api_key: cannot be converted to a valid HTTP Authorization header",
                        )
                    })?;
                    headers.insert(AUTHORIZATION, header_value);
                }
                AuthScheme::None => {}
            }
        }

        // Apply all extra headers verbatim
        // This is the single injection point for proxy-auth headers and any other URL- or environment-specific headers the session decides to set
        for (key, value) in &config.extra_headers {
            let header_name = HeaderName::try_from(key.as_str())
                .map_err(|_| SamplingError::InvalidConfiguration("Invalid extra header name"))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|_| SamplingError::InvalidConfiguration("Invalid extra header value"))?;
            headers.insert(header_name, header_value);
        }

        // Resolve here, not into `extra_headers`, so an env-sourced secret stays out of persisted state
        apply_env_http_headers(
            &config.env_http_headers,
            |var| std::env::var(var).ok(),
            &mut headers,
        );

        // Add x-grok-client-version header for version gating at the proxy.
        if let Some(client_version) = config.client_version.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(client_version)
        {
            headers.insert(
                HeaderName::from_static("x-grok-client-version"),
                header_value,
            );
        }

        if let Some(deployment_id) = config.deployment_id.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(deployment_id)
        {
            headers.insert(
                HeaderName::from_static("x-grok-deployment-id"),
                header_value,
            );
        }

        if let Some(user_id) = config.user_id.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(user_id)
        {
            headers.insert(HeaderName::from_static("x-grok-user-id"), header_value);
        }

        if let Some(conversation_group_id) = config.conversation_group_id.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(conversation_group_id.as_ref())
        {
            headers.insert(
                HeaderName::from_static("x-grok-conv-group-id"),
                header_value,
            );
        }

        {
            let client_id = config
                .client_identifier
                .clone()
                .unwrap_or_else(|| DEFAULT_CLIENT_IDENTIFIER.to_string());
            if let Ok(header_value) = HeaderValue::from_str(&client_id) {
                headers.insert(
                    HeaderName::from_static("x-grok-client-identifier"),
                    header_value,
                );
            }
        }

        // Always set User-Agent: per-session origin if available, else fallback.
        {
            let ua_string = match config.origin_client.as_ref() {
                Some(origin) => user_agent_string_for(origin),
                None => user_agent_string_for(&OriginClientInfo {
                    product: AGENT_PRODUCT.to_string(),
                    version: Some(agent_version()),
                }),
            };
            if let Ok(v) = HeaderValue::from_str(&ua_string) {
                headers.insert(USER_AGENT, v);
            }
        }

        if config.force_http1 {
            tracing::info!("Using HTTP/1.1 for sampling client (force_http1=true)");
        }
        let http = if let Some(cert_dir) = config.mtls_cert_dir.as_deref() {
            crate::shared_http::mtls_client(cert_dir, config.force_http1)?
        } else if config.force_http1 {
            crate::shared_http::client_http1().map_err(SamplingError::Http)?
        } else {
            crate::shared_http::client().map_err(SamplingError::Http)?
        };

        tracing::info!(
            target: crate::sampling_log::TARGET,
            event = "client_new",
            base_url = %config.base_url,
            model = %config.model,
            api_backend = ?config.api_backend,
            auth_scheme = ?config.auth_scheme,
            request_compression = ?config.request_compression,
            // "unset" (not "none"): `ReasoningEffort::None` is a real wire value; logging the absent Option as "none" looked like we were sending it
            reasoning_effort = config.reasoning_effort.map_or("unset", |e| e.into()),
            has_api_key = config.api_key.is_some(),
            has_bearer_resolver = config.bearer_resolver.is_some(),
            has_authorization_header = headers.get(AUTHORIZATION).is_some(),
            has_x_api_key_header = headers.get(HeaderName::from_static("x-api-key")).is_some(),
        );

        let defaults = ClientDefaults {
            model: config.model,
            max_completion_tokens: config.max_completion_tokens,
            temperature: config.temperature,
            top_p: config.top_p,
            context_window: config.context_window,
            api_backend: config.api_backend,
            auth_scheme: config.auth_scheme,
            request_compression: config.request_compression,
            stream_tool_calls: config.stream_tool_calls,
            reasoning_summary: config.reasoning_summary,
            extra_response_includes: config.extra_response_includes,
            doom_loop_recovery: config.doom_loop_recovery,
            chat_message_profile: config.chat_message_profile,
        };

        // Ollama's native paths are siblings of the OpenAI-compatible endpoint
        // at the host root, not children of it. A provider block names one
        // base URL for both, so `http://host:11434/v1` has to resolve
        // `api/chat` at `http://host:11434/api/chat`.
        let endpoint_base = if defaults.api_backend == ApiBackend::Ollama {
            native_host_root(&config.base_url)
        } else {
            config.base_url.clone()
        };
        let endpoint = EndpointTemplate::new(&endpoint_base, &config.query_params);

        Ok(Self {
            http,
            default_headers: headers,
            base_url: config.base_url,
            extra_body: config.extra_body,
            defaults,
            attribution_callback: config.attribution_callback,
            bearer_resolver: config.bearer_resolver,
            header_injector: config.header_injector,
            endpoint,
            first_use_noted: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn api_backend(&self) -> ApiBackend {
        self.defaults.api_backend.clone()
    }

    /// Give the bearer resolver its pre-send hook before [`Self::post`] reads it.
    /// Awaited separately because `post` is sync (its callers hand the builder straight to `send()`).
    async fn prepare_bearer(&self) {
        if let Some(resolver) = &self.bearer_resolver {
            resolver.prepare_for_send().await;
        }
    }

    /// The credential tail is captured at build time — see [`SentRequest`] for
    /// why a record-time re-read would race the recovery a 401 triggers.
    fn post(&self, url: impl reqwest::IntoUrl) -> SentRequest {
        if !self.first_use_noted.load(Ordering::Relaxed)
            && !self.first_use_noted.swap(true, Ordering::Relaxed)
        {
            crate::prewarm::note_first_sampling_use(&self.base_url);
        }
        let mut headers = self.default_headers.clone();
        if let Some(resolver) = &self.bearer_resolver {
            // Sole auth source: without a live bearer, send no credential rather than a stale seed key.
            headers.remove(AUTHORIZATION);
            headers.remove(HeaderName::from_static("x-api-key"));
            if let Some(fresh) = resolver.current_bearer() {
                match self.defaults.auth_scheme {
                    AuthScheme::XApiKey => {
                        if let Ok(v) = HeaderValue::from_str(&fresh) {
                            headers.insert(HeaderName::from_static("x-api-key"), v);
                        }
                    }
                    AuthScheme::Bearer => {
                        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {fresh}")) {
                            headers.insert(AUTHORIZATION, v);
                        }
                    }
                    // Already stripped above; a fresh bearer never gets attached.
                    AuthScheme::None => {}
                }
            }
        }
        // Presence only: this fork never logs a credential, not even a
        // prefix of one.
        tracing::info!(
            target: crate::sampling_log::TARGET,
            event = "client_post",
            base_url = %self.base_url,
            model = %self.defaults.model,
            api_backend = ?self.defaults.api_backend,
            auth_scheme = ?self.defaults.auth_scheme,
            has_bearer_resolver = self.bearer_resolver.is_some(),
            has_authorization_header = headers.get(AUTHORIZATION).is_some(),
            has_x_api_key_header = headers.get(HeaderName::from_static("x-api-key")).is_some(),
        );
        let sent_bearer = Self::sent_fragment_from_headers(&headers, &self.defaults.auth_scheme);
        if let Some(injector) = &self.header_injector {
            injector.inject(&mut headers);
        }
        SentRequest {
            builder: self.http.post(url).headers(headers),
            sent_bearer,
        }
    }

    /// Must run before the span gets its first child, which starts it and freezes its parent.
    fn adopt_traceparent(&self, span: &tracing::Span, traceparent: Option<&str>) {
        if let Some(injector) = &self.header_injector
            && let Some(traceparent) = traceparent
            && !span.is_disabled()
        {
            injector.set_span_parent(span, traceparent);
        }
    }

    /// Tail fragment of the credential in `headers`: `x-api-key` (Messages-API scheme) or `Authorization`.
    /// The fragment length is [`crate::attribution::BEARER_SUFFIX_LEN`].
    fn sent_fragment_from_headers(headers: &HeaderMap, scheme: &AuthScheme) -> Option<String> {
        let raw = match scheme {
            AuthScheme::XApiKey => headers
                .get(HeaderName::from_static("x-api-key"))
                .and_then(|v| v.to_str().ok()),
            AuthScheme::Bearer => headers
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer ")),
            // No credential is attached under this scheme, so there is
            // nothing to attribute a 401 to.
            AuthScheme::None => None,
        };
        raw.map(|s| bearer_suffix(s).to_string())
    }

    /// Best-effort *build-time* view of what the next request would carry (resolver-authoritative).
    /// For request-start diagnostics ([`Self::auth_info`]) only.
    /// 401 attribution must use the fragment captured by [`Self::post`], which cannot race a recovery.
    fn current_sent_bearer_suffix(&self) -> Option<String> {
        // A resolver stays wired when the endpoint opts out of auth; its
        // bearer is not what goes on the wire.
        if self.defaults.auth_scheme == AuthScheme::None {
            return None;
        }
        if self.bearer_resolver.is_some() {
            return self
                .bearer_resolver
                .as_ref()
                .and_then(|r| r.current_bearer())
                .map(|s| bearer_suffix(&s).to_string());
        }
        Self::sent_fragment_from_headers(&self.default_headers, &self.defaults.auth_scheme)
    }

    /// Invoke the optional 401 attribution callback for one logical 401 response.
    /// The emit happens at the lowest layer that saw the status, so higher layers that react to a 401 must not emit a duplicate event.
    /// `sent_suffix` is the fragment [`Self::post`] captured for the rejected request.
    fn record_401_attribution(
        &self,
        consumer: crate::attribution::SamplingConsumer,
        sent_suffix: Option<&str>,
    ) {
        if let Some(cb) = self.attribution_callback.as_ref() {
            cb.record_401(consumer, sent_suffix);
        }
    }

    pub fn auth_info(&self) -> crate::sampling_log::AuthInfo {
        let auth_prefix = self.current_sent_bearer_suffix();
        let auth_type = match (&self.defaults.auth_scheme, &auth_prefix) {
            (AuthScheme::XApiKey, Some(_)) => "x-api-key",
            (AuthScheme::Bearer, Some(_)) => "bearer",
            (AuthScheme::None, _) => "none",
            (_, None) => "none",
        };
        crate::sampling_log::AuthInfo {
            auth_type,
            auth_prefix,
        }
    }

    fn is_sensitive_header(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.contains("authorization")
            || lower.contains("api-key")
            || lower.contains("apikey")
            || lower.contains("token")
            || lower.contains("secret")
    }

    /// Short lossy body snippet for error logs (never user-facing).
    fn body_preview(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).chars().take(500).collect()
    }

    /// Log all headers from a request at debug level (redacting sensitive values).
    fn log_request_headers(request: &reqwest::Request, endpoint_name: &str) {
        for (name, value) in request.headers().iter() {
            let value_str = if Self::is_sensitive_header(name.as_str()) {
                "[REDACTED]"
            } else {
                value.to_str().unwrap_or("[non-utf8]")
            };
            tracing::debug!(
                header_name = %name,
                header_value = %value_str,
                "Request header ({})",
                endpoint_name
            );
        }
    }

    fn endpoint(&self, path: &str) -> String {
        self.endpoint.url_for_path(path)
    }

    fn apply_defaults(&self, mut request: ChatCompletionRequest) -> Result<ChatCompletionRequest> {
        if request.model.is_none() {
            request.model = Some(self.defaults.model.clone());
        }

        if request.max_tokens.is_none() {
            request.max_tokens = self.defaults.max_completion_tokens;
        }

        if request.temperature.is_none() {
            request.temperature = self.defaults.temperature;
        }

        if request.top_p.is_none() {
            request.top_p = self.defaults.top_p;
        }

        Ok(request)
    }

    /// `sent_bearer` is the fragment [`Self::post`] captured for the request that produced `response` (401 attribution).
    async fn handle_response(
        &self,
        response: reqwest::Response,
        sent_bearer: Option<&str>,
    ) -> Result<ChatCompletionResponse> {
        let status = response.status();
        let request_url = response.url().to_string();
        let model_metadata = extract_model_metadata(response.headers());
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        let bytes = read_body(response, status).await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::ChatCompletions,
                    sent_bearer,
                );
                let server_message = user_facing_api_error_message(status, bytes.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401): {server_message}"),
                    sent_bearer,
                ));
            }
            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let completion = serde_json::from_slice::<ChatCompletionResponse>(&bytes).map_err(|e| {
            let raw_body = String::from_utf8_lossy(&bytes);
            tracing::error!(
                error = %e,
                raw_body = %raw_body,
                "Failed to deserialize ChatCompletionResponse"
            );
            SamplingError::Serialization(e)
        })?;
        Ok(completion)
    }

    // =========================================================================
    // Chat Completions API
    // =========================================================================

    pub async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let payload = self.apply_defaults(request)?;
        let x_grok_conv_id = &payload.x_grok_conv_id.clone().unwrap_or_default();
        let x_grok_req_id = &payload.x_grok_req_id.clone().unwrap_or_default();
        let model_id = payload.model.clone().unwrap_or_default();

        let request_region = crate::span_timing::Region::from_span(tracing::info_span!(
            "sampling.nonstream_request",
            model = %model_id,
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
        ));

        tracing::debug!(
            base_url = %self.base_url,
            model_id = %model_id,
            "Sending chat completion request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: payload.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: payload.x_grok_turn_idx.as_deref(),
            transient_retry: payload.x_grok_transient_retry.as_deref(),
            agent_id: payload.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: payload.x_grok_deployment_id.as_deref(),
            user_id: payload.x_grok_user_id.as_deref(),
        };
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("chat/completions"));
        let built_request = self
            .build_json_request(grok_headers.apply(builder), &payload)
            .await?;
        let response = self.send(built_request).await?;

        let status = response.status();
        request_region
            .span()
            .record("status_code", status.as_u16() as i64);
        request_region.span().record("success", status.is_success());

        self.handle_response(response, sent_bearer.as_deref()).await
    }

    /// Serialize `payload` onto `builder` the way `RequestBuilder::json` does
    /// (a caller-set `Content-Type` wins), zstd-compressing large bodies when
    /// the shell marked this endpoint as accepting it.
    async fn build_json_request<T: Serialize + ?Sized>(
        &self,
        builder: reqwest::RequestBuilder,
        payload: &T,
    ) -> Result<reqwest::Request> {
        let json = serde_json::to_vec(payload).map_err(|e| {
            tracing::error!("Failed to serialize request body: {}", e);
            SamplingError::Serialization(e)
        })?;
        let mut request = builder.build().map_err(|e| {
            tracing::error!("Failed to build HTTP request: {}", e);
            SamplingError::Http(e)
        })?;
        if !request.headers().contains_key(CONTENT_TYPE) {
            request
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        let body = if should_compress(self.defaults.request_compression, json.len()) {
            match compress_body(&json).await {
                Some(compressed) => {
                    request
                        .headers_mut()
                        .insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
                    compressed
                }
                None => json,
            }
        } else {
            json
        };
        *request.body_mut() = Some(reqwest::Body::from(body));
        Ok(request)
    }

    async fn send(&self, request: reqwest::Request) -> Result<reqwest::Response> {
        self.http
            .execute(request)
            .await
            .inspect_err(|e| tracing::debug!("HTTP request failed: {}", e))
            .map_err(Into::into)
    }

    async fn execute_stream_request(
        &self,
        built_request: reqwest::Request,
        span_timing: &mut StreamSpanTiming,
    ) -> Result<reqwest::Response> {
        span_timing.record_request_build();
        let response = self.http.execute(built_request).await.map_err(|e| {
            tracing::debug!("HTTP request failed: {}", e);
            span_timing.record_transport_failure(&e.to_string());
            e
        })?;
        span_timing.record_response_headers();
        Ok(response)
    }

    /// Start a streaming chat completion request. Returns a stream of typed chunks.
    pub async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(
        BoxStream<'static, Result<ChatCompletionChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        let region = crate::span_timing::stream_span!(
            "http.chat_completion_stream",
            endpoint = %self.endpoint("chat/completions"),
            model_id = request.model.as_deref().unwrap_or(""),
        );
        self.adopt_traceparent(region.span(), request.traceparent.as_deref());
        if region.span().is_disabled() {
            self.chat_completion_stream_inner(request, region).await
        } else {
            let span = region.span().clone();
            self.chat_completion_stream_inner(request, region)
                .instrument(span)
                .await
        }
    }

    async fn chat_completion_stream_inner(
        &self,
        request: ChatCompletionRequest,
        region: crate::span_timing::Region,
    ) -> Result<(
        BoxStream<'static, Result<ChatCompletionChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        let mut span_timing = StreamSpanTiming::start(region);
        let payload = self.apply_defaults(request)?;
        let x_grok_conv_id = &payload.x_grok_conv_id.clone().unwrap_or_default();
        let x_grok_req_id = &payload.x_grok_req_id.clone().unwrap_or_default();
        let model_id = payload.model.clone().unwrap_or_default();

        // Wrap the request with streaming fields and serialize once.
        let streaming_request = StreamingChatRequest {
            inner: &payload,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: payload.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: payload.x_grok_turn_idx.as_deref(),
            transient_retry: payload.x_grok_transient_retry.as_deref(),
            agent_id: payload.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: payload.x_grok_deployment_id.as_deref(),
            user_id: payload.x_grok_user_id.as_deref(),
        };
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("chat/completions"));
        let http_request = grok_headers
            .apply(builder)
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"));
        // The typed request is closed, so a per-deployment field only this
        // target understands (LM Studio's `ttl`) rides here. The extra
        // serialization is paid only by a caller that configured one.
        let built_request = if self.extra_body.is_empty() {
            self.build_json_request(http_request, &streaming_request)
                .await?
        } else {
            let mut body =
                serde_json::to_value(&streaming_request).map_err(SamplingError::Serialization)?;
            xai_grok_sampling_types::merge_extra_body(&mut body, &self.extra_body);
            self.build_json_request(http_request, &body).await?
        };

        tracing::debug!(
            url = %built_request.url(),
            method = %built_request.method(),
            "Sending chat/completions request"
        );
        Self::log_request_headers(&built_request, "chat/completions");
        let response = self
            .execute_stream_request(built_request, &mut span_timing)
            .await?;

        let status = response.status();
        let request_url = response.url().to_string();
        span_timing
            .span()
            .record(STATUS_CODE, status.as_u16() as i64);
        span_timing.span().record(SUCCESS, status.is_success());
        let model_metadata = extract_model_metadata(response.headers());
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                span_timing.span().record(ERROR, "unauthorized (401)");
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::ChatCompletionsStream,
                    sent_bearer.as_deref(),
                );
                let endpoint = self.endpoint("chat/completions");
                let body = response.bytes().await.unwrap_or_default();
                let server_message = user_facing_api_error_message(status, body.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }

            let bytes = read_body(response, status).await?;
            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            span_timing.span().record(ERROR, message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                body_preview = %Self::body_preview(bytes.as_ref()),
                model_id = %model_id,
                "chat/completions API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        // Strip UTF-8 BOM if present: eventsource-stream 0.2.3 incorrectly slices BOM at byte 1 instead of 3.
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        let event_stream = byte_stream.eventsource();

        // Map SSE events into ChatCompletionChunk.
        // Uses `scan` so that `[DONE]` and transport errors both terminate the stream (`None`)
        // The first transport error is emitted to the consumer, then subsequent polls return `None`
        let chunks = event_stream
            .scan(false, |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "chat_completions",
                            data = %data,
                        );

                        if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Err(stream_error))
                        } else {
                            Some(parse_sse_event::<ChatCompletionChunk>(data).map_err(|e| {
                                tracing::error!(
                                    error = %e,
                                    raw_data = %data,
                                    "Failed to deserialize ChatCompletionChunk from stream"
                                );
                                e
                            }))
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Err(SamplingError::EventStreamError(sse_error_text(&e))))
                    }
                };
                std::future::ready(item)
            })
            .boxed();

        Ok((
            span_timing.hold_until_first_content(chunks, chat_chunk_class),
            model_metadata,
        ))
    }

    // =========================================================================
    // Responses API
    // =========================================================================

    fn apply_response_defaults(&self, request: &mut CreateResponseWrapper) -> Result<()> {
        if request.inner.model.is_none() {
            request.inner.model = Some(self.defaults.model.clone());
        }

        if request.inner.temperature.is_none() {
            request.inner.temperature = self.defaults.temperature;
        }

        if request.inner.top_p.is_none() {
            request.inner.top_p = self.defaults.top_p;
        }

        if request.inner.max_output_tokens.is_none() {
            request.inner.max_output_tokens = self.defaults.max_completion_tokens;
        }

        // The API defaults `store` to true, which breaks ZDR compliance
        if request.inner.store.is_none() {
            request.inner.store = Some(false);
        }

        if let Some(summary) = self.defaults.reasoning_summary {
            let summary = summary.to_responses_api();
            match request.inner.reasoning.as_mut() {
                Some(reasoning) => reasoning.summary = summary,
                None if summary.is_some() => {
                    request.inner.reasoning = Some(rs::Reasoning {
                        effort: None,
                        summary,
                    });
                }
                None => {}
            }
        }

        // Include encrypted reasoning content if not specified
        let includes = request.inner.include.get_or_insert_with(Vec::new);
        if !includes.contains(&rs::IncludeEnum::ReasoningEncryptedContent) {
            includes.push(rs::IncludeEnum::ReasoningEncryptedContent);
        }

        Ok(())
    }

    /// Create a response using the Responses API (non-streaming).
    pub async fn create_response(
        &self,
        mut request: CreateResponseWrapper,
    ) -> Result<rs::Response> {
        self.apply_response_defaults(&mut request)?;

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone().unwrap_or_default();

        let request_region = crate::span_timing::Region::from_span(tracing::info_span!(
            "sampling.nonstream_request",
            model = %model_id,
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
        ));

        // The trace field is process-local: upstream session code consumes it (and may upload a payload artifact); the sampler never forwards it
        // Drop it before we send
        request.trace.take();

        tracing::debug!("create_response: {:?}", &request);
        tracing::debug!("endpoint: {:?}", self.endpoint("responses"));

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            transient_retry: request.x_grok_transient_retry.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let extra_tool_entries = std::mem::take(&mut request.extra_tool_entries);
        let mut request_body = serde_json::to_value(&request.inner).map_err(|e| {
            tracing::error!("Failed to serialize responses request: {}", e);
            SamplingError::Serialization(e)
        })?;
        splice_extra_tool_entries(&mut request_body, extra_tool_entries);
        append_response_includes(&mut request_body, &self.defaults.extra_response_includes);
        // async-openai's ReasoningTextContent struct omits the `type` discriminator that the Responses API requires on input
        // Patch it in after serializing
        xai_grok_sampling_types::patch_reasoning_text_types(&mut request_body);
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("responses"));
        let built_request = self
            .build_json_request(grok_headers.apply(builder), &request_body)
            .await?;
        let response = self.send(built_request).await?;

        let status = response.status();
        let request_url = response.url().to_string();
        request_region
            .span()
            .record("status_code", status.as_u16() as i64);
        request_region.span().record("success", status.is_success());
        let model_metadata = extract_model_metadata(response.headers());
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        let bytes = read_body(response, status).await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::Responses,
                    sent_bearer.as_deref(),
                );
                let endpoint = self.endpoint("responses");
                let server_message = user_facing_api_error_message(status, bytes.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }

            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            tracing::warn!(
                status = %status,
                error_message = %message,
                body_preview = %Self::body_preview(bytes.as_ref()),
                model_id = %model_id,
                "responses API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let response_obj = serde_json::from_slice::<rs::Response>(&bytes).map_err(|e| {
            let raw_body = String::from_utf8_lossy(&bytes);
            tracing::error!(
                error = %e,
                raw_body = %raw_body,
                "Failed to deserialize rs::Response"
            );
            SamplingError::Serialization(e)
        })?;
        Ok(response_obj)
    }

    /// Create a streaming response using the Responses API.
    ///
    /// Third element is the doom-loop collector, `Some` only when `doom_loop_recovery` is set.
    #[allow(clippy::type_complexity)]
    pub async fn create_response_stream(
        &self,
        request: CreateResponseWrapper,
    ) -> Result<(
        BoxStream<'static, Result<rs::ResponseStreamEvent>>,
        Option<ResponseModelMetadata>,
        Option<crate::doom_loop::DoomLoopSignalCollector>,
    )> {
        let region = crate::span_timing::stream_span!(
            "http.create_response_stream",
            endpoint = %self.endpoint("responses"),
            model_id = request.inner.model.as_deref().unwrap_or(""),
        );
        self.adopt_traceparent(region.span(), request.traceparent.as_deref());
        if region.span().is_disabled() {
            self.create_response_stream_inner(request, region).await
        } else {
            let span = region.span().clone();
            self.create_response_stream_inner(request, region)
                .instrument(span)
                .await
        }
    }

    #[allow(clippy::type_complexity)]
    async fn create_response_stream_inner(
        &self,
        mut request: CreateResponseWrapper,
        region: crate::span_timing::Region,
    ) -> Result<(
        BoxStream<'static, Result<rs::ResponseStreamEvent>>,
        Option<ResponseModelMetadata>,
        Option<crate::doom_loop::DoomLoopSignalCollector>,
    )> {
        let mut span_timing = StreamSpanTiming::start(region);
        self.apply_response_defaults(&mut request)?;

        request.inner.stream = Some(true);

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone().unwrap_or_default();

        // Drop process-local trace data (see note in `create_response`).
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = model_id.as_str(),
            "Sending responses API stream request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            transient_retry: request.x_grok_transient_retry.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        let extra_tool_entries = std::mem::take(&mut request.extra_tool_entries);
        let mut request_body = serde_json::to_value(&request.inner).map_err(|e| {
            tracing::error!("Failed to serialize responses request: {}", e);
            SamplingError::Serialization(e)
        })?;
        // Inject xAI-specific fields not in async-openai's CreateResponse type.
        if self.defaults.stream_tool_calls
            && let Some(obj) = request_body.as_object_mut()
        {
            obj.insert("stream_tool_calls".to_owned(), serde_json::json!(true));
        }
        splice_extra_tool_entries(&mut request_body, extra_tool_entries);
        append_response_includes(&mut request_body, &self.defaults.extra_response_includes);
        xai_grok_sampling_types::patch_reasoning_text_types(&mut request_body);
        // Fresh per attempt so signals never leak across retries; `None` (check disabled) sends no header and does no peek work per event
        let doom_loop = self
            .defaults
            .doom_loop_recovery
            .map(crate::doom_loop::DoomLoopSignalCollector::new);
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("responses"));
        let mut http_request = grok_headers
            .apply(builder)
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if let Some(policy) = self.defaults.doom_loop_recovery {
            http_request = http_request
                .header(DOOM_LOOP_CHECK_HEADER, policy.window_tokens.to_string())
                .header(
                    EXACT_REPETITION_CHECK_HEADER,
                    DEFAULT_EXACT_REPETITION_MIN_TOKENS.to_string(),
                );
        }
        let built_request = self.build_json_request(http_request, &request_body).await?;

        tracing::debug!(
            url = %built_request.url(),
            method = %built_request.method(),
            "Sending responses API stream request"
        );
        Self::log_request_headers(&built_request, "responses");
        let response = self
            .execute_stream_request(built_request, &mut span_timing)
            .await?;

        let status = response.status();
        let request_url = response.url().to_string();
        span_timing
            .span()
            .record(STATUS_CODE, status.as_u16() as i64);
        span_timing.span().record(SUCCESS, status.is_success());
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                span_timing.span().record(ERROR, "unauthorized (401)");
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::ResponsesStream,
                    sent_bearer.as_deref(),
                );
                let endpoint = self.endpoint("responses");
                let body = response.bytes().await.unwrap_or_default();
                let server_message = user_facing_api_error_message(status, body.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }
            let model_metadata = extract_model_metadata(response.headers());
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let bytes = read_body(response, status).await?;
            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            span_timing.span().record(ERROR, message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                body_preview = %Self::body_preview(bytes.as_ref()),
                model_id = %model_id,
                "responses API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let model_metadata = extract_model_metadata(response.headers());

        // Strip UTF-8 BOM if present
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        let event_stream = byte_stream.eventsource();

        let doom_loop_for_stream = doom_loop.clone();

        // The scan item is an `Option`: `Some(None)` skips an absorbed doom-loop event without terminating the stream (`filter_map` below)
        // An outer `None` still ends the stream
        let events = event_stream
            .scan(false, move |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "responses",
                            data = %data,
                        );

                        // Intercept the non-standard doom-loop event before typed deserialization
                        // async-openai's event enum does not know it and would fail to parse it
                        // With the check disabled, `is_check_event` still guards against a server emitting it without opt-in (rollout skew)
                        let swallow = match &doom_loop_for_stream {
                            Some(collector) => collector.absorb(&event.event, data),
                            None => is_check_event(&event.event, data),
                        };
                        if swallow {
                            Some(None)
                        } else if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Some(Err(stream_error)))
                        } else {
                            Some(Some(deserialize_response_event(data)))
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Some(Err(SamplingError::EventStreamError(sse_error_text(
                            &e,
                        )))))
                    }
                };
                std::future::ready(item)
            })
            .filter_map(std::future::ready)
            .boxed();

        Ok((
            span_timing.hold_until_first_content(events, responses_event_class),
            model_metadata,
            doom_loop,
        ))
    }

    // =========================================================================
    // Anthropic Messages API
    // =========================================================================

    fn apply_message_defaults(&self, request: &mut MessagesRequestWrapper) -> Result<()> {
        if request.inner.model.is_empty() {
            request.inner.model = self.defaults.model.clone();
        }

        if request.inner.max_tokens == 0 {
            request.inner.max_tokens = self
                .defaults
                .max_completion_tokens
                .unwrap_or(ANTHROPIC_DEFAULT_MAX_TOKENS);
        }

        if request.inner.temperature.is_none() {
            request.inner.temperature = self.defaults.temperature;
        }

        if request.inner.top_p.is_none() {
            request.inner.top_p = self.defaults.top_p;
        }

        Ok(())
    }

    /// Create a message using the Anthropic Messages API (non-streaming).
    pub async fn create_message(
        &self,
        mut request: MessagesRequestWrapper,
    ) -> Result<messages::MessagesResponse> {
        self.apply_message_defaults(&mut request)?;

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone();

        let request_region = crate::span_timing::Region::from_span(tracing::info_span!(
            "sampling.nonstream_request",
            model = %model_id,
            status_code = tracing::field::Empty,
            success = tracing::field::Empty,
        ));

        // Drop process-local trace data.
        request.trace.take();

        tracing::debug!("create_message: {:?}", &request.inner);
        tracing::debug!("endpoint: {:?}", self.endpoint("messages"));

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            transient_retry: request.x_grok_transient_retry.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("messages"));
        let built_request = self
            .build_json_request(grok_headers.apply(builder), &request.inner)
            .await?;
        let response = self.send(built_request).await?;

        let status = response.status();
        let request_url = response.url().to_string();
        request_region
            .span()
            .record("status_code", status.as_u16() as i64);
        request_region.span().record("success", status.is_success());
        let model_metadata = extract_model_metadata(response.headers());
        let retry_after_secs = extract_retry_after(response.headers());
        let should_retry = extract_should_retry(response.headers());
        let bytes = read_body(response, status).await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::Messages,
                    sent_bearer.as_deref(),
                );
                let endpoint = self.endpoint("messages");
                let server_message = user_facing_api_error_message(status, bytes.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }

            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            tracing::warn!(
                status = %status,
                error_message = %message,
                body_preview = %Self::body_preview(bytes.as_ref()),
                model_id = %model_id,
                "messages API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let response_obj =
            serde_json::from_slice::<messages::MessagesResponse>(&bytes).map_err(|e| {
                let raw_body = String::from_utf8_lossy(&bytes);
                tracing::error!(
                    error = %e,
                    raw_body = %raw_body,
                    "Failed to deserialize MessagesResponse"
                );
                SamplingError::Serialization(e)
            })?;
        Ok(response_obj)
    }

    /// Create a streaming message using the Anthropic Messages API.
    pub async fn create_message_stream(
        &self,
        request: MessagesRequestWrapper,
    ) -> Result<(
        BoxStream<'static, Result<messages::MessageStreamEvent>>,
        Option<ResponseModelMetadata>,
    )> {
        let region = crate::span_timing::stream_span!(
            "http.create_message_stream",
            endpoint = %self.endpoint("messages"),
            model_id = request.inner.model.as_str(),
        );
        self.adopt_traceparent(region.span(), request.traceparent.as_deref());
        if region.span().is_disabled() {
            self.create_message_stream_inner(request, region).await
        } else {
            let span = region.span().clone();
            self.create_message_stream_inner(request, region)
                .instrument(span)
                .await
        }
    }

    async fn create_message_stream_inner(
        &self,
        mut request: MessagesRequestWrapper,
        region: crate::span_timing::Region,
    ) -> Result<(
        BoxStream<'static, Result<messages::MessageStreamEvent>>,
        Option<ResponseModelMetadata>,
    )> {
        let mut span_timing = StreamSpanTiming::start(region);
        self.apply_message_defaults(&mut request)?;

        request.inner.stream = Some(true);

        let x_grok_conv_id = request.x_grok_conv_id.as_deref().unwrap_or_default();
        let x_grok_req_id = request.x_grok_req_id.as_deref().unwrap_or_default();
        let model_id = request.inner.model.clone();

        // Drop process-local trace data.
        request.trace.take();

        tracing::debug!(
            base_url = %self.base_url,
            model_id = model_id.as_str(),
            "Sending Messages API stream request"
        );

        let grok_headers = GrokRequestHeaders {
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
            model_id: &model_id,
            session_id: request.x_grok_session_id.as_deref().unwrap_or_default(),
            turn_idx: request.x_grok_turn_idx.as_deref(),
            transient_retry: request.x_grok_transient_retry.as_deref(),
            agent_id: request.x_grok_agent_id.as_deref().unwrap_or_default(),
            deployment_id: request.x_grok_deployment_id.as_deref(),
            user_id: request.x_grok_user_id.as_deref(),
        };
        self.prepare_bearer().await;
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(self.endpoint("messages"));
        let http_request = grok_headers
            .apply(builder)
            .header(ACCEPT, HeaderValue::from_static("text/event-stream"));
        let built_request = if self.extra_body.is_empty() {
            self.build_json_request(http_request, &request.inner)
                .await?
        } else {
            let mut body =
                serde_json::to_value(&request.inner).map_err(SamplingError::Serialization)?;
            xai_grok_sampling_types::merge_extra_body(&mut body, &self.extra_body);
            self.build_json_request(http_request, &body).await?
        };

        tracing::debug!(
            url = %built_request.url(),
            method = %built_request.method(),
            "Sending messages API stream request"
        );
        Self::log_request_headers(&built_request, "messages");
        let response = self
            .execute_stream_request(built_request, &mut span_timing)
            .await?;

        let status = response.status();
        let request_url = response.url().to_string();
        span_timing
            .span()
            .record(STATUS_CODE, status.as_u16() as i64);
        span_timing.span().record(SUCCESS, status.is_success());
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                span_timing.span().record(ERROR, "unauthorized (401)");
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::MessagesStream,
                    sent_bearer.as_deref(),
                );
                let endpoint = self.endpoint("messages");
                let body = response.bytes().await.unwrap_or_default();
                let server_message = user_facing_api_error_message(status, body.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }
            let model_metadata = extract_model_metadata(response.headers());
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let bytes = read_body(response, status).await?;
            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            span_timing.span().record(ERROR, message.as_str());
            tracing::error!(
                status = %status,
                error_message = %message,
                body_preview = %Self::body_preview(bytes.as_ref()),
                model_id = %model_id,
                "messages API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let model_metadata = extract_model_metadata(response.headers());

        // Strip UTF-8 BOM if present
        const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut is_first = true;
        let byte_stream = response.bytes_stream().map(move |result| {
            result.map(|bytes| {
                if is_first {
                    is_first = false;
                    if bytes.starts_with(UTF8_BOM) {
                        return bytes.slice(UTF8_BOM.len()..);
                    }
                }
                bytes
            })
        });

        let event_stream = byte_stream.eventsource();

        // Map SSE events into MessageStreamEvent.
        // Uses `scan` so transport errors terminate the stream after the first error (same pattern as `chat_completion_stream`)
        let events = event_stream
            .scan(false, |had_transport_error, event_res| {
                if *had_transport_error {
                    return std::future::ready(None);
                }
                let item = match event_res {
                    Ok(event) => {
                        let data = &event.data;
                        if data == "[DONE]" {
                            return std::future::ready(None);
                        }

                        tracing::info!(
                            target: crate::sampling_log::TARGET,
                            event = "sse_chunk",
                            backend = "messages",
                            data = %data,
                        );

                        if let Some(stream_error) = try_parse_stream_error(data) {
                            Some(Err(stream_error))
                        } else {
                            Some(
                                parse_sse_event::<messages::MessageStreamEvent>(data).map_err(
                                    |e| {
                                        tracing::error!(
                                            error = %e,
                                            raw_data = %data,
                                            "Failed to deserialize MessageStreamEvent from stream"
                                        );
                                        e
                                    },
                                ),
                            )
                        }
                    }
                    Err(e) => {
                        *had_transport_error = true;
                        Some(Err(SamplingError::EventStreamError(sse_error_text(&e))))
                    }
                };
                std::future::ready(item)
            })
            .boxed();

        Ok((
            span_timing.hold_until_first_content(events, message_event_class),
            model_metadata,
        ))
    }

    // =========================================================================
    // Unified Conversation API
    // =========================================================================

    fn apply_conversation_defaults(&self, request: &mut ConversationRequest) -> Result<()> {
        if request.model.is_none() {
            request.model = Some(self.defaults.model.clone());
        }

        if request.temperature.is_none() {
            request.temperature = self.defaults.temperature;
        }

        if request.top_p.is_none() {
            request.top_p = self.defaults.top_p;
        }

        if request.max_output_tokens.is_none() {
            request.max_output_tokens = self.defaults.max_completion_tokens;
        }

        // The per-model config is authoritative for the message schema, and
        // narrows (never widens) whatever the caller asked for. A request
        // carrying an already-narrowed profile — e.g. set by the strict-schema
        // recovery after a 400 — therefore keeps it, while a model configured
        // strict strips the properties even when the caller left the
        // permissive default in place.
        request.chat_message_profile = request
            .chat_message_profile
            .narrowed_by(self.defaults.chat_message_profile);

        // The provider counts the requested output against the same window as
        // the prompt, so the default applied just above is not free: on a large
        // conversation it is what carries the request past the window. Every
        // backend converter reads `max_output_tokens` from here, so this is the
        // last point that can hold the sum inside the window. The estimate is
        // the only prompt size this layer has; a caller that tracks the
        // provider's reported usage fits the budget with that number first, and
        // this only ever cuts further.
        let usable_window =
            xai_token_estimation::window_less_estimate_slack(self.defaults.context_window);
        if let Some(clamp) =
            request.fit_output_budget(request.estimate_prompt_tokens(), usable_window)
        {
            tracing::warn!(
                requested = clamp.requested,
                applied = clamp.applied,
                estimated_prompt_tokens = clamp.prompt_tokens,
                usable_window = clamp.context_window,
                model = %request.model.as_deref().unwrap_or_default(),
                "output budget exceeded the context window with the prompt; cut it to fit"
            );
        }

        Ok(())
    }

    /// Send a conversation request using the Chat Completions API (streaming).
    pub async fn conversation_stream(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<ChatCompletionChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let mut chat_request: ChatCompletionRequest = request.into();
        if let Some(trace) = trace {
            chat_request.trace = Some(trace);
        }

        self.chat_completion_stream(chat_request).await
    }

    /// Send a conversation request using the Chat Completions API (non-streaming).
    pub async fn conversation(
        &self,
        mut request: ConversationRequest,
    ) -> Result<ChatCompletionResponse> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let mut chat_request: ChatCompletionRequest = request.into();
        if let Some(trace) = trace {
            chat_request.trace = Some(trace);
        }

        self.chat_completion(chat_request).await
    }

    /// Send a conversation request using the Responses API (streaming).
    /// The third tuple element is the per-request doom-loop signal collector (see [`Self::create_response_stream`]).
    /// Callers that don't consume the signals can ignore it.
    #[allow(clippy::type_complexity)]
    pub async fn conversation_stream_responses(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<rs::ResponseStreamEvent>>,
        Option<ResponseModelMetadata>,
        Option<crate::doom_loop::DoomLoopSignalCollector>,
    )> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_transient_retry = request.x_grok_transient_retry.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        // The hosted tools travel as raw JSON, spliced in after serialization by `splice_extra_tool_entries`, whose doc explains why each one does
        let extra_tools = xai_grok_sampling_types::extra_tool_entries(&request.hosted_tools);

        let responses_request: rs::CreateResponse = (&request).into();

        let mut wrapper = CreateResponseWrapper::new(responses_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_transient_retry = x_grok_transient_retry;
        wrapper.x_grok_agent_id = x_grok_agent_id;
        wrapper.extra_tool_entries = extra_tools;
        wrapper.traceparent = request.traceparent;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_response_stream(wrapper).await
    }

    /// Send a conversation request using the Responses API (non-streaming).
    pub async fn conversation_responses(
        &self,
        mut request: ConversationRequest,
    ) -> Result<rs::Response> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_transient_retry = request.x_grok_transient_retry.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        // The hosted tools travel as raw JSON, spliced in by `create_response` via `splice_extra_tool_entries`, whose doc explains why
        let extra_tools = xai_grok_sampling_types::extra_tool_entries(&request.hosted_tools);

        let responses_request: rs::CreateResponse = (&request).into();

        let mut wrapper = CreateResponseWrapper::new(responses_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_transient_retry = x_grok_transient_retry;
        wrapper.x_grok_agent_id = x_grok_agent_id;
        wrapper.extra_tool_entries = extra_tools;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_response(wrapper).await
    }

    /// Send a conversation request using the Anthropic Messages API (streaming).
    pub async fn conversation_stream_messages(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<messages::MessageStreamEvent>>,
        Option<ResponseModelMetadata>,
    )> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_transient_retry = request.x_grok_transient_retry.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        let messages_request = build_messages_request(&request);

        let mut wrapper = MessagesRequestWrapper::new(messages_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_transient_retry = x_grok_transient_retry;
        wrapper.x_grok_agent_id = x_grok_agent_id;
        wrapper.traceparent = request.traceparent;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_message_stream(wrapper).await
    }

    /// Stream a conversation through Ollama's native `/api/chat`.
    ///
    /// The response is NDJSON: one whole JSON object per line, no SSE framing
    /// and no `[DONE]` sentinel. Lines are reassembled here because a chunk
    /// boundary lands at an arbitrary byte, so a line can straddle two of
    /// them.
    pub async fn conversation_stream_ollama(
        &self,
        mut request: ConversationRequest,
    ) -> Result<(
        BoxStream<'static, Result<xai_grok_sampling_types::ollama::OllamaChatChunk>>,
        Option<ResponseModelMetadata>,
    )> {
        use xai_grok_sampling_types::build_ollama_chat_request;

        self.apply_conversation_defaults(&mut request)?;
        request.trace.take();

        let model_id = request.model.clone().unwrap_or_default();
        let chat_request = build_ollama_chat_request(&request);

        let mut body = serde_json::to_value(&chat_request).map_err(SamplingError::Serialization)?;
        // `keep_alive`, `truncate` and `options.num_ctx` reach the wire from
        // here and nowhere else: they are per-deployment settings with no
        // cross-provider meaning, so they live in config rather than in the
        // typed request.
        xai_grok_sampling_types::merge_extra_body(&mut body, &self.extra_body);

        let endpoint = self.endpoint("api/chat");
        tracing::debug!(url = %endpoint, model_id = %model_id, "Sending ollama /api/chat request");
        let SentRequest {
            builder,
            sent_bearer,
        } = self.post(endpoint.clone());
        let built_request = builder
            .header(ACCEPT, HeaderValue::from_static("application/x-ndjson"))
            .json(&body)
            .build()
            .map_err(|e| {
                tracing::error!("Failed to build HTTP request: {}", e);
                SamplingError::Http(e)
            })?;
        Self::log_request_headers(&built_request, "ollama");

        let response = self.send(built_request).await?;

        let status = response.status();
        let request_url = response.url().to_string();
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.record_401_attribution(
                    crate::attribution::SamplingConsumer::ChatCompletionsStream,
                    sent_bearer.as_deref(),
                );
                let body = response.bytes().await.unwrap_or_default();
                let server_message = user_facing_api_error_message(status, body.as_ref());
                return Err(auth_rejected(
                    format!("Unauthorized (401) from {endpoint}: {server_message}"),
                    sent_bearer.as_deref(),
                ));
            }
            let model_metadata = extract_model_metadata(response.headers());
            let retry_after_secs = extract_retry_after(response.headers());
            let should_retry = extract_should_retry(response.headers());
            let bytes = read_body(response, status).await?;
            let message = api_error_message_for_endpoint(status, bytes.as_ref(), &request_url);
            tracing::error!(
                status = %status,
                error_message = %message,
                model_id = %model_id,
                "ollama API error"
            );
            return Err(SamplingError::Api {
                status,
                message,
                model_metadata,
                retry_after_secs,
                should_retry,
                error_code: parse_error_code(bytes.as_ref()),
            });
        }

        let model_metadata = extract_model_metadata(response.headers());
        let chunks = ndjson_chunk_stream(response.bytes_stream()).boxed();
        Ok((chunks, model_metadata))
    }

    /// Send a conversation request using the Anthropic Messages API (non-streaming).
    pub async fn conversation_messages(
        &self,
        mut request: ConversationRequest,
    ) -> Result<messages::MessagesResponse> {
        self.apply_conversation_defaults(&mut request)?;

        let trace = request.trace.take();
        let x_grok_conv_id = request.x_grok_conv_id.clone();
        let x_grok_req_id = request.x_grok_req_id.clone();
        let x_grok_session_id = request.x_grok_session_id.clone();
        let x_grok_turn_idx = request.x_grok_turn_idx.clone();
        let x_grok_transient_retry = request.x_grok_transient_retry.clone();
        let x_grok_agent_id = request.x_grok_agent_id.clone();

        let messages_request = build_messages_request(&request);

        let mut wrapper = MessagesRequestWrapper::new(messages_request);
        wrapper.x_grok_conv_id = x_grok_conv_id;
        wrapper.x_grok_req_id = x_grok_req_id;
        wrapper.x_grok_session_id = x_grok_session_id;
        wrapper.x_grok_turn_idx = x_grok_turn_idx;
        wrapper.x_grok_transient_retry = x_grok_transient_retry;
        wrapper.x_grok_agent_id = x_grok_agent_id;

        if let Some(trace) = trace {
            wrapper.trace = Some(trace);
        }

        self.create_message(wrapper).await
    }

    /// Backend-aware streaming call that collects the full response.
    /// Honors the request's [`LengthPolicy`](xai_grok_sampling_types::LengthPolicy) like the actor path.
    /// The default still fails a text-only or empty `Length` stop, so side callers never persist a silently truncated result.
    pub async fn conversation_collect(
        &self,
        request: ConversationRequest,
    ) -> Result<ConversationResponse> {
        self.conversation_collect_with_idle_timeout(request, std::time::Duration::from_secs(300))
            .await
    }

    /// [`Self::conversation_collect`] with a caller-chosen idle timeout, for short side calls (autocomplete, memory notes) that must give up fast.
    pub async fn conversation_collect_with_idle_timeout(
        &self,
        request: ConversationRequest,
        idle_timeout: std::time::Duration,
    ) -> Result<ConversationResponse> {
        let request_id = crate::types::RequestId::random();
        let length_policy = request.length_policy;
        let result = match self.api_backend() {
            ApiBackend::ChatCompletions => {
                let (raw, meta) = self.conversation_stream(request).await?;
                let events =
                    crate::stream::stream_chat_completions(raw, meta, request_id, idle_timeout);
                crate::stream::collect_response(events).await
            }
            ApiBackend::Responses => {
                let (raw, meta, doom_loop) = self.conversation_stream_responses(request).await?;
                let events =
                    crate::stream::stream_responses(raw, meta, request_id, idle_timeout, doom_loop);
                crate::stream::collect_response(events).await
            }
            ApiBackend::Messages => {
                let (raw, meta) = self.conversation_stream_messages(request).await?;
                let events = crate::stream::stream_messages(raw, meta, request_id, idle_timeout);
                crate::stream::collect_response(events).await
            }
            ApiBackend::Ollama => {
                let (raw, meta) = self.conversation_stream_ollama(request).await?;
                let events = crate::stream::stream_ollama(raw, meta, request_id, idle_timeout);
                crate::stream::collect_response(events).await
            }
        };
        let response = result
            .map(|(response, _metrics)| response)
            .map_err(stream_collect_error)?;
        apply_length_policy(length_policy, response)
    }
}

/// The host root Ollama's native API lives under.
///
/// A provider names ONE base URL and it points at the OpenAI-compatible
/// endpoint, because that is what every other client wants. `/api/chat` is a
/// sibling of that endpoint rather than a child, so the suffix comes off.
fn native_host_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    for suffix in ["/api/v1", "/api/v0", "/v1", "/api"] {
        if let Some(root) = trimmed.strip_suffix(suffix) {
            return root.trim_end_matches('/').to_owned();
        }
    }
    trimmed.to_owned()
}

/// The text of an SSE stream failure, with the transport's full cause chain.
fn sse_error_text(error: &eventsource_stream::EventStreamError<reqwest::Error>) -> String {
    match error {
        eventsource_stream::EventStreamError::Transport(inner) => error_chain(inner),
        other => other.to_string(),
    }
}

/// Parse an NDJSON byte stream into a single object for each line.
///
/// A transport chunk boundary lands at an arbitrary byte, so a line can
/// straddle two of them and the tail has to be carried across. The final line
/// often arrives without a trailing newline, so what is left in the buffer at
/// end of stream is a line too.
fn ndjson_chunk_stream<S, B, T>(byte_stream: S) -> impl Stream<Item = Result<T>> + Send
where
    S: Stream<Item = std::result::Result<B, reqwest::Error>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    T: serde::de::DeserializeOwned + Send + 'static,
{
    async_stream::stream! {
        let mut buffer: Vec<u8> = Vec::new();
        let mut stream = std::pin::pin!(byte_stream);
        let mut failed = false;

        while let Some(next) = stream.next().await {
            let bytes = match next {
                Ok(bytes) => bytes,
                Err(error) => {
                    // A body that stopped arriving mid-read is transient, and
                    // the sampler's stream-interrupt budget is what answers
                    // it; reporting it as a finished response would hand the
                    // turn a truncated answer.
                    failed = true;
                    yield Err(SamplingError::EventStreamError(error_chain(&error)));
                    break;
                }
            };
            buffer.extend_from_slice(bytes.as_ref());

            while let Some(newline) = buffer.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=newline).collect();
                if let Some(item) = parse_ndjson_line(&line[..line.len() - 1]) {
                    yield item;
                }
            }
        }

        if !failed && !buffer.is_empty() {
            if let Some(item) = parse_ndjson_line(&buffer) {
                yield item;
            }
        }
    }
}

/// Parse one NDJSON line, or `None` for a line carrying nothing.
fn parse_ndjson_line<T: serde::de::DeserializeOwned>(line: &[u8]) -> Option<Result<T>> {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    tracing::info!(
        target: crate::sampling_log::TARGET,
        event = "ndjson_chunk",
        backend = "ollama",
        data = %text,
    );
    match serde_json::from_str::<T>(text) {
        Ok(item) => Some(Ok(item)),
        Err(error) => {
            tracing::error!(%error, raw_data = %text, "failed to deserialize an NDJSON line");
            Some(Err(SamplingError::StreamError {
                error_type: "malformed_ndjson".to_owned(),
                message: format!("malformed NDJSON line from the model endpoint: {error}"),
                code: None,
            }))
        }
    }
}

/// Read a whole response body. The error that reqwest gives for a body that
/// stops mid-read carries no status, so this logs the status that the
/// headers already gave.
async fn read_body(
    response: reqwest::Response,
    status: reqwest::StatusCode,
) -> Result<bytes::Bytes> {
    let url = response.url().to_string();
    response.bytes().await.map_err(|error| {
        tracing::error!(
            status = %status,
            url = %url,
            %error,
            "the response body could not be read"
        );
        SamplingError::Http(error)
    })
}

/// Rebuild `Api` from stream-collected info, preserving status,
/// `Retry-After`, and `x-should-retry` (kind is lost on this path).
/// Applies the request's [`xai_grok_sampling_types::LengthPolicy`] to a collected response.
/// Fails a `Length` stop the policy rejects, logs the salvage breadcrumb otherwise.
/// The single gate shared by `drive_l2` and the direct-collect path so the two cannot drift.
pub(crate) fn apply_length_policy(
    policy: xai_grok_sampling_types::LengthPolicy,
    response: xai_grok_sampling_types::ConversationResponse,
) -> Result<xai_grok_sampling_types::ConversationResponse> {
    use xai_grok_sampling_types::LengthVerdict;
    match policy.verdict(&response) {
        LengthVerdict::Pass => Ok(response),
        LengthVerdict::Fail => Err(SamplingError::MaxTokensTruncation),
        LengthVerdict::Salvage => {
            // Breadcrumb for "why did the user get half an answer".
            tracing::info!(
                content_len = response.assistant().map_or(0, |a| a.content.len()),
                completion_tokens = response.usage.as_ref().map(|u| u.completion_tokens),
                "salvaging Length-truncated response per LengthPolicy::CompletePartial"
            );
            Ok(response)
        }
        LengthVerdict::SalvageToolCalls => {
            // Breadcrumb for counting turns rescued from max_tokens_truncation.
            tracing::info!(
                tool_calls = response.tool_calls().len(),
                content_len = response.assistant().map_or(0, |a| a.content.len()),
                completion_tokens = response.usage.as_ref().map(|u| u.completion_tokens),
                "completing Length-truncated response with completed tool calls"
            );
            Ok(response)
        }
    }
}

/// Rebuild `Api` from stream-collected info, preserving status, `Retry-After`, and `x-should-retry` (kind is lost on this path).
fn stream_collect_error(info: SamplingErrorInfo) -> SamplingError {
    SamplingError::Api {
        status: info
            .status_code
            .and_then(|c| reqwest::StatusCode::from_u16(c).ok())
            .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
        message: info.message,
        model_metadata: info.model_metadata,
        retry_after_secs: info.retry_after_secs,
        should_retry: info.should_retry,
        error_code: info.error_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nth<T>(xs: &[T], i: usize) -> &T {
        let Some(x) = xs.get(i) else {
            panic!("expected item {i}, got {} items", xs.len());
        };
        x
    }
    use axum::{Router, body::Bytes, routing::post};
    use indexmap::IndexMap;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use xai_grok_sampling_types::ApiErrorCode;
    use xai_grok_sampling_types::types::ChatRequestMessage;

    /// The sampler's own default output budget is what a caller that sets none
    /// sends, so the default is where an impossible request is born: the
    /// provider adds it to the prompt and rejects the sum. Nothing downstream
    /// can fix that, so the sum is held inside the window here.
    #[test]
    fn the_default_output_budget_cannot_carry_a_request_past_the_window() {
        let mut cfg = minimal_config();
        cfg.context_window = 1_000_000;
        cfg.max_completion_tokens = Some(262_144);
        let client = SamplingClient::new(cfg).expect("client should build");

        // ~737_857 estimated prompt tokens, the size the server reported.
        let mut request =
            ConversationRequest::from_items(vec![xai_grok_sampling_types::ConversationItem::user(
                "x".repeat(737_857 * 4),
            )]);
        client
            .apply_conversation_defaults(&mut request)
            .expect("defaults apply");

        let budget = u64::from(request.max_output_tokens.expect("a budget is applied"));
        assert!(
            budget < 262_144,
            "the default must be cut, not sent whole: {budget}"
        );
        assert!(
            request.estimate_prompt_tokens() + budget <= 1_000_000,
            "prompt + output must fit the window"
        );
    }

    /// The same default on an ordinary conversation is the configured one.
    #[test]
    fn a_conversation_with_room_keeps_the_configured_output_budget() {
        let mut cfg = minimal_config();
        cfg.context_window = 1_000_000;
        cfg.max_completion_tokens = Some(262_144);
        let client = SamplingClient::new(cfg).expect("client should build");

        let mut request =
            ConversationRequest::from_items(vec![xai_grok_sampling_types::ConversationItem::user(
                "hello",
            )]);
        client
            .apply_conversation_defaults(&mut request)
            .expect("defaults apply");
        assert_eq!(request.max_output_tokens, Some(262_144));
    }

    /// The banner a user actually reads is this message, so a rejected payload
    /// has to say WHICH field it rejected. Both wire surfaces are covered: a
    /// chunk (derived struct, positioned error) and a Responses event
    /// (internally tagged, so serde buffers it and drops the position).
    #[test]
    fn a_rejected_payload_names_the_field_that_failed() {
        let chunk = from_sse_payload::<ChatCompletionChunk>(
            r#"{"id":"c","object":"chat.completion.chunk","created":0,"model":"m",
                "choices":[{"index":0,"delta":{"content":[]}}]}"#,
        )
        .expect_err("an array where content wants a string must be rejected");
        assert!(
            chunk.to_string().contains("choices[0].delta.content"),
            "chunk error must name the field: {chunk}"
        );

        // An internally tagged event gets no path from serde, so the message
        // falls back to where the payload's nulls are — the one thing that
        // makes a bare "expected a sequence" actionable.
        let event = from_sse_payload::<rs::ResponseStreamEvent>(
            r#"{"type":"response.created","sequence_number":0,
                "response":{"id":"r","object":"response","created_at":0,"model":"m",
                            "status":"in_progress","output":null}}"#,
        )
        .expect_err("a null output list must be rejected by the strict parse");
        let event = event.to_string();
        assert!(
            event.contains("response.output"),
            "event error must locate the null: {event}"
        );
        assert!(
            event.contains("invalid type: null"),
            "and must keep serde's own reason: {event}"
        );
    }

    /// The same event the strict parse rejects above must come back alive from
    /// the retry, so naming the failure never replaces surviving it.
    #[test]
    fn a_null_list_is_rescued_on_every_surface() {
        let event = parse_sse_event::<rs::ResponseStreamEvent>(
            r#"{"type":"response.created","sequence_number":0,
                "response":{"id":"r","object":"response","created_at":0,"model":"m",
                            "status":"in_progress","output":null,"tools":null}}"#,
        )
        .expect("responses: a null output list must be rescued");
        assert!(matches!(event, rs::ResponseStreamEvent::ResponseCreated(_)));

        let chunk = parse_sse_event::<ChatCompletionChunk>(
            r#"{"id":"c","object":"chat.completion.chunk","created":0,"model":"m",
                "choices":null,"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#,
        )
        .expect("chat completions: a null choices list must be rescued");
        assert!(chunk.choices.is_empty());
        assert_eq!(chunk.usage.map(|u| u.total_tokens), Some(3));

        let start = parse_sse_event::<messages::MessageStreamEvent>(
            r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant",
                "content":null,"model":"m","stop_reason":null,
                "usage":{"input_tokens":1,"output_tokens":0}}}"#,
        )
        .expect("messages: a null content list must be rescued");
        assert!(matches!(
            start,
            messages::MessageStreamEvent::MessageStart { .. }
        ));
    }

    #[test]
    fn splice_extra_tool_entries_extends_existing_tools_array() {
        let mut body = serde_json::json!({ "tools": [{ "type": "function" }] });
        splice_extra_tool_entries(&mut body, vec![serde_json::json!({ "type": "web_search" })]);
        assert_eq!(
            body.get("tools"),
            Some(&serde_json::json!([{ "type": "function" }, { "type": "web_search" }]))
        );
    }

    #[test]
    fn splice_extra_tool_entries_creates_tools_array_when_absent() {
        let mut body = serde_json::json!({});
        splice_extra_tool_entries(&mut body, vec![serde_json::json!({ "type": "web_search" })]);
        assert_eq!(
            body.get("tools"),
            Some(&serde_json::json!([{ "type": "web_search" }]))
        );
    }

    #[test]
    fn splice_extra_tool_entries_noop_when_empty() {
        let mut body = serde_json::json!({ "tools": [{ "type": "function" }] });
        splice_extra_tool_entries(&mut body, vec![]);
        assert_eq!(
            body.get("tools"),
            Some(&serde_json::json!([{ "type": "function" }]))
        );
    }

    #[test]
    fn stream_collect_error_preserves_should_retry() {
        let info = SamplingErrorInfo {
            kind: crate::events::SamplingErrorKind::Api,
            status_code: Some(529),
            message: "Overloaded".into(),
            is_retryable: true,
            retry_after_secs: Some(3),
            should_retry: Some(false),
            error_code: Some(ApiErrorCode::InvalidImage),
            model_metadata: None,
            empty_response_context: None,
            doom_loop_triggers: None,
            doom_loop_aborted_at_chunk: None,
            output_rate: None,
            credential: xai_grok_sampling_types::SentCredential::Unknown,
        };
        // SamplingError is not PartialEq (it carries reqwest/serde errors), so destructure once and compare all fields in a single assert
        let SamplingError::Api {
            status,
            message,
            model_metadata,
            retry_after_secs,
            should_retry,
            error_code,
        } = stream_collect_error(info)
        else {
            panic!("expected Api");
        };
        assert_eq!(
            (
                status.as_u16(),
                message.as_str(),
                model_metadata.is_none(),
                retry_after_secs,
                should_retry,
                error_code,
            ),
            (
                529,
                "Overloaded",
                true,
                Some(3),
                Some(false),
                Some(ApiErrorCode::InvalidImage)
            ),
        );
    }

    fn minimal_config() -> SamplerConfig {
        SamplerConfig {
            api_key: Some("test-key".to_string()),
            base_url: "https://example.test".to_string(),
            model: "test-model".to_string(),
            context_window: 8192,
            ..Default::default()
        }
    }

    /// The serialized StreamingChatRequest flattens all ChatCompletionRequest fields at top level.
    /// The wrapper adds `stream: true` and `stream_options.include_usage: true`.
    #[test]
    fn streaming_chat_request_serializes_correctly() {
        let request = ChatCompletionRequest {
            model: Some("test-model".into()),
            messages: vec![ChatRequestMessage::user("hello")],
            temperature: Some(0.7),
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
        };

        let wrapper = StreamingChatRequest {
            inner: &request,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };

        let json: serde_json::Value = serde_json::to_value(&wrapper).unwrap();
        let obj = json.as_object().unwrap();

        assert_eq!(obj.get("stream").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            obj.get("stream_options")
                .and_then(|v| v.get("include_usage"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(
            !obj.keys().any(|k| k.starts_with("x_grok_")),
            "x_grok_* are header fields and must never serialize into the body: {:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(
            obj.get("traceparent").is_none(),
            "traceparent rides the span, never the body"
        );

        assert!(
            obj.get("inner").is_none(),
            "inner field should be flattened"
        );
        assert_eq!(
            obj.get("model").and_then(|v| v.as_str()),
            Some("test-model")
        );
        assert!(obj.get("messages").is_some());
        let temp = obj.get("temperature").and_then(|v| v.as_f64()).unwrap();
        assert!((temp - 0.7).abs() < 0.001, "temperature should be ~0.7");

        assert!(obj.get("max_tokens").is_none());
        assert!(obj.get("tools").is_none());
    }

    const EMPTY_RESPONSE_JSON: &str = r#"{"id":"resp","object":"response","created_at":0,"model":"test-model","status":"completed","output":[],"usage":{"input_tokens":0,"input_tokens_details":{"cached_tokens":0},"output_tokens":0,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":0}}"#;

    async fn capture_response_body(streaming: bool) -> serde_json::Value {
        let (body_tx, body_rx) = oneshot::channel();
        let body_tx = std::sync::Arc::new(std::sync::Mutex::new(Some(body_tx)));
        let app = Router::new().route(
            "/v1/responses",
            post(move |body: Bytes| {
                let body_tx = body_tx.clone();
                async move {
                    let _ = body_tx.lock().unwrap().take().unwrap().send(body);
                    if streaming {
                        axum::response::Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(axum::body::Body::from("data: [DONE]\n\n"))
                            .unwrap()
                    } else {
                        axum::response::Response::builder()
                            .header("content-type", "application/json")
                            .body(axum::body::Body::from(EMPTY_RESPONSE_JSON))
                            .unwrap()
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = SamplingClient::new(SamplerConfig {
            base_url: format!("http://{addr}/v1"),
            api_backend: ApiBackend::Responses,
            extra_response_includes: vec!["no_inline_citations".to_owned()],
            ..minimal_config()
        })
        .unwrap();
        let mut request = rs::CreateResponse {
            input: rs::InputParam::Text("hi".to_owned()),
            include: Some(vec![rs::IncludeEnum::ReasoningEncryptedContent]),
            tools: Some(vec![rs::Tool::WebSearch(rs::WebSearchTool::default())]),
            ..Default::default()
        };
        let mut wrapper = CreateResponseWrapper::new(request.clone());
        wrapper.extra_tool_entries = vec![serde_json::json!({"type": "x_search"})];
        if streaming {
            let (_stream, _model_metadata, _doom_loop_collector) = client
                .create_response_stream(wrapper)
                .await
                .expect("streaming request should succeed");
        } else {
            request.tools = None;
            client
                .create_response(CreateResponseWrapper::new(request))
                .await
                .expect("unary request should succeed");
        }
        let body = body_rx.await.unwrap();
        server.abort();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn response_call_sites_emit_final_includes_and_stream_fields() {
        let unary = capture_response_body(false).await;
        assert_eq!(
            Some(&serde_json::json!([
                "reasoning.encrypted_content",
                "no_inline_citations"
            ])),
            unary.get("include"),
        );

        let stream = capture_response_body(true).await;
        assert_eq!(
            Some(&serde_json::json!([
                "reasoning.encrypted_content",
                "no_inline_citations"
            ])),
            stream.get("include"),
        );
        assert_eq!(Some(true), stream.get("stream").and_then(|v| v.as_bool()));
        assert!(
            stream
                .get("tools")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .any(|tool| tool.get("type") == Some(&serde_json::json!("x_search")))
        );
    }

    const EMPTY_CHAT_COMPLETION_JSON: &str =
        r#"{"id":"chat","object":"chat.completion","created":0,"model":"test-model","choices":[]}"#;
    const EMPTY_MESSAGE_JSON: &str = r#"{"id":"msg","type":"message","role":"assistant","content":[],"model":"test-model","stop_reason":"end_turn","usage":{"input_tokens":0,"output_tokens":0}}"#;

    /// One conversation request through `backend` (unary or SSE) against a mock
    /// that hands back the request's headers and raw body.
    async fn capture_request(
        backend: ApiBackend,
        streaming: bool,
        request_compression: RequestCompression,
        input: &str,
    ) -> (axum::http::HeaderMap, Bytes) {
        use xai_grok_sampling_types::{ContentPart, ConversationItem, UserItem};

        let (content_type, reply) = match (streaming, &backend) {
            (true, _) => ("text/event-stream", "data: [DONE]\n\n"),
            (false, ApiBackend::Responses) => ("application/json", EMPTY_RESPONSE_JSON),
            (false, ApiBackend::ChatCompletions) => {
                ("application/json", EMPTY_CHAT_COMPLETION_JSON)
            }
            (false, ApiBackend::Messages) => ("application/json", EMPTY_MESSAGE_JSON),
            (false, ApiBackend::Ollama) => unreachable!("capture_request does not cover Ollama"),
        };
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
        let handler = post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx.lock().unwrap().take().unwrap().send((headers, body));
                axum::response::Response::builder()
                    .header("content-type", content_type)
                    .body(axum::body::Body::from(reply))
                    .unwrap()
            }
        });
        let app = Router::new()
            .route("/v1/chat/completions", handler.clone())
            .route("/v1/responses", handler.clone())
            .route("/v1/messages", handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let client = SamplingClient::new(SamplerConfig {
            base_url: format!("http://{addr}/v1"),
            api_backend: backend.clone(),
            request_compression,
            ..minimal_config()
        })
        .unwrap();
        let request = ConversationRequest {
            items: vec![ConversationItem::User(UserItem {
                content: vec![ContentPart::Text {
                    text: Arc::from(input),
                }],
                ..Default::default()
            })],
            ..Default::default()
        };
        let sent = match (streaming, &backend) {
            (false, ApiBackend::ChatCompletions) => client.conversation(request).await.map(drop),
            (true, ApiBackend::ChatCompletions) => {
                client.conversation_stream(request).await.map(drop)
            }
            (false, ApiBackend::Responses) => {
                client.conversation_responses(request).await.map(drop)
            }
            (true, ApiBackend::Responses) => client
                .conversation_stream_responses(request)
                .await
                .map(drop),
            (false, ApiBackend::Messages) => client.conversation_messages(request).await.map(drop),
            (true, ApiBackend::Messages) => {
                client.conversation_stream_messages(request).await.map(drop)
            }
            (_, ApiBackend::Ollama) => unreachable!("capture_request does not cover Ollama"),
        };
        sent.unwrap_or_else(|e| panic!("{backend:?} streaming={streaming}: {e}"));
        let captured = rx.await.unwrap();
        server.abort();
        captured
    }

    fn large_input() -> String {
        "x".repeat(2 * crate::request_compression::MIN_COMPRESS_BYTES)
    }

    fn header<'a>(headers: &'a axum::http::HeaderMap, name: HeaderName) -> Option<&'a str> {
        headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[tokio::test]
    async fn every_chat_route_compresses_large_bodies_when_configured() {
        let input = large_input();
        for backend in [
            ApiBackend::ChatCompletions,
            ApiBackend::Responses,
            ApiBackend::Messages,
        ] {
            for streaming in [false, true] {
                let route = format!("{backend:?} streaming={streaming}");
                let (headers, body) =
                    capture_request(backend.clone(), streaming, RequestCompression::Zstd, &input)
                        .await;
                assert_eq!(Some("zstd"), header(&headers, CONTENT_ENCODING), "{route}");
                assert_eq!(
                    Some("application/json"),
                    header(&headers, CONTENT_TYPE),
                    "{route}: the encoding wraps a JSON body"
                );
                // cli-chat-proxy rejects a zstd body it cannot attribute from headers.
                assert!(
                    header(&headers, HeaderName::from_static("x-grok-model-override"))
                        .is_some_and(|model| !model.is_empty()),
                    "{route}: a compressed body must carry the model override"
                );
                assert!(
                    body.len() < input.len() / 10,
                    "{route}: zstd body should shrink the padding"
                );
                let decoded = String::from_utf8(zstd::decode_all(body.as_ref()).unwrap()).unwrap();
                serde_json::from_str::<serde_json::Value>(&decoded).expect("decoded body is JSON");
                assert!(decoded.contains(&input), "{route}: payload lost");
            }
        }
    }

    /// Bodies past the offload threshold compress on the blocking pool; the
    /// wire result must be indistinguishable from the inline path.
    #[tokio::test]
    async fn offloaded_large_body_compresses_like_the_inline_path() {
        let input = "y".repeat(3 * 1024 * 1024);
        let (headers, body) = capture_request(
            ApiBackend::Responses,
            false,
            RequestCompression::Zstd,
            &input,
        )
        .await;
        assert_eq!(Some("zstd"), header(&headers, CONTENT_ENCODING));
        let decoded = String::from_utf8(zstd::decode_all(body.as_ref()).unwrap()).unwrap();
        assert!(decoded.contains(&input), "payload lost on the offload path");
    }

    #[tokio::test]
    async fn small_body_is_sent_plain_even_when_configured() {
        let (headers, body) = capture_request(
            ApiBackend::Responses,
            false,
            RequestCompression::Zstd,
            "small-plain-body",
        )
        .await;
        assert_eq!(None, header(&headers, CONTENT_ENCODING));
        assert_eq!(Some("application/json"), header(&headers, CONTENT_TYPE));
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("small-plain-body")
        );
    }

    #[tokio::test]
    async fn large_body_stays_plain_when_not_configured() {
        let input = large_input();
        let (headers, body) = capture_request(
            ApiBackend::Responses,
            true,
            RequestCompression::None,
            &input,
        )
        .await;
        assert_eq!(None, header(&headers, CONTENT_ENCODING));
        assert_eq!(Some("application/json"), header(&headers, CONTENT_TYPE));
        assert!(std::str::from_utf8(&body).unwrap().contains(&input));
    }

    #[test]
    fn append_response_includes_preserves_typed_values_and_deduplicates() {
        let typed = [
            "reasoning.encrypted_content",
            "web_search_call.action.sources",
        ];
        let mut body = serde_json::json!({ "include": typed });
        append_response_includes(
            &mut body,
            &[
                "no_inline_citations".to_owned(),
                "no_inline_citations".to_owned(),
            ],
        );
        assert_eq!(
            Some(&serde_json::json!([
                "reasoning.encrypted_content",
                "web_search_call.action.sources",
                "no_inline_citations",
            ])),
            body.get("include"),
        );

        let mut unchanged = serde_json::json!({ "include": typed });
        let expected = unchanged.clone();
        append_response_includes(&mut unchanged, &[]);
        assert_eq!(expected, unchanged);

        for mut body in [
            serde_json::json!({}),
            serde_json::json!({ "include": null }),
        ] {
            append_response_includes(&mut body, &["no_inline_citations".to_owned()]);
            assert_eq!(
                Some(&serde_json::json!(["no_inline_citations"])),
                body.get("include")
            );
        }
    }

    #[test]
    fn extract_retry_after_parses_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(30));
    }

    #[test]
    fn extract_retry_after_caps_at_120() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3600".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(120));
    }

    #[test]
    fn extract_retry_after_zero_is_valid() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "0".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(0));
    }

    #[test]
    fn extract_retry_after_ignores_http_date() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Fri, 31 Dec 2025 23:59:59 GMT".parse().unwrap(),
        );
        assert_eq!(extract_retry_after(&headers), None);
    }

    #[test]
    fn extract_retry_after_none_when_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(extract_retry_after(&headers), None);
    }

    /// A token bucket answers a breach with the whole window, while its own
    /// reset says when the tokens come back. The sooner one is the wait.
    #[test]
    fn a_token_bucket_reset_beats_the_whole_window() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "60".parse().unwrap());
        headers.insert("x-ratelimit-reset-tokens-minute", "12.4".parse().unwrap());
        assert_eq!(
            extract_retry_after(&headers),
            Some(13),
            "fractions round up"
        );
    }

    /// Several token buckets can be breached at once. The request bucket is
    /// not one of them and must not shorten a token wait.
    #[test]
    fn the_soonest_token_bucket_wins_and_requests_are_ignored() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-ratelimit-reset-uncached-tokens-minute",
            "18s".parse().unwrap(),
        );
        headers.insert("x-ratelimit-reset-tokens-day", "4000".parse().unwrap());
        headers.insert("x-ratelimit-reset-requests-minute", "1".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(18));
    }

    #[test]
    fn a_token_reset_alone_is_the_wait() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-reset-tokens-minute", "7".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(7));
    }

    #[test]
    fn an_unparseable_token_reset_leaves_retry_after_alone() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "45".parse().unwrap());
        headers.insert("x-ratelimit-reset-tokens-minute", "soon".parse().unwrap());
        assert_eq!(extract_retry_after(&headers), Some(45));
    }

    #[test]
    fn extract_should_retry_true() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "true".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(true));
    }

    #[test]
    fn extract_should_retry_true_case_insensitive() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "TRUE".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(true));
    }

    #[test]
    fn extract_should_retry_false() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "false".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), Some(false));
    }

    #[test]
    fn extract_should_retry_unknown_value_is_none() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-should-retry", "banana".parse().unwrap());
        assert_eq!(extract_should_retry(&headers), None);
    }

    #[test]
    fn extract_should_retry_absent_is_none() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(extract_should_retry(&headers), None);
    }

    #[test]
    fn new_with_minimal_config_succeeds() {
        let client = SamplingClient::new(minimal_config()).expect("client should construct");
        assert_eq!(client.api_backend(), ApiBackend::ChatCompletions);
    }

    #[test]
    fn a_blank_base_url_is_refused_before_any_request() {
        for blank in ["", "   "] {
            let mut cfg = minimal_config();
            cfg.base_url = blank.to_string();
            cfg.api_backend = ApiBackend::Messages;
            match SamplingClient::new(cfg) {
                Err(SamplingError::InvalidConfiguration(msg)) => {
                    assert!(msg.contains("base_url"), "{msg}");
                }
                Err(other) => panic!("wrong error for {blank:?}: {other}"),
                Ok(_) => panic!("a blank base_url must not build a client"),
            }
        }
    }

    #[test]
    fn an_endpoint_nobody_allowed_is_refused_before_any_request() {
        for url in [
            "https://cli-chat-proxy.grok.com/v1",
            "https://api.x.ai/v1",
            "https://api.anthropic.com/v1",
        ] {
            let mut cfg = minimal_config();
            cfg.base_url = url.to_string();
            match SamplingClient::new(cfg) {
                Err(SamplingError::EndpointNotAllowed(msg)) => {
                    assert!(msg.contains("allowed_endpoints"), "{msg}");
                }
                Err(other) => panic!("wrong error for {url}: {other}"),
                Ok(_) => panic!("{url} is in no allowlist and must not build a client"),
            }
        }
    }

    #[test]
    fn apply_env_http_headers_resolves_trims_skips_and_overrides() {
        let mut map = IndexMap::new();
        map.insert("x-tenant-token".to_string(), "TENANT".to_string());
        map.insert("x-blank".to_string(), "BLANK".to_string());
        map.insert("x-missing".to_string(), "MISSING".to_string());
        map.insert("x-override".to_string(), "OVERRIDE".to_string());
        map.insert("x invalid".to_string(), "INVALID".to_string());

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-override"),
            HeaderValue::from_static("static"),
        );

        apply_env_http_headers(
            &map,
            |var| match var {
                // Leading space and trailing newline exercise trimming
                "TENANT" => Some(" tenant-secret\n".to_string()),
                "BLANK" => Some("   ".to_string()),
                "OVERRIDE" => Some("from-env".to_string()),
                "INVALID" => Some("value".to_string()),
                _ => None,
            },
            &mut headers,
        );

        assert_eq!(headers.get("x-tenant-token").unwrap(), "tenant-secret");
        assert!(headers.get("x-blank").is_none());
        assert!(headers.get("x-missing").is_none());
        // A resolved env value overrides an existing header of the same name.
        assert_eq!(headers.get("x-override").unwrap(), "from-env");
        // An invalid header name is skipped rather than panicking.
        assert!(headers.get("x invalid").is_none());
    }

    #[test]
    fn endpoint_appends_path_before_a_base_url_query_without_configured_params() {
        let template =
            EndpointTemplate::new("https://gateway.example/v1?api-version=x", &IndexMap::new());
        let url = template.url_for_path("responses");
        assert!(
            url.starts_with("https://gateway.example/v1/responses?"),
            "url: {url}"
        );
        assert!(url.contains("api-version=x"), "url: {url}");
        assert!(!url.contains("x/responses"), "url: {url}");
    }

    #[test]
    fn messages_plus_anthropic_api_key_uses_x_api_key_and_not_authorization() {
        let cfg = SamplerConfig {
            api_key: Some("anthropic-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert!(
            client
                .default_headers
                .get(HeaderName::from_static("x-api-key"))
                .is_some()
        );
        assert!(client.default_headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn messages_plus_bearer_uses_authorization_and_not_x_api_key() {
        let cfg = SamplerConfig {
            api_key: Some("bearer-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::Bearer,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert!(client.default_headers.get(AUTHORIZATION).is_some());
        assert!(
            client
                .default_headers
                .get(HeaderName::from_static("x-api-key"))
                .is_none()
        );
    }

    #[test]
    fn no_auth_ignores_configured_key_and_live_bearer() {
        let cfg = SamplerConfig {
            api_key: Some("must-not-be-sent".to_string()),
            auth_scheme: AuthScheme::None,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver(
                "also-must-not-be-sent",
            ))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest { builder, .. } = client.post("https://example.test/v1/chat/completions");
        let request = builder.build().expect("request should build");

        assert!(request.headers().get(AUTHORIZATION).is_none());
        assert!(request.headers().get("x-api-key").is_none());
        assert_eq!(client.auth_info().auth_type, "none");
        assert!(client.auth_info().auth_prefix.is_none());
    }

    // Regression: a past change dropped User-Agent from sampling requests.
    #[test]
    fn sampling_client_always_has_user_agent() {
        let client = SamplingClient::new(minimal_config()).expect("build");
        assert!(client.default_headers.contains_key(USER_AGENT));
    }

    // Regression: a past change dropped HeaderInjector (traceparent) from sampling requests.
    #[test]
    fn header_injector_is_called_in_post() {
        #[derive(Debug)]
        struct TestInjector;
        impl crate::config::HeaderInjector for TestInjector {
            fn inject(&self, headers: &mut HeaderMap) {
                headers.insert(
                    HeaderName::from_static("traceparent"),
                    HeaderValue::from_static("00-test-trace-id-00"),
                );
            }
        }

        let mut config = minimal_config();
        config.header_injector = Some(std::sync::Arc::new(TestInjector));
        let client = SamplingClient::new(config).expect("build");
        let SentRequest { builder, .. } = client.post("http://localhost/test");
        let req = builder.build().expect("build request");
        assert!(
            req.headers().contains_key("traceparent"),
            "HeaderInjector should inject traceparent into post() requests"
        );
    }

    /// Nothing listens on port 1: each send fails right after the hook runs.
    #[tokio::test]
    async fn stream_span_adopts_request_traceparent_on_every_backend() {
        #[derive(Debug)]
        struct RecordingInjector(tokio::sync::mpsc::UnboundedSender<String>);
        impl crate::config::HeaderInjector for RecordingInjector {
            fn inject(&self, _headers: &mut HeaderMap) {}
            fn set_span_parent(&self, _span: &tracing::Span, traceparent: &str) {
                self.0
                    .send(traceparent.to_owned())
                    .expect("test receiver alive");
            }
        }

        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let injector = Arc::new(RecordingInjector(seen_tx));
        let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let client = {
            let mut config = minimal_config();
            config.base_url = "http://127.0.0.1:1".to_string();
            config.header_injector = Some(injector);
            SamplingClient::new(config).expect("build")
        };
        let request = || ConversationRequest {
            items: vec![xai_grok_sampling_types::ConversationItem::user("hi")],
            traceparent: Some(traceparent.to_owned()),
            ..Default::default()
        };

        // Second registered dispatcher: other tests' threads cannot cache callsite interest as
        // `never` for this one.
        let _interest_pin = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let disabled =
            tracing::subscriber::set_default(tracing::subscriber::NoSubscriber::default());
        let refused = client.conversation_stream(request()).await;
        assert!(refused.is_err(), "chat completions: port 1 refuses");
        assert!(
            seen_rx.try_recv().is_err(),
            "a disabled span must not reach the hook"
        );
        drop(disabled);

        let _subscriber = tracing::subscriber::set_default(tracing_subscriber::Registry::default());
        let refused = client.conversation_stream(request()).await;
        assert!(refused.is_err(), "chat completions: port 1 refuses");
        let refused = client.conversation_stream_responses(request()).await;
        assert!(refused.is_err(), "responses: port 1 refuses");
        let refused = client.conversation_stream_messages(request()).await;
        assert!(refused.is_err(), "messages: port 1 refuses");

        let mut seen = Vec::new();
        while let Ok(tp) = seen_rx.try_recv() {
            seen.push(tp);
        }
        assert_eq!(vec![traceparent; 3], seen);
    }

    #[test]
    fn user_agent_includes_origin_and_agent_product() {
        let origin = OriginClientInfo {
            product: "my-client".to_string(),
            version: Some("1.2.3".to_string()),
        };
        let ua = user_agent_string_for(&origin);
        assert!(ua.contains("my-client/1.2.3"));
        assert!(ua.contains(AGENT_PRODUCT));
    }

    #[test]
    fn user_agent_omits_origin_version_when_absent() {
        let origin = OriginClientInfo {
            product: "my-client".to_string(),
            version: None,
        };
        let ua = user_agent_string_for(&origin);
        // No slash between product and the grok-shell agent product.
        assert!(ua.starts_with("my-client grok-shell/"));
    }

    #[test]
    fn user_agent_collapses_when_origin_matches_agent() {
        let agent_version = xai_grok_version::version().to_string();
        let origin = OriginClientInfo {
            product: AGENT_PRODUCT.to_string(),
            version: Some(agent_version.clone()),
        };
        let ua = user_agent_string_for(&origin);
        // Single product/version slot when the origin and agent match.
        assert!(ua.starts_with(&format!("{}/{}", AGENT_PRODUCT, agent_version)));
    }

    /// Counts callbacks for assertions in the tests below.
    #[derive(Default, Debug)]
    struct CountingCallback {
        invocations: std::sync::Mutex<Vec<(crate::attribution::SamplingConsumer, Option<String>)>>,
    }

    #[derive(Debug)]
    struct StaticBearerResolver(&'static str);

    impl crate::config::BearerResolver for StaticBearerResolver {
        fn current_bearer(&self) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    impl crate::attribution::Auth401AttributionCallback for CountingCallback {
        fn record_401(
            &self,
            consumer: crate::attribution::SamplingConsumer,
            sent_bearer: Option<&str>,
        ) {
            self.invocations
                .lock()
                .unwrap()
                .push((consumer, sent_bearer.map(|s| s.to_string())));
        }
    }

    /// `post()` strips the `"Bearer "` scheme prefix off `Authorization` and captures the tail fragment (see `BEARER_SUFFIX_LEN`).
    #[test]
    fn post_captures_bearer_tail_for_openai_compat() {
        let cfg = SamplerConfig {
            api_key: Some("test-bearer-1234567890".to_string()),
            api_backend: ApiBackend::ChatCompletions,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest {
            sent_bearer: bearer,
            ..
        } = client.post("https://example.test/v1/chat/completions");
        assert_eq!(bearer.as_deref(), Some("r-1234567890"));
        assert_eq!(
            bearer.as_deref().map(str::len),
            Some(crate::attribution::BEARER_SUFFIX_LEN),
        );
    }

    /// `post()` captures `x-api-key` for Messages-API backends and keeps the value's tail fragment.
    #[test]
    fn post_captures_x_api_key_tail_for_messages() {
        let cfg = SamplerConfig {
            api_key: Some("anthropic-key-abc123".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest {
            sent_bearer: bearer,
            ..
        } = client.post("https://example.test/v1/messages");
        assert_eq!(bearer.as_deref(), Some("c-key-abc123"));
        assert_eq!(
            bearer.as_deref().map(str::len),
            Some(crate::attribution::BEARER_SUFFIX_LEN),
        );
    }

    /// `post()` captures `None` when the request carries no auth header.
    #[test]
    fn post_captures_none_when_no_header() {
        let cfg = SamplerConfig {
            api_key: None,
            api_backend: ApiBackend::ChatCompletions,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest {
            sent_bearer: bearer,
            ..
        } = client.post("https://example.test/v1/chat/completions");
        assert!(bearer.is_none());
    }

    /// The race this design closes: a 401 triggers a recovery that rotates the resolver.
    /// A record-time re-read would then attribute a bearer the rejected request never carried.
    /// The attributed fragment must be the one captured when the request was built.
    #[test]
    fn post_capture_is_immune_to_resolver_rotation_after_build() {
        #[derive(Debug)]
        struct RotatingResolver(std::sync::Mutex<String>);
        impl crate::config::BearerResolver for RotatingResolver {
            fn current_bearer(&self) -> Option<String> {
                Some(self.0.lock().unwrap().clone())
            }
        }

        let resolver = std::sync::Arc::new(RotatingResolver(std::sync::Mutex::new(
            "rejected-token-oldtail1".to_string(),
        )));
        let cfg = SamplerConfig {
            api_key: None,
            api_backend: ApiBackend::Responses,
            bearer_resolver: Some(resolver.clone()),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");

        let SentRequest {
            sent_bearer: sent_at_build,
            ..
        } = client.post("https://example.test/v1/responses");
        // The 401 kicks recovery; the resolver rotates before the callback runs.
        *resolver.0.lock().unwrap() = "fresh-token-newtail99".to_string();

        assert_eq!(
            sent_at_build.as_deref(),
            Some("ken-oldtail1"),
            "attribution must describe the bearer the rejected request carried"
        );
        // A record-time re-read would report the rotated token, not the build-time capture.
        assert_eq!(
            client.current_sent_bearer_suffix().as_deref(),
            Some("en-newtail99"),
            "sanity: the build-time capture and a live re-read now differ"
        );
    }

    #[test]
    fn live_bearer_resolver_uses_authorization_for_messages_plus_bearer() {
        let cfg = SamplerConfig {
            api_key: Some("stale-bearer".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::Bearer,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-bearer"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest { builder, .. } = client.post("https://example.test/v1/messages");
        let request = builder.build().expect("request should build");
        let auth = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        assert_eq!(auth, Some("Bearer fresh-bearer"));
        assert!(request.headers().get("x-api-key").is_none());
    }

    /// Regression: `api_key` seeds `default_headers` with `Authorization: Bearer ...`.
    /// With a `bearer_resolver` also set, `post()` must produce exactly one `Authorization` header on the wire.
    /// `RequestBuilder::header(AUTHORIZATION, ...)` appends rather than replaces, causing two identical headers and a 400 from cli-chat-proxy.
    #[test]
    fn post_emits_single_authorization_with_api_key_and_bearer_resolver() {
        let cfg = SamplerConfig {
            api_key: Some("stale-bearer".to_string()),
            api_backend: ApiBackend::Responses,
            auth_scheme: AuthScheme::Bearer,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-bearer"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest { builder, .. } = client.post("https://example.test/v1/responses");
        let request = builder.build().expect("request should build");
        let auth_count = request.headers().get_all(AUTHORIZATION).iter().count();
        assert_eq!(
            auth_count, 1,
            "expected exactly one Authorization header, got {auth_count}"
        );
        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer fresh-bearer"),
        );
    }

    #[test]
    fn live_bearer_resolver_uses_x_api_key_for_messages_plus_anthropic_api_key() {
        let cfg = SamplerConfig {
            api_key: Some("stale-anthropic".to_string()),
            api_backend: ApiBackend::Messages,
            auth_scheme: AuthScheme::XApiKey,
            bearer_resolver: Some(std::sync::Arc::new(StaticBearerResolver("fresh-anthropic"))),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest { builder, .. } = client.post("https://example.test/v1/messages");
        let request = builder.build().expect("request should build");
        let api_key = request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        assert_eq!(api_key, Some("fresh-anthropic"));
        assert!(request.headers().get(AUTHORIZATION).is_none());
    }

    /// The callback receives the `post()`-captured fragment only; the full bearer never crosses the crate boundary.
    #[test]
    fn record_401_attribution_invokes_callback_with_captured_bearer() {
        let cb = std::sync::Arc::new(CountingCallback::default());
        let cb_dyn: crate::attribution::SharedAttributionCallback = cb.clone();
        let cfg = SamplerConfig {
            api_key: Some("the-bearer-1234567890-extra-tail".to_string()),
            api_backend: ApiBackend::ChatCompletions,
            attribution_callback: Some(cb_dyn),
            bearer_resolver: None,
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest { sent_bearer, .. } =
            client.post("https://example.test/v1/chat/completions");
        client.record_401_attribution(
            crate::attribution::SamplingConsumer::ChatCompletionsStream,
            sent_bearer.as_deref(),
        );
        let calls = cb.invocations.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            nth(&calls, 0).0,
            crate::attribution::SamplingConsumer::ChatCompletionsStream
        );
        assert_eq!(nth(&calls, 0).1.as_deref(), Some("0-extra-tail"));
        assert_eq!(
            nth(&calls, 0).1.as_deref().map(str::len),
            Some(crate::attribution::BEARER_SUFFIX_LEN),
        );
    }

    /// When a bearer_resolver is wired but returns `None`, attribution must report no sent bearer (not the construction-time default header seed).
    #[test]
    fn bearer_resolver_none_attribution_ignores_default_headers() {
        #[derive(Debug)]
        struct EmptyResolver;
        impl crate::config::BearerResolver for EmptyResolver {
            fn current_bearer(&self) -> Option<String> {
                None
            }
        }

        let cfg = SamplerConfig {
            api_key: Some("stale-seed-token".to_string()),
            api_backend: ApiBackend::Responses,
            bearer_resolver: Some(std::sync::Arc::new(EmptyResolver)),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        assert_eq!(
            client.current_sent_bearer_suffix(),
            None,
            "resolver None must not attribute a stripped default seed"
        );
    }

    /// A wired bearer_resolver that returns `None` means a hard-expired session with no live access token.
    /// Default Authorization / x-api-key must be stripped so a stale seed key cannot ride the wire.
    #[test]
    fn bearer_resolver_none_strips_default_authorization() {
        #[derive(Debug)]
        struct EmptyResolver;
        impl crate::config::BearerResolver for EmptyResolver {
            fn current_bearer(&self) -> Option<String> {
                None
            }
        }

        let cfg = SamplerConfig {
            api_key: Some("stale-token".to_string()),
            api_backend: ApiBackend::Responses,
            bearer_resolver: Some(std::sync::Arc::new(EmptyResolver)),
            ..minimal_config()
        };
        let client = SamplingClient::new(cfg).expect("client should build");
        let SentRequest {
            builder,
            sent_bearer: sent,
        } = client.post("https://example.test/v1/responses");
        let request = builder.body("").build().expect("request should build");
        assert_eq!(sent, None, "capture must agree: nothing was sent");
        assert!(
            request.headers().get(AUTHORIZATION).is_none(),
            "stale default Authorization must not be sent when resolver is empty"
        );
    }

    /// `response.completed` carrying `usage.context_details.{input_tokens, output_tokens}` rewrites `usage.total_tokens` in place.
    /// The new value is the live context length (`ctx.input + ctx.output`).
    /// Billing fields stay on the wire's cumulative values.
    #[test]
    fn deserialize_response_event_overrides_total_tokens_from_context_details() {
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 6003,
                    "input_tokens_details": { "cached_tokens": 1984 },
                    "output_tokens": 711,
                    "output_tokens_details": { "reasoning_tokens": 388 },
                    "total_tokens": 6714,
                    "context_details": {
                        "input_tokens": 5022,
                        "output_tokens": 571
                    }
                }
            }
        }"#;
        let event = deserialize_response_event(sse).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        // Billing fields stay cumulative, unchanged by context_details
        assert_eq!(usage.input_tokens, 6003);
        assert_eq!(usage.output_tokens, 711);
        assert_eq!(usage.input_tokens_details.cached_tokens, 1984);
        assert_eq!(usage.output_tokens_details.reasoning_tokens, 388);
        // total_tokens is rewritten to ctx.input + ctx.output (5022 + 571), not the wire's cumulative total (6714)
        assert_eq!(usage.total_tokens, 5_593);
    }

    #[test]
    fn deserialize_response_event_stashes_cost_in_metadata() {
        let make = |ticks: i64| {
            format!(
                r#"{{
                "type": "response.completed",
                "sequence_number": 0,
                "response": {{
                    "id": "resp_1", "object": "response", "created_at": 0,
                    "model": "grok-build", "status": "completed", "output": [],
                    "usage": {{
                        "input_tokens": 10,
                        "input_tokens_details": {{ "cached_tokens": 0 }},
                        "output_tokens": 5,
                        "output_tokens_details": {{ "reasoning_tokens": 0 }},
                        "total_tokens": 15,
                        "cost_in_usd_ticks": {ticks}
                    }}
                }}
            }}"#
            )
        };

        let event = deserialize_response_event(&make(78)).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        assert_eq!(
            e.response
                .metadata
                .as_ref()
                .and_then(|m| m.get(COST_USD_TICKS_METADATA_KEY))
                .map(String::as_str),
            Some("78")
        );

        // The REST mapper backfills 0 for unbilled requests: no stash.
        let event = deserialize_response_event(&make(0)).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        assert!(e.response.metadata.is_none());
    }

    #[test]
    fn cost_usd_float_converted_to_ticks_when_no_ticks_field_responses() {
        // OpenRouter-style: `usage.cost` (USD float) with no `cost_in_usd_ticks`.
        // The override must convert the float to integer ticks and stash it
        // in metadata for `stream_responses`.
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 5,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 15,
                    "cost": 0.0000416
                }
            }
        }"#;
        let event = deserialize_response_event(sse).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        // round(0.0000416 * 1e10) = 416_000
        assert_eq!(
            e.response
                .metadata
                .as_ref()
                .and_then(|m| m.get(COST_USD_TICKS_METADATA_KEY))
                .map(String::as_str),
            Some("416000")
        );
    }

    #[test]
    fn cost_usd_ticks_preferred_over_cost_float_responses() {
        // When BOTH are present, the ticks field wins.
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 5,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 15,
                    "cost_in_usd_ticks": 42,
                    "cost": 0.0000999
                }
            }
        }"#;
        let event = deserialize_response_event(sse).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        assert_eq!(
            e.response
                .metadata
                .as_ref()
                .and_then(|m| m.get(COST_USD_TICKS_METADATA_KEY))
                .map(String::as_str),
            Some("42")
        );
    }

    #[test]
    fn deserialize_response_event_total_tokens_unchanged_when_context_details_absent() {
        // Older / non-Responses backends omit `context_details`.
        // `total_tokens` passes through from the wire unchanged.
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 10000,
                    "input_tokens_details": { "cached_tokens": 0 },
                    "output_tokens": 100,
                    "output_tokens_details": { "reasoning_tokens": 0 },
                    "total_tokens": 10100
                }
            }
        }"#;
        let event = deserialize_response_event(sse).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        assert_eq!(usage.total_tokens, 10_100);
    }

    #[test]
    fn deserialize_response_event_total_tokens_unchanged_when_context_details_partial() {
        // Defensive: if the backend ever ships only one of the two context_details fields, we can't know the live context size
        // Leave `total_tokens` on the wire's cumulative value instead of guessing; treating the missing half as 0 would silently under-report
        let sse = r#"{
            "type": "response.completed",
            "sequence_number": 0,
            "response": {
                "id": "resp_1",
                "object": "response",
                "created_at": 0,
                "model": "grok-build",
                "status": "completed",
                "output": [],
                "usage": {
                    "input_tokens": 6003,
                    "input_tokens_details": { "cached_tokens": 1984 },
                    "output_tokens": 711,
                    "output_tokens_details": { "reasoning_tokens": 388 },
                    "total_tokens": 6714,
                    "context_details": {
                        "input_tokens": 5022
                    }
                }
            }
        }"#;
        let event = deserialize_response_event(sse).expect("parse");
        let rs::ResponseStreamEvent::ResponseCompleted(e) = event else {
            panic!("expected ResponseCompleted");
        };
        let usage = e.response.usage.expect("usage present");
        assert_eq!(usage.total_tokens, 6_714);
    }

    #[test]
    fn deserialize_response_event_ignores_context_details_on_non_terminal_events() {
        // Non-terminal events don't carry final usage; even if the backend ever echoed `context_details` on one, we don't touch it
        let sse = r#"{
            "type": "response.output_text.delta",
            "sequence_number": 0,
            "item_id": "item-1",
            "output_index": 0,
            "content_index": 0,
            "delta": "hello",
            "logprobs": []
        }"#;
        let event = deserialize_response_event(sse).expect("non-terminal event parses");
        assert!(matches!(
            event,
            rs::ResponseStreamEvent::ResponseOutputTextDelta(_)
        ));
    }

    /// A request as the builder emits it: `reasoning.summary` already set to the built-in default.
    fn built_response_request() -> CreateResponseWrapper {
        CreateResponseWrapper::new(rs::CreateResponse {
            reasoning: Some(rs::Reasoning {
                effort: Some(rs::ReasoningEffort::High),
                summary: Some(rs::ReasoningSummary::Concise),
            }),
            ..Default::default()
        })
    }

    fn client_with_summary(
        summary: Option<xai_grok_sampling_types::ReasoningSummary>,
    ) -> SamplingClient {
        SamplingClient::new(SamplerConfig {
            reasoning_summary: summary,
            ..minimal_config()
        })
        .expect("client should construct")
    }

    #[test]
    fn reasoning_summary_unset_keeps_the_built_request() {
        let client = client_with_summary(None);
        let mut request = built_response_request();
        client.apply_response_defaults(&mut request).unwrap();
        let reasoning = request.inner.reasoning.expect("reasoning block kept");
        assert_eq!(reasoning.effort, Some(rs::ReasoningEffort::High));
        assert_eq!(reasoning.summary, Some(rs::ReasoningSummary::Concise));
    }

    #[test]
    fn reasoning_summary_none_omits_the_field_but_keeps_effort() {
        let client = client_with_summary(Some(xai_grok_sampling_types::ReasoningSummary::None));
        let mut request = built_response_request();
        client.apply_response_defaults(&mut request).unwrap();
        let body = serde_json::to_value(&request.inner).unwrap();
        assert_eq!(
            body.get("reasoning"),
            Some(&serde_json::json!({ "effort": "high" }))
        );
        let reasoning = request
            .inner
            .reasoning
            .expect("reasoning block kept for effort");
        assert_eq!(reasoning.effort, Some(rs::ReasoningEffort::High));
        assert_eq!(reasoning.summary, None);
    }

    #[test]
    fn reasoning_summary_override_replaces_the_built_value() {
        let client = client_with_summary(Some(xai_grok_sampling_types::ReasoningSummary::Detailed));
        let mut request = built_response_request();
        client.apply_response_defaults(&mut request).unwrap();
        assert_eq!(
            request.inner.reasoning.unwrap().summary,
            Some(rs::ReasoningSummary::Detailed)
        );
    }

    #[test]
    fn reasoning_summary_adds_a_reasoning_block_only_when_there_is_something_to_send() {
        let with_summary =
            client_with_summary(Some(xai_grok_sampling_types::ReasoningSummary::Auto));
        let mut request = CreateResponseWrapper::new(rs::CreateResponse::default());
        with_summary.apply_response_defaults(&mut request).unwrap();
        assert_eq!(
            request.inner.reasoning,
            Some(rs::Reasoning {
                effort: None,
                summary: Some(rs::ReasoningSummary::Auto),
            })
        );

        let without = client_with_summary(Some(xai_grok_sampling_types::ReasoningSummary::None));
        let mut request = CreateResponseWrapper::new(rs::CreateResponse::default());
        without.apply_response_defaults(&mut request).unwrap();
        assert_eq!(request.inner.reasoning, None);
    }
}
