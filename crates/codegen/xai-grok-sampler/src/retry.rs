//! Retry classification, backoff, and decision-making.
//!
//! Pure logic only: no I/O, no notifications, no logging side-effects.
//! The actor (M4) wraps this with the actual retry loop.
//!
//! # Retry behavior summary
//!
//! **Retried** (up to 14 times — attempt [`DEFAULT_MAX_RETRIES`] = 15 is
//! fatal — ≈5.5 min: every wait, including a server `Retry-After`, is
//! capped at [`MAX_RETRY_BACKOFF`] and jittered):
//! - 429 and any 5xx except 525/526 — covers the Cloudflare edge pages
//!   (520–524 origin unreachable/timed out, 530 edge 1xxx) and upstream
//!   overload (529). The rule is `RetryPolicy::edge_client`.
//! - Connection errors (timeout, refused, reset)
//! - `StreamError` (a server-sent mid-stream error)
//! - `EmptyResponse` (model returned no content/tool calls)
//!
//! **Retried on their own budget** (the transport budget above is untouched):
//! - A stream that died mid-body — `EventStreamError`, or a reqwest decode
//!   failure ("error decoding response body"):
//!   [`STREAM_INTERRUPT_MAX_RETRIES`] = 10, same exponential backoff.
//!
//! **Retried with lower cap** ([`RATE_LIMIT_RETRY_THRESHOLD`] = 5):
//! - 429 (rate limited) — waits the server's `Retry-After`, clamped per
//!   attempt to [`MAX_RETRY_BACKOFF`]; the attempt count bounds the total
//!
//! **Special handling** (not counted against retry budget):
//! - 413 / image processing errors → strip images and retry once
//! - model-takes-no-image-input errors (any of 400/404/415/422/500,
//!   provider-worded) → strip images and retry once
//! - 400 on a `thinking` block's `signature` → drop replayed reasoning and
//!   retry once
//! - 400 "Reasoning is mandatory for this endpoint and cannot be disabled."
//!   → mark the target reasoning-mandatory, remap a disabled/omitted
//!   requested effort to the lowest non-disabled tier, and retry once
//!
//! **Not retried** (Fatal immediately):
//! - 400, 401, 403, 404, 408, 422 (client errors)
//! - Cloudflare 525/526 (origin TLS handshake / invalid cert) — a broken
//!   origin certificate never clears on its own
//! - `Auth` / `InvalidConfiguration` (credential/config issues)
//! - `IdleTimeout` (model stuck, retry would stall again)
//! - `Serialization` (response parsing failure)
//! - `MaxTokensTruncation` (by design)
//!
//! **Server hint** (`x-should-retry` header from CCP):
//! - `false` → Fatal immediately, regardless of status code
//! - `true` / absent → falls through to status-code logic above
//!
//! CCP's header is 429 + any 5xx (`RetryPolicy::server`). Cloudflare's own
//! 52x pages never carry it, so the client policy above is what applies at
//! the edge, and 525/526 stay Fatal even if a future header said retry.

use std::time::Duration;

use xai_grok_sampling_types::{SamplingError, is_retryable_api_status};

/// After this many rate-limit (429) retries, escalate to the caller
/// instead of waiting again. Each wait is capped at [`MAX_RETRY_BACKOFF`],
/// so the budget bounds the total 429 wait at the 120s the header parser
/// admits (`extract_retry_after`) rather than at one attempt.
pub const RATE_LIMIT_RETRY_THRESHOLD: u32 = 5;

pub const RATE_LIMIT_RETRY_DISABLED: u32 = 1;

pub const DEFAULT_MAX_RETRIES: u32 = 15;

/// Retries granted to a stream that died mid-body
/// ([`SamplingError::is_stream_interrupted`]): 10, each after the same
/// exponential backoff the transport path uses. It is a budget of its own,
/// like the doom-loop and output-rate ones: a dropped connection is not a
/// server fault, and spending the transport budget on it leaves nothing for
/// the 5xx that follows. It is also a guarantee — a model configured with a
/// smaller `max_retries` still gets these 10, so one network blip can no
/// longer end a turn with "error decoding response body".
pub const STREAM_INTERRUPT_MAX_RETRIES: u32 = 10;

/// The budget [`classify_error`] must be given for a stream interruption, so
/// that exactly [`STREAM_INTERRUPT_MAX_RETRIES`] retries happen (the attempt
/// reaching the budget is fatal, hence the `+ 1`).
///
/// `transport_budget` of 0 is observe-only, or a caller that cannot take
/// duplicate output; both keep their zero.
pub fn stream_interrupt_budget(transport_budget: u32) -> u32 {
    if transport_budget == 0 {
        0
    } else {
        STREAM_INTERRUPT_MAX_RETRIES + 1
    }
}

/// Longest single wait on the generic retry path — the exponential-backoff
/// ceiling, and the clamp for a server `Retry-After` on every path. Cloudflare
/// answers 52x with `Retry-After: 60`–`120`; honoring that verbatim across 14
/// retries would stall a turn ~28 min instead of the ~5.5 min budget above. A
/// 429 is clamped to the same ceiling and retried up to
/// [`RATE_LIMIT_RETRY_THRESHOLD`] times, so a header asking for longer is
/// still waited out across attempts — one of which may find the limit already
/// clear.
pub const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(30);

pub const TRANSPORT_REBUILD_BACKOFF: Duration = Duration::from_millis(200);

pub(crate) fn resolve_max_retries_with_env(
    env_override: Option<&str>,
    model_max_retries: Option<u32>,
) -> u32 {
    env_override
        .and_then(|value| value.parse::<u32>().ok())
        .or(model_max_retries)
        .unwrap_or(DEFAULT_MAX_RETRIES)
}

pub fn resolve_max_retries(model_max_retries: Option<u32>) -> u32 {
    let env_override = std::env::var("GROK_MAX_RETRIES").ok();
    resolve_max_retries_with_env(env_override.as_deref(), model_max_retries)
}

pub fn doom_loop_backoff(retry_count: u32) -> Duration {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

    let mut hasher = std::hash::DefaultHasher::new();
    JITTER_SEQ.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    retry_count.hash(&mut hasher);
    Duration::from_millis(hasher.finish() % 251)
}

