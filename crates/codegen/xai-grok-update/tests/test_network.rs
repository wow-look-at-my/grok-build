//! Network-level integration tests using `wiremock`.
//!
//! Covers `fetch_gcs_version_from_base`, which takes its URL as a parameter and
//! so is still live with auto-update disabled. The download paths these tests
//! used to share a file with refuse outright now -- see test_disabled_pins.rs.
//! We don't need `serial_test` here because each `MockServer` binds to its own
//! random port and tests don't touch global state.
//!
//! NOTE on retry timing: the prod retry backoff is 1s + 2s + 4s = 7s
//! wall-clock. We can't use `tokio::time::pause()` because reqwest's I/O
//! reactor uses the same tokio timer and stalls when time is paused. So
//! retry-exhaustion tests are intrinsically slow (~7s each); we keep the
//! count small and let them run in parallel (wiremock binds random ports
//! so there's no contention).

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use xai_grok_update::version::fetch_gcs_version_from_base;

// ─────────────────────────────────────────────────────────────────────────────
// Happy-path tests (fast, no retries triggered).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gcs_pointer_returns_version_on_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181\n"))
        .expect(1)
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_trims_whitespace() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("  0.1.181  \r\n  "))
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_rejects_invalid_semver_no_retry() {
    // Invalid semver in the channel pointer is a hard error — must NOT
    // retry (it's a server data bug, not a transient failure).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-a-version"))
        .expect(1) // exactly one request — no retry on parse failure
        .mount(&server)
        .await;

    let err = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("invalid semver"), "msg: {msg}");
}

#[tokio::test]
async fn gcs_pointer_alpha_channel_returns_max_of_alpha_and_stable_when_stable_higher() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.180-alpha.5"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .expect(1)
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("alpha", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_alpha_returns_alpha_when_higher() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.182-alpha.1"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("alpha", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.182-alpha.1");
}

#[tokio::test]
async fn gcs_pointer_stable_channel_does_not_fetch_alpha() {
    // Stable-channel users should not pay the cost of fetching the alpha
    // pointer. The mock for /alpha should never be hit.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/alpha"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_with_long_pre_release_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.190-alpha.42"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.189"))
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("alpha", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.190-alpha.42");
}

#[tokio::test]
async fn gcs_pointer_preserves_path_in_base_url() {
    // base_url may include a path component (in practice the prod GCS URL
    // does: `/cli`). The function appends `/{channel}`.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cli/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .mount(&server)
        .await;

    let base = format!("{}/cli", server.uri());
    let v = fetch_gcs_version_from_base("stable", &base).await.unwrap();
    assert_eq!(v, "0.1.181");
}

// ─────────────────────────────────────────────────────────────────────────────
// Retry behavior — these tests intentionally exercise the 1s+2s+4s backoff,
// so each takes ~7 seconds. They run in parallel.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gcs_pointer_retries_on_5xx_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(503).set_body_string("backend down"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_gives_up_after_max_retries() {
    let server = MockServer::start().await;
    // 4 attempts total: initial + 3 retries.
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(500))
        .expect(4)
        .mount(&server)
        .await;

    let err = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("HTTP 500"), "msg: {msg}");
}

#[tokio::test]
async fn gcs_pointer_retries_on_empty_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.181"))
        .mount(&server)
        .await;

    let v = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap();
    assert_eq!(v, "0.1.181");
}

#[tokio::test]
async fn gcs_pointer_alpha_propagates_error_from_either_pointer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0.1.182-alpha.1"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(500))
        .expect(4)
        .mount(&server)
        .await;

    let err = fetch_gcs_version_from_base("alpha", &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("HTTP 500"), "msg: {msg}");
}

#[tokio::test]
async fn gcs_pointer_4xx_is_retryable_until_exhausted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(404))
        .expect(4)
        .mount(&server)
        .await;

    let err = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("HTTP 404"), "msg: {msg}");
}

#[tokio::test]
async fn gcs_pointer_includes_url_in_error_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(500))
        .expect(4)
        .mount(&server)
        .await;

    let err = fetch_gcs_version_from_base("stable", &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("/stable"), "url should be in error: {msg}");
}

#[tokio::test]
async fn gcs_pointer_connection_refused_is_retried_and_returns_error() {
    // Bind a TcpListener to claim a port, then drop it so connections refuse.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let url = format!("http://127.0.0.1:{port}");

    let err = fetch_gcs_version_from_base("stable", &url)
        .await
        .unwrap_err();
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("fetch failed")
            || msg.contains("connection")
            || msg.contains("error sending request")
            || msg.contains("refused"),
        "expected network error message, got: {msg}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// download_silent — same body shape as download_with_progress but no
// progress bar to capture.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// download_with_progress — same contract; covers the spinner path
// (no Content-Length) and the progress-bar path (with Content-Length).
// ─────────────────────────────────────────────────────────────────────────────

