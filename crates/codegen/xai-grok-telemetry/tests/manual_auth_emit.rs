//! Wire test pinning the **build-baseline disabled contract** for product
//! events. `client::track` is hard-disabled in this build (returns before any
//! routing), so `log_event(ManualAuth)` must NOT POST to the product events
//! endpoint — even with `TelemetryMode::Enabled` and a fully-configured client
//! pointed at a live collector. Mocks the observability backend (real HTTP
//! collector) and asserts it receives nothing.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use xai_grok_telemetry::client;
use xai_grok_telemetry::config::{TelemetryConfig, TelemetryMode};
use xai_grok_telemetry::events::{AuthTokenKind, ManualAuth, ManualAuthReason, ManualAuthSurface};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_auth_does_not_post_when_product_telemetry_disabled() {
    let bodies: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = bodies.clone();
    let app = axum::Router::new().route(
        "/events",
        axum::routing::post(move |axum::Json(v): axum::Json<serde_json::Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(v);
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/events", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Fully configured, explicitly Enabled — the disable happens downstream in
    // `client::track`, not in configuration.
    client::init(
        TelemetryConfig {
            events_url: Some(url),
            events_api_key: Some("test-key".into()),
            mixpanel_enabled: false,
            ..TelemetryConfig::default()
        },
        TelemetryMode::Enabled,
        Some("user-xyz".into()),
        None,
        None,
        None,
        "0.0.0-test".into(),
        None,
        reqwest::Client::new(),
    );

    xai_grok_telemetry::log_event(ManualAuth {
        reason: ManualAuthReason::RefreshTokenRejected,
        trigger: ManualAuthSurface::Turn,
        token_kind: AuthTokenKind::OidcSession,
        principal: Some("user-xyz".into()),
    });

    // The emit is fire-and-forget; give any (erroneous) POST ample time to land,
    // then assert the collector stayed empty.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let count = bodies.lock().unwrap().len();
    assert_eq!(
        count, 0,
        "product events are hard-disabled: no POST may reach the events endpoint"
    );

    server.abort();
}