/// Backoff for an output-rate resample. Waiting is the thing the gate exists
/// to stop doing, so it is the same near-immediate jitter a doom-loop resample
/// takes.
pub fn output_rate_backoff(retry_count: u32) -> Duration {
    doom_loop_backoff(retry_count)
}

/// Exponential backoff (2s, 4s, 8s, ..., capped at [`MAX_RETRY_BACKOFF`])
/// with +/-20% jitter to prevent thundering-herd retry storms.
pub fn retry_backoff_with_jitter(retry_count: u32) -> Duration {
    let shift = retry_count.saturating_sub(1);
    let base_ms = 2000u64
        .checked_shl(shift)
        .unwrap_or(u64::MAX)
        .min(MAX_RETRY_BACKOFF.as_millis() as u64);
    jitter_backoff(Duration::from_millis(base_ms))
}

pub fn jitter_backoff(base: Duration) -> Duration {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

    let base_ms = base.as_millis() as u64;
    let jitter_range = base_ms / 5;
    let mut hasher = std::hash::DefaultHasher::new();
    JITTER_SEQ.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let jitter = hasher.finish() % (jitter_range * 2 + 1);
    Duration::from_millis(base_ms - jitter_range + jitter)
}

pub fn retry_after_or_backoff(attempt: u32, retry_after_secs: Option<u64>) -> Duration {
    match retry_after_secs.filter(|secs| *secs > 0) {
        Some(secs) => jitter_backoff(Duration::from_secs(secs).min(MAX_RETRY_BACKOFF)),
        None => retry_backoff_with_jitter(attempt),
    }
}

#[derive(Debug)]
pub enum RetryDecision {
    Retry {
        backoff: Duration,
    },

    RetryWithBackoff {
        backoff: Duration,
        is_rate_limited: bool,
    },

    RetryWithImageStrip,

    /// Retry with the request's thinking replay stepped down one level (a
    /// thinking block whose signature, or `encrypted_content`, this model
    /// cannot take).
    RetryWithReasoningStrip,

    /// Retry after marking the target reasoning-mandatory and remapping a
    /// disabled/omitted requested effort to the lowest non-disabled tier (the
    /// provider answered our disabling body with its "reasoning is mandatory"
    /// 400). The wire builders then emit a supported non-disabled effort.
    RetryWithReasoningEffortRemap,

    /// Retry after dropping the message-level properties a strict-schema
    /// provider rejected (`wrong_api_format ... is unsupported`). Narrowing
    /// the request's [`ChatMessageProfile`] makes the wire conversion omit
    /// them, so the retry body carries only properties the target defines.
    RetryWithMessagePropertyStrip,

    /// Retry after rebuilding the HTTP client with HTTP/1.1 (transport
    /// error, first retry only).
    RetryWithClientRebuild {
        backoff: Duration,
    },

    EmitToSession(SamplingError),

    Fatal(SamplingError),
}

pub fn classify_error(
    err: &SamplingError,
    retry_count: u32,
    max_retries: u32,
    rate_limit_threshold: u32,
) -> RetryDecision {
    if err.is_auth_error() {
        return RetryDecision::EmitToSession(clone_error(err));
    }
    if max_retries == 0 {
        return RetryDecision::Fatal(clone_error(err));
    }

    // A blob another model minted, on either backend. It has to come before
    // the status-code arms below, which would call this 400 fatal: the blocks
    // are in conversation history, so every following turn on the same
    // history fails the same way. The replay level steps down and the
    // request goes again. The session's own flatten-and-resubmit covers what
    // is left once the level is spent.
    if err.is_encrypted_content_error() || err.is_thinking_signature_error() {
        return RetryDecision::RetryWithReasoningStrip;
    }

    // Token overflows fail fast via the retry veto below; byte-coded rejections (413 or a byte-size code) strip images and retry
    if err.is_payload_too_large() || err.is_byte_size_overflow_coded() {
        return RetryDecision::RetryWithImageStrip;
    }

    if err.is_image_processing_error() {
        return RetryDecision::RetryWithImageStrip;
    }

    // The routed model takes no image input (e.g. OpenRouter's 404 "No
    // endpoints found that support image input"). Same strip recovery, and it
    // has to come before the status-code arms below, which would call a 404
    // fatal: the images are in conversation history, so a fatal here fails
    // every following turn on the same history with no way out but a new
    // session.
    if err.is_image_input_unsupported_error() {
        return RetryDecision::RetryWithImageStrip;
    }

    // The provider mandates reasoning and we sent a disabling/omitting body
    // ("Reasoning is mandatory for this endpoint and cannot be disabled.").
    // Remap the requested effort to the lowest non-disabled tier and retry —
    // the same disabling body would fail again, so this has to beat the
    // status-code arms below, which would otherwise call this 400 fatal.
    if err.is_reasoning_mandatory_error() {
        return RetryDecision::RetryWithReasoningEffortRemap;
    }

    // The provider's schema rejected a message-level property it does not
    // define (`wrong_api_format ... is unsupported`, e.g. Cerebras rejecting
    // `model_id`/`reasoning_content` on replayed assistant messages). The
    // offending property lives in conversation *history*, so re-sending the
    // same body fails identically on every turn and every retry — without
    // this arm, a 400 here would be Fatal and permanently brick the session.
    // Recovery is to omit the named properties from the serialized body and
    // retry once.
    if err.is_unsupported_message_property_error() {
        return RetryDecision::RetryWithMessagePropertyStrip;
    }

    // Shared retry vetoes (`SamplingError::is_retry_vetoed`, also used by
    // one-shot callers like /btw):
    // - x-should-retry: false — trust the server, it knows if the error is
    //   request-content-caused (e.g. malformed tool call in history) vs
    //   transient. x-should-retry: true is intentionally NOT handled — the
    //   header only suppresses retries; forcing them on non-retryable
    //   statuses could amplify failures.
    // - Context-window / size overflow — deterministic, re-sending the same
    //   (or larger) payload always fails, whatever status the backend used.
    //
    // Checked AFTER image-strip guards: image stripping changes the
    // request payload, so a server "don't retry" on the original
    // request doesn't apply to the stripped request.
    if err.is_retry_vetoed() {
        return RetryDecision::Fatal(clone_error(err));
    }

    if matches!(err, SamplingError::DoomLoopDetected { .. }) {
        return RetryDecision::Retry {
            backoff: doom_loop_backoff(retry_count + 1),
        };
    }

    // Output-rate collapses and first-token timeouts: same shape as the
    // doom-loop arm above. The rate
    // gate intercepts these before classification and runs its own budget;
    // this arm keeps classification total so one arriving by any other path
    // can never be Fatal.
    if matches!(
        err,
        SamplingError::OutputRateCollapsed { .. } | SamplingError::FirstTokenTimeout { .. }
    ) {
        return RetryDecision::Retry {
            backoff: output_rate_backoff(retry_count + 1),
        };
    }

    // Rate-limited (429): cap retries at the rate-limit threshold to
    // avoid burning long waits. A server `Retry-After` is clamped to
    // [`MAX_RETRY_BACKOFF`] like every other wait — a provider that answers a
    // per-minute bucket with `Retry-After: 60` makes one attempt sit idle for
    // the whole minute, and the limit often clears before the header says.
    // The budget above is what covers the rest of the server's wait.
    if err.is_rate_limited() {
        let next_attempt = retry_count + 1;
        if next_attempt >= max_retries.min(rate_limit_threshold) {
            return RetryDecision::Fatal(clone_error(err));
        }
        let backoff = err
            .retry_after()
            .map(|secs| Duration::from_secs(secs).min(MAX_RETRY_BACKOFF))
            .unwrap_or_else(|| retry_backoff_with_jitter(next_attempt));
        return RetryDecision::RetryWithBackoff {
            backoff,
            is_rate_limited: true,
        };
    }

    if err.is_retryable() {
        let next_attempt = retry_count + 1;
        if next_attempt >= max_retries {
            return RetryDecision::Fatal(clone_error(err));
        }
        if next_attempt == 1 {
            let backoff = match err {
                SamplingError::Http(_) => jitter_backoff(TRANSPORT_REBUILD_BACKOFF),
                _ => retry_after_or_backoff(next_attempt, err.retry_after()),
            };
            return RetryDecision::RetryWithClientRebuild { backoff };
        }
        return RetryDecision::Retry {
            backoff: retry_after_or_backoff(next_attempt, err.retry_after()),
        };
    }

    RetryDecision::Fatal(clone_error(err))
}

