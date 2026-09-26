//! Wire test pinning the **build-baseline disabled contract** under ambient
//! session context. Events emitted inside a `with_session_ctx` scope would,
//! on a live stream, carry `session.id` / `turn_number` / `prompt.id` /
//! `event.sequence`. Because `external::build_handle` returns `None` in this
//! build, the stream never activates: emissions inside the ctx are no-ops and
//! the in-process OTLP collector receives nothing.

use std::sync::Arc;
use std::time::Duration;

use xai_grok_telemetry::external;
use xai_grok_test_support::MockOtelServer;

#[tokio::test]
async fn ambient_ctx_injects_session_turn_and_prompt_id() {
    let server = MockOtelServer::start().await.unwrap();

    let env = server.exporter_env();
    let mut cfg = external::ExternalOtelConfig::resolve_with(|name| env.get(name).cloned(), None)
        .expect("double opt-in must resolve");
    cfg.client = external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: "0.0.0-test".into(),
        app_entrypoint: "cli".into(),
    };
    external::init(Some(cfg));
    assert!(
        !external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline (session ctx)"
    );

    // Emit inside a session ctx (turn_number = 3) so the ambient snapshot is
    // populated. With the stream disabled these emissions are no-ops.
    let ctx = xai_grok_telemetry::TelemetryCtx::new(
        "sess-ctx".to_owned(),
        Arc::new(tokio::sync::Mutex::new(3usize)),
    );
    xai_grok_telemetry::with_session_ctx(ctx, async {
        xai_grok_telemetry::session_ctx::begin_prompt_id();
        xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
            prompt_length: 42,
            model_id: "grok-4".into(),
            client_identifier: None,
            screen_mode: None,
            prompt_text: None,
            command_name: None,
        });
        xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
            model_id: "grok-4".into(),
            duration_ms: 5,
            stop_reason: Some("stop".into()),
            prompt_tokens: Some(11),
            completion_tokens: None,
            reasoning_tokens: None,
            cached_prompt_tokens: None,
            cache_creation_tokens: None,
            context_tokens: None,
            cost_usd_ticks: None,
        });
    })
    .await;

    tokio::task::spawn_blocking(external::flush).await.unwrap();
    // Give any (erroneous) exporter ample time to phone home.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        server.recorder().log_records().is_empty(),
        "disabled external stream must export no logs from a session ctx"
    );
    assert!(
        server.recorder().metric_points().is_empty(),
        "disabled external stream must export no metrics from a session ctx"
    );

    tokio::task::spawn_blocking(external::shutdown)
        .await
        .unwrap();
    assert!(!external::is_active());
}
