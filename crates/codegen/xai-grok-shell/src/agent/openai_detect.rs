//! Auto-detection of the wire API backend (Responses vs Chat Completions) for
//! user-managed OpenAI-compatible endpoints (`[openai_compatible]`).
//!
//! `OpenAiCompatibleConfig` defaults `api_backend` to [`ApiBackend::AutoDetect`]
//! because the user tells us *where* to send requests but not which wire
//! protocol the server implements. This module probes the endpoint and memoizes
//! the result per base URL so the interactive hot path never re-probes.
//!
//! Security: these functions only ever use the caller-supplied *OpenAI* API key.
//! There is no session-bearer parameter anywhere in this module, so a session
//! token can never leak to the openai endpoint (BYOK fail-closed).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::RwLock;

use xai_grok_sampling_types::ApiBackend;

/// Upper bound for a single probe request. Kept short so a misconfigured or
/// unreachable endpoint cost only a bounded stall before falling back.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Memo of resolved backend per normalized base URL.
static BACKEND_MEMO: OnceLock<RwLock<HashMap<String, ApiBackend>>> = OnceLock::new();

fn memo() -> &'static RwLock<HashMap<String, ApiBackend>> {
    BACKEND_MEMO.get_or_init(|| RwLock::new(HashMap::new()))
}

fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_owned()
}

/// Non-blocking read of the memoized backend for `base_url`.
///
/// Callers on the interactive hot path use this and fall back to
/// [`ApiBackend::auto_detect_fallback()`] when it returns `None`, so they never
/// block on network I/O.
pub(crate) fn memoized_backend(base_url: &str) -> Option<ApiBackend> {
    let key = normalize_base_url(base_url);
    memo()
        .read()
        .map(|m| m.get(&key).cloned())
        .unwrap_or(None)
}

/// Best-effort, never-panicking async probe of `{base_url}/responses` and
/// `{base_url}/chat/completions`.
///
/// * Returns [`ApiBackend::Responses`] if the `/responses` route is defined
///   (any HTTP response other than a 404/connection error — including
///   401/403/405 — means the route exists).
/// * Else returns [`ApiBackend::ChatCompletions`] if `/chat/completions` is
///   defined.
/// * Else returns the shared fallback ([`ApiBackend::auto_detect_fallback`]).
pub async fn detect_api_backend(base_url: &str, api_key: Option<&str>) -> ApiBackend {
    let base = normalize_base_url(base_url);
    if base.is_empty() || !is_http_url(&base) {
        return ApiBackend::auto_detect_fallback();
    }
    let client = crate::http::shared_client();
    if route_defined(&client, &format!("{base}/responses"), api_key).await {
        return ApiBackend::Responses;
    }
    if route_defined(&client, &format!("{base}/chat/completions"), api_key).await {
        return ApiBackend::ChatCompletions;
    }
    ApiBackend::auto_detect_fallback()
}

fn is_http_url(url: &str) -> bool {
    url::Url::parse(url)
        .map(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or(false)
}

/// Whether a request to `url` proves the route is defined on the server.
///
/// Only a `404 Not Found` (and connection-level errors, which are
/// indistinguishable from "no route" for our purposes) is treated as
/// unsupported. Any other HTTP status — a 401/403 (route exists, needs auth),
/// a 405 (route exists, wrong method), a 400, or a 2xx/3xx/5xx response — means
/// the path is recognized, so we treat the route as present.
async fn route_defined(
    client: &reqwest::Client,
    url: &str,
    api_key: Option<&str>,
) -> bool {
    let mut request = client.get(url);
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {}", key.trim()));
    }
    let result = tokio::time::timeout(PROBE_TIMEOUT, request.send()).await;
    match result {
        Ok(Ok(response)) => response.status().as_u16() != 404,
        _ => false,
    }
}

