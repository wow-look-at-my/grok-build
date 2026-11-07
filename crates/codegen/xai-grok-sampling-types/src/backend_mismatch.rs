//! Errors that say the request went to an API the model is not served on.

use xai_grok_net::StatusCode;

use crate::{ApiBackend, ApiErrorCode, SamplingError};

/// The `error.code` GitHub Copilot sends when a model is not served on the API the request used.
pub const UNSUPPORTED_API_FOR_MODEL_CODE: &str = "unsupported_api_for_model";

const NO_SUCH_ENDPOINT_MARKER: &str = " No such endpoint: ";

/// How each [`ApiBackend`] that the fallback can use is named in provider
/// error text, most specific spelling first.
const API_NAMES: &[(ApiBackend, &[&str])] = &[
    (
        ApiBackend::ChatCompletions,
        &[
            "chat/completions",
            "chat completions api",
            "chat completions endpoint",
        ],
    ),
    (
        ApiBackend::Responses,
        &[
            "v1/responses",
            "/responses endpoint",
            "responses api",
            "responses endpoint",
        ],
    ),
    (
        ApiBackend::Messages,
        &[
            "v1/messages",
            "/messages endpoint",
            "messages api",
            "messages endpoint",
        ],
    ),
];

/// Phrases that refuse the API named right after them.
const REFUSALS: &[&str] = &[
    "only supported",
    "not supported",
    "not accessible",
    "unsupported",
    "does not support",
    "doesn't support",
    "not available",
];

/// Phrases that point at the API named right after them.
const REDIRECTS: &[&str] = &[
    "only supported in",
    "only supported via",
    "only supported on",
    "please use",
    "use the",
];

/// How far before an API name a refusal or a redirect phrase may start.
const PHRASE_WINDOW: usize = 48;

impl SamplingError {
    /// The provider says the model is not served on the API this request used.
    pub fn is_unsupported_backend_error(&self) -> bool {
        let SamplingError::Api {
            status,
            message,
            error_code,
            ..
        } = self
        else {
            return false;
        };
        if !matches!(status.as_u16(), 400 | 404 | 405 | 422) {
            return false;
        }
        if matches!(error_code, Some(ApiErrorCode::Other(code)) if code == UNSUPPORTED_API_FOR_MODEL_CODE)
        {
            return true;
        }
        let text = provider_text(message).to_ascii_lowercase();
        // The completions endpoint is not a backend this client speaks.
        if text.contains("not a chat model") {
            return false;
        }
        api_named_after(&text, REFUSALS).is_some()
    }

    pub fn is_endpoint_missing_error(&self) -> bool {
        let SamplingError::Api {
            status, message, ..
        } = self
        else {
            return false;
        };
        if *status == StatusCode::METHOD_NOT_ALLOWED {
            return true;
        }
        if *status != StatusCode::NOT_FOUND {
            return false;
        }
        let text = provider_text(message).to_ascii_lowercase();
        let text = text.trim().trim_end_matches('.');
        [
            "invalid url",
            "cannot post",
            "unknown endpoint",
            "unknown url",
            "unrecognized request url",
            "no route",
            "route not found",
            "page not found",
            "path not found",
            "request failed (http 404)",
        ]
        .iter()
        .any(|needle| text.contains(needle))
            || text == "not found"
            || text.ends_with("http 404: not found")
    }

    /// The backend that the error text says the model is served on, when it
    /// names one ("only supported in v1/responses", "please use /v1/responses").
    pub fn backend_named_as_supported(&self) -> Option<ApiBackend> {
        let SamplingError::Api { message, .. } = self else {
            return None;
        };
        api_named_after(&provider_text(message).to_ascii_lowercase(), REDIRECTS)
    }
}

/// The provider's own words, without the endpoint URL the client appends.
fn provider_text(message: &str) -> &str {
    message
        .split_once(NO_SUCH_ENDPOINT_MARKER)
        .map_or(message, |(text, _)| text)
}