pub fn format_sampling_error(err: &SamplingError, retry_count: Option<u32>) -> String {
    let retry_prefix = match retry_count {
        Some(count) => format!("Request failed after {} retries. ", count),
        None => String::new(),
    };

    match err {
        SamplingError::Auth { message, .. } => {
            format!(
                "{}Authentication failed: {}. Please check your API key configuration.",
                retry_prefix, message
            )
        }
        SamplingError::InvalidConfiguration(msg) => {
            format!(
                "{}Invalid configuration: {}. Please check your model settings.",
                retry_prefix, msg
            )
        }
        SamplingError::EndpointNotAllowed(msg) => format!("{retry_prefix}{msg}"),
        SamplingError::MtlsConfiguration(msg) => {
            format!(
                "{}Invalid mTLS configuration: {}. Please check the model endpoint and certificate directory.",
                retry_prefix, msg
            )
        }

        SamplingError::Http(e) => {
            let mut details = Vec::new();
            if e.is_timeout() {
                details.push("timeout".to_string());
            }
            if e.is_connect() {
                details.push("connection failed".to_string());
            }
            if let Some(status) = e.status() {
                details.push(format!("status {}", status));
            }
            if let Some(url) = e.url() {
                details.push(format!("url: {}", url));
            }
            let detail_str = if details.is_empty() {
                e.to_string()
            } else {
                format!("{} ({})", e, details.join(", "))
            };
            format!(
                "{}HTTP request failed: {}. This may be a network issue or the API endpoint may be unavailable.",
                retry_prefix, detail_str
            )
        }
        SamplingError::Serialization(e) => {
            format!(
                "{}Failed to parse API response at line {} column {}: {}. This indicates an unexpected response format from the server.",
                retry_prefix,
                e.line(),
                e.column(),
                e
            )
        }
        SamplingError::Api {
            status, message, ..
        } => {
            let status_hint = match status.as_u16() {
                400 => " (bad request - check your input)",
                401 | 403 => " (authentication issue - check your API key)",
                404 => " (endpoint not found - check model configuration)",
                413 => " (request too large - try /compact or start new session)",
                429 => " (rate limited - please wait and retry)",
                500 => " (server internal error)",
                _ if is_retryable_api_status(*status) => " (server unavailable - please retry)",
                _ => "",
            };
            format!(
                "{}API error (HTTP {}{}): {}",
                retry_prefix,
                status.as_u16(),
                status_hint,
                message
            )
        }
        SamplingError::EventStreamError(msg) => {
            format!(
                "{}Event stream error: {}. The connection to the server was interrupted.",
                retry_prefix, msg
            )
        }
        SamplingError::StreamError {
            error_type,
            message,
            ..
        } => {
            format!(
                "{}Server stream error ({}): {}. The server encountered an error while streaming the response.",
                retry_prefix, error_type, message
            )
        }
        SamplingError::IdleTimeout { elapsed_secs } => {
            format!(
                "{}Model stopped responding after {}s. The model may be overloaded or stuck. Try again or use a different model.",
                retry_prefix, elapsed_secs
            )
        }
        SamplingError::EmptyResponse { context } => {
            format!(
                "{}Empty response from model ({}): model={}, had_reasoning={}, finish_reason={}, completion_tokens={}",
                retry_prefix,
                context.reason,
                context.model,
                context.had_reasoning,
                context.finish_reason_str(),
                context.completion_tokens.unwrap_or(0),
            )
        }
        SamplingError::MaxTokensTruncation => {
            format!("{}Response truncated by max_tokens.", retry_prefix)
        }
        SamplingError::DoomLoopDetected { triggers, .. } => {
            format!(
                "{}Server detected a reasoning loop ({}); resampling the response.",
                retry_prefix,
                triggers.join(", ")
            )
        }
        SamplingError::OutputRateCollapsed {
            observed_tokens_per_sec,
            floor_tokens_per_sec,
            window_secs,
        } => {
            format!(
                "{}Output rate collapsed to {:.1} tok/s over {}s (floor {:.1}); reissuing the request.",
                retry_prefix, observed_tokens_per_sec, window_secs, floor_tokens_per_sec,
            )
        }
        SamplingError::FirstTokenTimeout {
            waited_secs,
            limit_secs,
        } => {
            format!(
                "{}No output after {}s (time-to-first-token limit {}s); reissuing the request.",
                retry_prefix, waited_secs, limit_secs,
            )
        }
    }
}

