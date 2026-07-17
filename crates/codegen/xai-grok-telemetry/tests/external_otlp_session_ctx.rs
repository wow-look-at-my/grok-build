//! Wire test pinning the **build-baseline disabled contract** under ambient
//! session context. Events emitted inside a `with_session_ctx` scope would,
//! on a live stream, carry `session.id` / `turn_number` / `prompt.id` /
//! `event.sequence`. Because `external::build_handle` returns `None` in this
//! build, the stream never activates: emissions inside the ctx are no-ops and
//! the in-process OTLP collector receives nothing.

mod otlp_collector;

use std::sync::Arc;

use otlp_collector as col;
use xai_grok_telemetry::external;

#[test]
fn ambient_ctx_injects_session_turn_and_prompt_id() {
    let collected = col::Collected::default();
    let endpoint = col::start_collector(collected.clone());

    let mut cfg = external::ExternalOtelConfig::resolve_with(
        |name| match name {
            "GROK_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" | "OTEL_METRICS_EXPORTER" => Some("otlp".into()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(endpoint.clone()),
            "OTEL_METRIC_EXPORT_INTERVAL" => Some("150".into()),
            "OTEL_BLRP_SCHEDULE_DELAY" => Some("100".into()),
            _ => None,
        },
        None,
    )
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("current-thread runtime");
    rt.block_on(xai_grok_telemetry::with_session_ctx(ctx, async {
        xai_grok_telemetry::session_ctx::begin_prompt_id();
        xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
            prompt_length: 42,
            model_id: "grok-4".into(),
            client_identifier: None,
            screen_mode: None,
            prompt_text: None,
        });
        xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
            model_id: "grok-4".into(),
            duration_ms: 5,
            stop_reason: Some("stop".into()),
            prompt_tokens: Some(11),
            completion_tokens: None,
            reasoning_tokens: None,
            cached_prompt_tokens: None,
        });
    }));

    external::flush();

    // Give any (erroneous) exporter ample time to phone home.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(
        collected.logs_len(),
        0,
        "disabled external stream must export no logs from a session ctx"
    );
    assert_eq!(
        collected.metrics_len(),
        0,
        "disabled external stream must export no metrics from a session ctx"
    );

    external::shutdown();
    assert!(!external::is_active());
}