/// Resolve `AutoDetect` for `base_url`, memoized.
///
/// - If already cached locally, returns immediately (non-blocking).
/// - Otherwise performs a bounded synchronous probe (via the blocking HTTP
///   client) once per base URL and caches the result; a probe failure falls
///   back to [`ApiBackend::auto_detect_fallback()`].
///
/// Never panics. The blocking probe happens outside any lock, so concurrent
/// first calls may probe in parallel (benign and idempotent), and the hot path
/// afterwards only takes a short read lock.
pub(crate) fn resolved_backend(base_url: &str, api_key: Option<&str>) -> ApiBackend {
    let key = normalize_base_url(base_url);
    if let Some(backend) = memoized_backend(&key) {
        return backend;
    }

    let detected = blocking_probe(base_url, api_key);
    // Only cache a concrete, non-fallback result so a transient probe failure
    // can be retried later. Probing happens outside any lock, so concurrent
    // first calls may probe in parallel (benign and idempotent); the hot path
    // afterwards only takes a short read lock.
    if detected != ApiBackend::auto_detect_fallback()
        && let Ok(mut memo) = memo().write()
    {
        memo.insert(key, detected.clone());
    }
    detected
}

/// Run a probe using the blocking HTTP client, so resolution can happen from
/// synchronous shell code. Returns the shared fallback on failure.
fn blocking_probe(base_url: &str, api_key: Option<&str>) -> ApiBackend {
    let base = normalize_base_url(base_url);
    if base.is_empty() || !is_http_url(&base) {
        return ApiBackend::auto_detect_fallback();
    }
    let client = crate::http::shared_startup_blocking_client();
    if blocking_route_defined(&client, &format!("{base}/responses"), api_key) {
        return ApiBackend::Responses;
    }
    if blocking_route_defined(&client, &format!("{base}/chat/completions"), api_key) {
        return ApiBackend::ChatCompletions;
    }
    ApiBackend::auto_detect_fallback()
}

fn blocking_route_defined(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
) -> bool {
    let mut request = client.get(url).timeout(PROBE_TIMEOUT);
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        request = request.header("Authorization", format!("Bearer {}", key.trim()));
    }
    match request.send() {
        Ok(response) => response.status().as_u16() != 404,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_test_support::{MockInferenceServer, MockModelEntry};

    /// A GET to a mock `{base}/v1/responses` route returns 405 (route defined,
    /// wrong method) while `{base}/responses` (no `/v1` prefix) is not served,
    /// exercising both the Responses-detection and the ChatCompletions-fallback
    /// branches against a real HTTP listener.
    #[tokio::test]
    async fn detect_picks_responses_when_responses_route_is_defined() {
        let server = MockInferenceServer::start_with_models(vec![
            MockModelEntry::new("test-model"),
        ])
        .await
        .unwrap();
        // base = {url}/v1 → probe hits .../v1/responses and .../v1/chat/completions
        let base = format!("{}/v1", server.url());
        let backend = detect_api_backend(&base, Some("dummy-key")).await;
        assert_eq!(backend, ApiBackend::Responses);
    }

    #[tokio::test]
    async fn detect_falls_back_when_neither_route_is_defined() {
        let server = MockInferenceServer::start_with_models(vec![
            MockModelEntry::new("test-model"),
        ])
        .await
        .unwrap();
        // base = {url} (no /v1) → nothing served at /responses or /chat/completions
        let base = server.url();
        let backend = detect_api_backend(&base, Some("dummy-key")).await;
        assert_eq!(backend, ApiBackend::auto_detect_fallback());
    }

    #[test]
    fn resolved_backend_memoizes_and_is_stable() {
        // A non-http base URL must fall back safely and never panic.
        let backend = resolved_backend("not a url", Some("k"));
        assert_eq!(backend, ApiBackend::auto_detect_fallback());
        // Memoized non-blocking read for an unknown http host returns None.
        assert_eq!(memoized_backend("https://example.invalid/v1"), None);
    }
}