pub(crate) fn clone_error(err: &SamplingError) -> SamplingError {
    match err {
        SamplingError::Auth {
            message,
            credential,
        } => SamplingError::Auth {
            message: message.clone(),
            credential: *credential,
        },
        SamplingError::InvalidConfiguration(msg) => SamplingError::InvalidConfiguration(msg),
        SamplingError::EndpointNotAllowed(msg) => SamplingError::EndpointNotAllowed(msg.clone()),
        SamplingError::MtlsConfiguration(msg) => SamplingError::MtlsConfiguration(msg.clone()),
        SamplingError::Http(e) => {
            // reqwest::Error is not Clone; preserve the rendered message
            // as an EventStreamError (the closest retryable transport
            // variant) so callers see an equivalent description.
            SamplingError::EventStreamError(xai_grok_sampling_types::error::error_chain(e))
        }
        SamplingError::Serialization(e) => {
            // serde_json::Error is not Clone; its Display already carries the
            // original line/column exactly once.
            SamplingError::serialization_message(e)
        }
        SamplingError::Api {
            status,
            message,
            model_metadata,
            retry_after_secs,
            should_retry,
            error_code,
        } => SamplingError::Api {
            status: *status,
            message: message.clone(),
            model_metadata: model_metadata.clone(),
            retry_after_secs: *retry_after_secs,
            should_retry: *should_retry,
            error_code: error_code.clone(),
        },
        SamplingError::EventStreamError(msg) => SamplingError::EventStreamError(msg.clone()),
        SamplingError::StreamError {
            error_type,
            message,
            code,
        } => SamplingError::StreamError {
            error_type: error_type.clone(),
            message: message.clone(),
            code: code.clone(),
        },
        SamplingError::IdleTimeout { elapsed_secs } => SamplingError::IdleTimeout {
            elapsed_secs: *elapsed_secs,
        },
        SamplingError::EmptyResponse { context } => SamplingError::EmptyResponse {
            context: context.clone(),
        },
        SamplingError::MaxTokensTruncation => SamplingError::MaxTokensTruncation,
        SamplingError::DoomLoopDetected {
            triggers,
            aborted_at_chunk,
        } => SamplingError::DoomLoopDetected {
            triggers: triggers.clone(),
            aborted_at_chunk: *aborted_at_chunk,
        },
        SamplingError::OutputRateCollapsed {
            observed_tokens_per_sec,
            floor_tokens_per_sec,
            window_secs,
        } => SamplingError::OutputRateCollapsed {
            observed_tokens_per_sec: *observed_tokens_per_sec,
            floor_tokens_per_sec: *floor_tokens_per_sec,
            window_secs: *window_secs,
        },
        SamplingError::FirstTokenTimeout {
            waited_secs,
            limit_secs,
        } => SamplingError::FirstTokenTimeout {
            waited_secs: *waited_secs,
            limit_secs: *limit_secs,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;
    use xai_grok_sampling_types::ApiErrorCode;

    fn api_err(status: StatusCode, message: &str) -> SamplingError {
        SamplingError::Api {
            status,
            message: message.to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        }
    }

    fn api_err_with_retry_after(status: StatusCode, retry_after: u64) -> SamplingError {
        SamplingError::Api {
            status,
            message: "x".to_string(),
            model_metadata: None,
            retry_after_secs: Some(retry_after),
            should_retry: None,
            error_code: None,
        }
    }

    #[test]
    fn resolve_max_retries_env_override_takes_precedence() {
        assert_eq!(resolve_max_retries_with_env(Some("9"), Some(3)), 9);
    }

    #[test]
    fn resolve_max_retries_falls_back_to_model() {
        assert_eq!(resolve_max_retries_with_env(None, Some(7)), 7);
    }

    #[test]
    fn resolve_max_retries_default() {
        assert_eq!(
            resolve_max_retries_with_env(None, None),
            DEFAULT_MAX_RETRIES
        );
    }

    #[test]
    fn resolve_max_retries_invalid_env_falls_through() {
        assert_eq!(resolve_max_retries_with_env(Some("abc"), Some(4)), 4);
    }

    #[test]
    fn backoff_first_retry_is_around_two_seconds() {
        let backoff = retry_backoff_with_jitter(1);
        assert!(
            backoff >= Duration::from_millis(1600) && backoff <= Duration::from_millis(2400),
            "first retry backoff out of range: {:?}",
            backoff
        );
    }

    #[test]
    fn backoff_doubles_then_caps_at_thirty_seconds() {
        let r2 = retry_backoff_with_jitter(2);
        assert!(r2 >= Duration::from_millis(3200) && r2 <= Duration::from_millis(4800));

        let r10 = retry_backoff_with_jitter(10);
        assert!(r10 >= Duration::from_millis(24_000) && r10 <= Duration::from_millis(36_000));
    }

    #[test]
    fn backoff_zero_retry_count_is_well_defined() {
        let backoff = retry_backoff_with_jitter(0);
        assert!(backoff >= Duration::from_millis(1600) && backoff <= Duration::from_millis(2400));
    }

    #[test]
    fn classify_auth_error_emits_to_session() {
        let err = SamplingError::auth_unknown("bad token");
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::EmitToSession(SamplingError::Auth { .. }) => {}
            other => panic!("expected EmitToSession(Auth), got {other:?}"),
        }
    }

    #[test]
    fn classify_unauthorized_emits_to_session() {
        let err = api_err(StatusCode::UNAUTHORIZED, "no");
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::EmitToSession(SamplingError::Api { status, .. }) => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
            other => panic!("expected EmitToSession(Api 401), got {other:?}"),
        }
    }

    #[test]
    fn classify_encrypted_content_steps_the_replay_level_down() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "Could not decrypt the provided encrypted_content",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithReasoningStrip
        ));
    }

    /// A rejected thinking signature is a 400, which every other rule calls
    /// fatal. It has to strip instead: the blocks sit in conversation history,
    /// so a fatal here fails every later turn on the same history too.
    #[test]
    fn classify_thinking_signature_strips_reasoning() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "messages.1.content.0: Invalid `signature` in `thinking` block",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithReasoningStrip
        ));
    }

    /// OpenRouter's "reasoning is mandatory" is a 400, which every other rule
    /// calls fatal. It has to remap instead: re-sending the same disabling
    /// body always fails, and nothing about the target is fixed by stripping
    /// history — the effort has to come back enabled.
    #[test]
    fn classify_reasoning_mandatory_400_remaps_effort() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "Reasoning is mandatory for this endpoint and cannot be disabled.",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithReasoningEffortRemap
        ));
    }

    /// The server's "don't retry" hint is about the request it saw; remapping
    /// to a non-disabled effort makes a different request, so the remap must
    /// outrank the veto.
    #[test]
    fn classify_reasoning_mandatory_outranks_should_retry_veto() {
        let err = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "Reasoning is mandatory for this endpoint and cannot be disabled.".to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithReasoningEffortRemap
        ));
    }

    #[test]
    fn classify_reasoning_mandatory_requires_a_400() {
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Reasoning is mandatory for this endpoint and cannot be disabled.",
        );
        assert!(
            !matches!(
                classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
                RetryDecision::RetryWithReasoningEffortRemap
            ),
            "a 5xx with the same text must not take the reasoning-mandatory remap arm"
        );
    }

    /// The server's "don't retry" hint is about the request it saw; dropping
    /// the offending properties makes a different request, so the strip must
    /// outrank the veto — otherwise a strict-schema provider's 400 with
    /// `x-should-retry: false` would dead-end a recoverable session.
    #[test]
    fn classify_unsupported_message_property_outranks_should_retry_veto() {
        let err = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "wrong_api_format: messages.6.assistant.model_id: property \
                 'messages.6.assistant.model_id' is unsupported"
                .to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithMessagePropertyStrip
        ));
    }

    /// The Cerebras shape must take the strip arm rather than the generic
    /// fatal path — this is the arm that un-bricks a session whose stored
    /// history predates the fix.
    #[test]
    fn classify_unsupported_message_property_400_strips() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "wrong_api_format: messages.6.assistant.model_id: property \
             'messages.6.assistant.model_id' is unsupported\n\
             messages.6.assistant.reasoning_content: property \
             'messages.6.assistant.reasoning_content' is unsupported",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithMessagePropertyStrip
        ));
    }

    /// A bare 400 that names nothing unsupported must stay fatal: stripping
    /// fields cannot fix an unrelated malformed request, and silently
    /// rewriting the body would hide the real bug.
    #[test]
    fn classify_plain_400_stays_fatal() {
        let err = api_err(StatusCode::BAD_REQUEST, "malformed tool call in history");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    /// Like the other content-recovery arms, this one must not fire on a 5xx
    /// whose text happens to match.
    #[test]
    fn classify_unsupported_message_property_requires_a_400() {
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "wrong_api_format: messages.0.assistant.model_id: property \
             'messages.0.assistant.model_id' is unsupported",
        );
        assert!(
            !matches!(
                classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
                RetryDecision::RetryWithMessagePropertyStrip
            ),
            "a 5xx with the same text must not take the strip arm"
        );
    }

    /// The server's "don't retry" hint is about the request it saw; dropping
    /// the reasoning makes a different request, so the strip must outrank it.
    #[test]
    fn classify_thinking_signature_outranks_should_retry_veto() {
        let err = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "Invalid `signature` in `thinking` block".to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithReasoningStrip
        ));
    }

    #[test]
    fn classify_payload_too_large_strips_images() {
        let err = api_err(StatusCode::PAYLOAD_TOO_LARGE, "too big");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    /// A vision-less model is a 404, which every other rule calls fatal. It has
    /// to strip instead: the images sit in conversation history, so a fatal
    /// here fails every later turn on the same history too.
    #[test]
    fn classify_image_input_unsupported_404_strips_images() {
        let err = api_err(
            StatusCode::NOT_FOUND,
            "No endpoints found that support image input",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    /// The server's "don't retry" hint is about the request it saw; stripping
    /// images makes a different request, so the strip must outrank the veto.
    #[test]
    fn classify_image_input_unsupported_outranks_should_retry_veto() {
        let err = SamplingError::Api {
            status: StatusCode::NOT_FOUND,
            message: "No endpoints found that support image input".to_string(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_unrelated_404_stays_fatal() {
        let err = api_err(StatusCode::NOT_FOUND, "The model `gpt-9` does not exist");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn classify_image_processing_error_400_strips_images() {
        let err = api_err(StatusCode::BAD_REQUEST, "Could not process image");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_many_image_dimension_400_strips_images() {
        let err = api_err(
            StatusCode::BAD_REQUEST,
            "invalid_request_error: messages.0.content.4.image.source.base64.data: \
             At least one of the image dimensions exceed max allowed size for \
             many-image requests: 2000 pixels",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_processing_error_500_wrapped_strips_images() {
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream: 400 Bad Request: Could not process image",
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_processing_error_takes_priority_over_5xx_retry() {
        let err = api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not process image: bad format",
        );
        assert!(
            err.is_retryable(),
            "500 is retryable without the image-processing guard"
        );
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_400_strips_even_with_should_retry_false() {
        let err = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "some future wording without the legacy phrase".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: Some(ApiErrorCode::InvalidImage),
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_image_stream_error_strips_instead_of_blind_retry() {
        let err = SamplingError::StreamError {
            error_type: "invalid_request_error".into(),
            message: "Base64 string of provided image cannot be decoded.".into(),
            code: Some(ApiErrorCode::InvalidImage),
        };
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));

        let unrelated = SamplingError::StreamError {
            error_type: "overloaded_error".into(),
            message: "The server is overloaded.".into(),
            code: None,
        };
        assert!(!matches!(
            classify_error(&unrelated, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithImageStrip
        ));
    }

    #[test]
    fn classify_rate_limited_uses_retry_after() {
        let err = api_err_with_retry_after(StatusCode::TOO_MANY_REQUESTS, 7);
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithBackoff {
                backoff,
                is_rate_limited,
            } => {
                assert!(is_rate_limited);
                assert_eq!(backoff, Duration::from_secs(7));
            }
            other => panic!("expected RetryWithBackoff, got {other:?}"),
        }
    }

    #[test]
    fn tpm_429_with_retry_after_backs_off_despite_size_text() {
        // A TPM 429 often carries size wording ("Request too large for model...") plus a Retry-After promising capacity later
        // The size veto must not fast-fail it
        let err = SamplingError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Request too large for model: Limit 30000, Requested 50000 tokens per min"
                .to_string(),
            model_metadata: None,
            retry_after_secs: Some(7),
            should_retry: None,
            error_code: None,
        };
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithBackoff {
                is_rate_limited, ..
            } => assert!(is_rate_limited),
            other => panic!("expected RetryWithBackoff, got {other:?}"),
        }
        // Without Retry-After the same message fast-fails: the request exceeds the per-minute cap outright and retrying is futile
        let no_retry_after = api_err(
            StatusCode::TOO_MANY_REQUESTS,
            "Request too large for model: Limit 30000, Requested 50000 tokens per min",
        );
        assert!(matches!(
            classify_error(&no_retry_after, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn rate_limit_retry_layer_splits_by_threshold() {
        let err = api_err_with_retry_after(StatusCode::TOO_MANY_REQUESTS, 5);
        assert!(
            matches!(
                classify_error(&err, 0, 15, RATE_LIMIT_RETRY_DISABLED),
                RetryDecision::Fatal(_)
            ),
            "disabled threshold must surface the first 429, not wait internally"
        );
        assert!(
            matches!(
                classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
                RetryDecision::RetryWithBackoff {
                    is_rate_limited: true,
                    ..
                }
            ),
            "default threshold keeps the sampler's own 429 retry"
        );
    }

    #[test]
    fn classify_rate_limited_capped_at_threshold() {
        let err = api_err(StatusCode::TOO_MANY_REQUESTS, "slow");
        // retry_count=1, threshold=2 -> next_attempt=2 >= 2 -> Fatal.
        match classify_error(&err, 1, 5, 2) {
            RetryDecision::Fatal(SamplingError::Api { status, .. }) => {
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            }
            other => panic!("expected Fatal at threshold, got {other:?}"),
        }
    }

    /// A per-minute bucket answers `Retry-After: 60`. One attempt must not sit
    /// idle for the whole minute, and the budget must still cover the wait the
    /// server asked for, so the turn is never failed earlier than before.
    #[test]
    fn a_long_rate_limit_wait_is_split_across_attempts() {
        let err = api_err_with_retry_after(StatusCode::TOO_MANY_REQUESTS, 60);
        let mut waited = Duration::ZERO;
        let mut retries = 0u32;
        loop {
            match classify_error(
                &err,
                retries,
                DEFAULT_MAX_RETRIES,
                RATE_LIMIT_RETRY_THRESHOLD,
            ) {
                RetryDecision::RetryWithBackoff { backoff, .. } => {
                    assert!(
                        backoff <= MAX_RETRY_BACKOFF,
                        "one wait must stay under the ceiling: {backoff:?}"
                    );
                    waited += backoff;
                    retries += 1;
                }
                RetryDecision::Fatal(_) => break,
                other => panic!("expected RetryWithBackoff, got {other:?}"),
            }
        }
        assert!(
            waited >= Duration::from_secs(60),
            "the budget must still cover the server's own wait, got {waited:?}"
        );
    }

    #[test]
    fn zero_retry_budget_never_reuses_a_model_output_cap() {
        for err in [
            api_err(StatusCode::INTERNAL_SERVER_ERROR, "boom"),
            api_err(StatusCode::PAYLOAD_TOO_LARGE, "too big"),
            api_err(StatusCode::BAD_REQUEST, "Could not process image"),
            SamplingError::EmptyResponse {
                context: xai_grok_sampling_types::EmptyResponseContext {
                    reason: xai_grok_sampling_types::EmptyReason::NoVisibleContent,
                    had_reasoning: false,
                    content_len: 0,
                    tool_call_count: 0,
                    finish_reason: Some("stop".into()),
                    completion_tokens: Some(1),
                    reasoning_tokens: Some(0),
                    prompt_tokens: Some(10),
                    model: "m".into(),
                    first_choice_seen: true,
                },
            },
        ] {
            assert!(matches!(
                classify_error(&err, 0, 0, RATE_LIMIT_RETRY_THRESHOLD),
                RetryDecision::Fatal(_)
            ));
        }
    }

    #[test]
    fn classify_5xx_first_retry_rebuilds_client() {
        let err = api_err(StatusCode::INTERNAL_SERVER_ERROR, "boom");
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { backoff } => {
                assert!(backoff >= Duration::from_millis(1600));
            }
            other => panic!("expected RetryWithClientRebuild, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_failure_first_retry_skips_the_server_backoff() {
        const CONNECT_GUARD: Duration = Duration::from_secs(5);

        let send_err = tokio::time::timeout(CONNECT_GUARD, reqwest::get("http://127.0.0.1:0"))
            .await
            .expect("port 0 connect fails well within the guard")
            .expect_err("connecting to port 0 must fail");
        let err = SamplingError::Http(send_err);

        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { backoff } => assert!(
                backoff >= Duration::from_millis(160) && backoff <= Duration::from_millis(240),
                "transport rebuild must not wait the 2s server backoff: {backoff:?}"
            ),
            other => panic!("expected RetryWithClientRebuild, got {other:?}"),
        }

        match classify_error(&err, 1, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Retry { backoff } => {
                assert!(backoff >= Duration::from_millis(3200), "{backoff:?}");
            }
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn classify_cloudflare_522_is_retryable() {
        let err = api_err(
            StatusCode::from_u16(522).unwrap(),
            "Connection to Grok timed out or was interrupted. (HTTP 522).",
        );
        match classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { .. } => {}
            other => panic!("expected RetryWithClientRebuild for 522, got {other:?}"),
        }
        match classify_error(&err, 1, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Retry { .. } => {}
            other => panic!("expected Retry for 522 attempt 2, got {other:?}"),
        }
    }

    #[test]
    fn classify_cloudflare_525_is_fatal_even_with_should_retry_true() {
        for should_retry in [None, Some(true)] {
            let err = SamplingError::Api {
                status: StatusCode::from_u16(525).unwrap(),
                message: "Secure connection to Grok failed. (HTTP 525).".into(),
                model_metadata: None,
                retry_after_secs: None,
                should_retry,
                error_code: None,
            };
            match classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD) {
                RetryDecision::Fatal(SamplingError::Api { status, .. }) => {
                    assert_eq!(status.as_u16(), 525);
                }
                other => panic!("expected Fatal for 525 ({should_retry:?}), got {other:?}"),
            }
        }
    }

    #[test]
    fn classify_clamps_retry_after_on_every_path_and_jitters_the_generic_one() {
        // Cloudflare answers 52x with Retry-After: 60-120. Honoring that
        // verbatim across 14 retries would stall the turn ~28 min, and an
        // unjittered wait would re-hit the recovering origin in lockstep.
        let edge = api_err_with_retry_after(StatusCode::from_u16(522).unwrap(), 120);
        match classify_error(&edge, 1, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Retry { backoff } => {
                assert!(backoff >= Duration::from_secs(24), "got {backoff:?}");
                assert!(backoff <= Duration::from_secs(36), "got {backoff:?}");
            }
            other => panic!("expected Retry for 522, got {other:?}"),
        }

        // The 429 path takes the same clamp, unjittered: the server named a
        // deadline, so a wait under it is the one thing that cannot help.
        let rate_limited = api_err_with_retry_after(StatusCode::TOO_MANY_REQUESTS, 120);
        match classify_error(&rate_limited, 0, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithBackoff { backoff, .. } => {
                assert_eq!(backoff, MAX_RETRY_BACKOFF);
            }
            other => panic!("expected RetryWithBackoff for 429, got {other:?}"),
        }
    }

    #[test]
    fn classify_5xx_subsequent_retry_uses_plain_retry() {
        let err = api_err(StatusCode::BAD_GATEWAY, "boom");
        match classify_error(&err, 1, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Retry { backoff } => {
                assert!(backoff >= Duration::from_millis(3200));
            }
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn classify_5xx_exhausted_retries_is_fatal() {
        let err = api_err(StatusCode::SERVICE_UNAVAILABLE, "boom");
        match classify_error(&err, 4, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::Api { .. }) => {}
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn classify_event_stream_error_is_retryable() {
        let err = SamplingError::EventStreamError("connection reset".into());
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { .. } => {}
            other => panic!("expected RetryWithClientRebuild, got {other:?}"),
        }
    }

    /// The guarantee the budget exists for: a stream that dies mid-body is
    /// retried exactly 10 times, on backoff that grows, whatever the model's
    /// own `max_retries` says.
    #[test]
    fn a_stream_interruption_is_retried_ten_times_with_growing_backoff() {
        let err = SamplingError::EventStreamError("error decoding response body".into());
        let budget = stream_interrupt_budget(3);
        let mut previous = Duration::ZERO;
        for retries_done in 0..STREAM_INTERRUPT_MAX_RETRIES {
            let backoff =
                match classify_error(&err, retries_done, budget, RATE_LIMIT_RETRY_THRESHOLD) {
                    // The first retry also escapes a poisoned HTTP/2 pool.
                    RetryDecision::RetryWithClientRebuild { backoff } => {
                        assert_eq!(retries_done, 0);
                        backoff
                    }
                    RetryDecision::Retry { backoff } => backoff,
                    other => panic!("retry {retries_done} must happen, got {other:?}"),
                };
            // Only while the base is still under the ceiling: once every wait
            // is a jittered 30s, one can land below the last.
            if retries_done < 4 {
                assert!(
                    backoff > previous,
                    "backoff must grow: {backoff:?} after {previous:?}"
                );
            }
            previous = backoff;
        }
        assert!(
            previous >= Duration::from_secs(24),
            "the tail must reach the {MAX_RETRY_BACKOFF:?} ceiling, got {previous:?}"
        );
        match classify_error(
            &err,
            STREAM_INTERRUPT_MAX_RETRIES,
            budget,
            RATE_LIMIT_RETRY_THRESHOLD,
        ) {
            RetryDecision::Fatal(_) => {}
            other => panic!("the 11th attempt must be fatal, got {other:?}"),
        }
    }

    #[test]
    fn observe_only_grants_a_stream_interruption_nothing() {
        assert_eq!(stream_interrupt_budget(0), 0);
        let err = SamplingError::EventStreamError("conn reset".into());
        match classify_error(
            &err,
            0,
            stream_interrupt_budget(0),
            RATE_LIMIT_RETRY_THRESHOLD,
        ) {
            RetryDecision::Fatal(_) => {}
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn classify_stream_error_is_retryable() {
        let err = SamplingError::StreamError {
            error_type: "transient".into(),
            message: "x".into(),
            code: None,
        };
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::RetryWithClientRebuild { .. } => {}
            other => panic!("expected RetryWithClientRebuild for StreamError, got {other:?}"),
        }
    }

    #[test]
    fn classify_idle_timeout_is_fatal() {
        let err = SamplingError::IdleTimeout { elapsed_secs: 300 };
        match classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::IdleTimeout { elapsed_secs: 300 }) => {}
            other => panic!("expected Fatal(IdleTimeout), got {other:?}"),
        }
    }

    #[test]
    fn classify_invalid_config_is_fatal() {
        let err = SamplingError::InvalidConfiguration("missing model");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(SamplingError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn classify_api_400_non_encrypted_is_fatal() {
        let err = api_err(StatusCode::BAD_REQUEST, "Invalid model parameter");
        assert!(matches!(
            classify_error(&err, 0, 5, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    fn serialization_err() -> SamplingError {
        SamplingError::Serialization(serde_json::from_str::<i32>("not a number").unwrap_err())
    }

    #[test]
    fn clone_error_preserves_serialization_and_non_retryability() {
        let cloned = clone_error(&serialization_err());
        assert!(
            matches!(cloned, SamplingError::Serialization(_)),
            "expected Serialization, got {cloned:?}"
        );
        assert!(!cloned.is_retryable());
        assert!(
            cloned.to_string().contains("line 1 column"),
            "original position text must survive the clone: {cloned}"
        );
    }

    #[test]
    fn clone_error_preserves_stream_error_code() {
        let cloned = clone_error(&SamplingError::StreamError {
            error_type: "invalid_request_error".into(),
            message: "bad image".into(),
            code: Some(ApiErrorCode::InvalidImage),
        });
        let SamplingError::StreamError { code, .. } = &cloned else {
            panic!("expected StreamError, got {cloned:?}");
        };
        assert_eq!(*code, Some(ApiErrorCode::InvalidImage));
        assert!(cloned.is_image_processing_error());
    }

    #[test]
    fn classify_serialization_is_fatal_on_first_attempt() {
        match classify_error(&serialization_err(), 0, 15, RATE_LIMIT_RETRY_THRESHOLD) {
            RetryDecision::Fatal(SamplingError::Serialization(_)) => {}
            other => panic!("expected Fatal(Serialization) on attempt 1, got {other:?}"),
        }
    }

    #[test]
    fn format_includes_retry_prefix_when_count_present() {
        let err = SamplingError::auth_unknown("bad");
        let s = format_sampling_error(&err, Some(3));
        assert!(s.starts_with("Request failed after 3 retries."));
    }

    #[test]
    fn format_omits_retry_prefix_when_count_absent() {
        let err = SamplingError::auth_unknown("bad");
        let s = format_sampling_error(&err, None);
        assert!(!s.starts_with("Request failed after"));
        assert!(s.starts_with("Authentication failed:"));
    }

    #[test]
    fn format_includes_status_hint_for_known_codes() {
        let err = api_err(StatusCode::PAYLOAD_TOO_LARGE, "big");
        let s = format_sampling_error(&err, None);
        assert!(s.contains("HTTP 413"));
        assert!(s.contains("request too large"));
    }

    #[test]
    fn format_idle_timeout_includes_elapsed_secs() {
        let err = SamplingError::IdleTimeout { elapsed_secs: 240 };
        let s = format_sampling_error(&err, None);
        assert!(s.contains("240s"));
    }

    #[test]
    fn should_retry_false_overrides_retryable_status() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn context_length_overflow_is_fatal_even_as_500() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "none: The prompt is too long for this model's context window.".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn drifted_size_overflow_wordings_are_fatal_on_turn_path() {
        // Size-worded errors with no code must fail fast, not burn the retry budget
        let api_500 = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "exceed_context_size_error: request (300000 tokens) exceeds the model \
                      context size"
                .into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        };
        assert!(matches!(
            classify_error(&api_500, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));

        let stream = SamplingError::StreamError {
            error_type: "BAD_REQUEST".into(),
            message: "Input length (300000 tokens) exceeds the maximum allowed length \
                      (200000 tokens)"
                .into(),
            code: None,
        };
        assert!(matches!(
            classify_error(&stream, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));

        // Token-tier code with an opaque message: fatal, no strip.
        let coded = SamplingError::StreamError {
            error_type: "BAD_REQUEST".into(),
            message: "request rejected".into(),
            code: Some(xai_grok_sampling_types::ApiErrorCode::parse(
                "exceed_context_size_error",
            )),
        };
        assert!(matches!(
            classify_error(&coded, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }

    #[test]
    fn byte_size_coded_errors_get_image_strip_before_the_veto() {
        // Byte-size codes get the 413 remedy: strip images and retry once; the caller upgrades to Fatal when nothing is left to strip
        for code in ["413", "payload_too_large", "request_too_large"] {
            let coded = SamplingError::StreamError {
                error_type: "BAD_REQUEST".into(),
                message: "request rejected".into(),
                code: Some(xai_grok_sampling_types::ApiErrorCode::parse(code)),
            };
            assert!(
                matches!(
                    classify_error(&coded, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
                    RetryDecision::RetryWithImageStrip
                ),
                "expected image strip for coded {code}"
            );
        }
    }

    #[test]
    fn should_retry_true_falls_through_to_existing_logic() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: Some(true),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithClientRebuild { .. }
        ));
    }

    #[test]
    fn should_retry_absent_falls_through() {
        let err = SamplingError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "boom".into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::RetryWithClientRebuild { .. }
        ));
    }

    #[test]
    fn classify_doom_loop_detected_is_retry_with_immediate_backoff() {
        let err = SamplingError::DoomLoopDetected {
            triggers: vec!["tail_repetition:8@thinking".into()],
            aborted_at_chunk: None,
        };
        for retry_count in [0, 5, 99] {
            match classify_error(&err, retry_count, 2, RATE_LIMIT_RETRY_THRESHOLD) {
                RetryDecision::Retry { backoff } => {
                    assert!(backoff <= Duration::from_millis(250), "near-immediate");
                }
                other => panic!("expected Retry, got {other:?}"),
            }
        }
    }

    #[test]
    fn should_retry_false_on_429_is_fatal() {
        let err = SamplingError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "rate limited".into(),
            model_metadata: None,
            retry_after_secs: Some(10),
            should_retry: Some(false),
            error_code: None,
        };
        assert!(matches!(
            classify_error(&err, 0, 15, RATE_LIMIT_RETRY_THRESHOLD),
            RetryDecision::Fatal(_)
        ));
    }
}