/// The API named first after the earliest of `phrases` that has one within
/// [`PHRASE_WINDOW`] bytes. The nearest name wins, so "only supported in
/// v1/responses and not in v1/chat/completions" gives Responses.
fn api_named_after(text: &str, phrases: &[&str]) -> Option<ApiBackend> {
    let mut hits: Vec<usize> = phrases
        .iter()
        .flat_map(|phrase| text.match_indices(phrase).map(|(at, p)| at + p.len()))
        .collect();
    hits.sort_unstable();
    hits.into_iter().find_map(|from| {
        let mut to = (from + PHRASE_WINDOW).min(text.len());
        while !text.is_char_boundary(to) {
            to -= 1;
        }
        let window = text.get(from..to)?;
        API_NAMES
            .iter()
            .flat_map(|(backend, names)| {
                names
                    .iter()
                    .filter_map(move |name| window.find(name).map(|at| (at, backend)))
            })
            .min_by_key(|(at, _)| *at)
            .map(|(_, backend)| backend.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16, message: &str) -> SamplingError {
        SamplingError::Api {
            status: StatusCode::from_u16(status).unwrap(),
            message: message.to_owned(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        }
    }

    const OPENAI_RESPONSES_ONLY: &str = "This model is only supported in v1/responses and not in v1/chat/completions. No such endpoint: https://api.openai.com/v1/chat/completions";

    #[test]
    fn openai_responses_only_names_responses() {
        let err = api(404, OPENAI_RESPONSES_ONLY);
        assert!(err.is_unsupported_backend_error());
        assert!(!err.is_endpoint_missing_error());
        assert_eq!(
            err.backend_named_as_supported(),
            Some(ApiBackend::Responses)
        );
    }

    #[test]
    fn copilot_code_classifies_without_the_text() {
        let err = SamplingError::Api {
            status: StatusCode::BAD_REQUEST,
            message: "model gpt-5-codex is not accessible via the /chat/completions endpoint"
                .to_owned(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: Some(ApiErrorCode::parse(UNSUPPORTED_API_FOR_MODEL_CODE)),
        };
        assert!(err.is_unsupported_backend_error());
        assert_eq!(err.backend_named_as_supported(), None);

        let text_only = api(
            400,
            "model gpt-5-codex is not accessible via the /chat/completions endpoint",
        );
        assert!(text_only.is_unsupported_backend_error());
    }

    #[test]
    fn a_redirect_names_the_target_and_not_the_refused_api() {
        let err = api(
            400,
            "Function tools with reasoning_effort are not supported for gpt-5.1 in /v1/chat/completions. Please use /v1/responses instead.",
        );
        assert!(err.is_unsupported_backend_error());
        assert_eq!(
            err.backend_named_as_supported(),
            Some(ApiBackend::Responses)
        );
    }

    #[test]
    fn the_appended_endpoint_url_is_not_read() {
        // The URL names chat/completions; the provider's text names no API.
        let err = api(
            404,
            "The model `gpt-9` does not exist. No such endpoint: https://api.openai.com/v1/chat/completions",
        );
        assert!(!err.is_unsupported_backend_error());
        assert!(!err.is_endpoint_missing_error());
    }

    #[test]
    fn a_legacy_completions_model_is_not_a_backend_mismatch() {
        let err = api(
            404,
            "This is not a chat model and thus not supported in the v1/chat/completions endpoint. Did you mean to use v1/completions?",
        );
        assert!(!err.is_unsupported_backend_error());
    }

    #[test]
    fn router_not_found_is_a_missing_endpoint() {
        for message in [
            "Invalid URL (POST /v1/responses) No such endpoint: https://api.example.com/v1/responses",
            "localhost answered HTTP 404: 404 page not found No such endpoint: http://localhost/v1/responses",
            "localhost answered HTTP 404: Not Found No such endpoint: http://localhost/v1/messages",
            "Request failed (HTTP 404). No such endpoint: http://localhost/v1/messages",
        ] {
            assert!(api(404, message).is_endpoint_missing_error(), "{message}");
        }
        assert!(api(405, "Method Not Allowed").is_endpoint_missing_error());
    }

    #[test]
    fn a_missing_resource_is_not_a_missing_endpoint() {
        let err = api(
            404,
            "Previous response with id 'resp_123' not found. No such endpoint: https://api.openai.com/v1/responses",
        );
        assert!(!err.is_endpoint_missing_error());
        assert!(!err.is_unsupported_backend_error());
    }

    #[test]
    fn other_statuses_never_classify() {
        assert!(!api(500, OPENAI_RESPONSES_ONLY).is_unsupported_backend_error());
        assert!(!api(401, OPENAI_RESPONSES_ONLY).is_unsupported_backend_error());
        assert!(!api(500, "Not Found").is_endpoint_missing_error());
    }
}
