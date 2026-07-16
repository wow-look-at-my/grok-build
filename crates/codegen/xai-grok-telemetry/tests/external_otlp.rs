//! Integration test pinning the **build-baseline disabled contract** for the
//! external OTEL stream. In this build `external::build_handle` returns `None`
//! unconditionally, so a full double opt-in (standard `OTEL_*` env vars + the
//! `GROK_EXTERNAL_OTEL` master switch) still *resolves* but never *activates*:
//! `is_active()` stays false and the in-process OTLP collector must receive
//! nothing — no logs, no metrics — even after an explicit flush.

mod otlp_collector;

use otlp_collector as col;

const CANARY_MODEL: &str = "sk-CANARYabcdefghij1234567890";
const CANARY_PROMPT: &str = "CANARY_PROMPT_TEXT do not export";
const CANARY_MCP: &str = "canary-internal-mcp-server";

#[test]
fn external_stream_end_to_end() {
    let collected = col::Collected::default();
    let endpoint = col::start_collector(collected.clone());

    // Resolve through the real config path (double opt-in, gates off).
    let mut cfg = xai_grok_telemetry::external::ExternalOtelConfig::resolve_with(
        |name| match name {
            "GROK_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" | "OTEL_METRICS_EXPORTER" => Some("otlp".into()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(endpoint.clone()),
            // Keep intervals short so any (erroneous) exporter would fire fast.
            "OTEL_METRIC_EXPORT_INTERVAL" => Some("200".into()),
            "OTEL_BLRP_SCHEDULE_DELAY" => Some("100".into()),
            _ => None,
        },
        None,
    )
    .expect("double opt-in must resolve");
    cfg.client = xai_grok_telemetry::external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: "0.0.0-test".into(),
        app_entrypoint: "cli".into(),
    };

    xai_grok_telemetry::external::init(Some(cfg));
    assert!(
        !xai_grok_telemetry::external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline"
    );
    assert!(!xai_grok_telemetry::is_enabled());

    // Emit through the same funnel production uses. With the stream disabled
    // every emission is a no-op — none of these (including the canaries) may
    // reach the collector.
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionNew {
        session_id: "sess-int-1".into(),
        client_identifier: None,
        client_version: None,
        is_git_repo: true,
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionHarness {
        session_id: "sess-int-1".into(),
        client_identifier: Some("grok-pager".into()),
        model_id: "grok-4".into(),
        agent_name: "grok-build-plan".into(),
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
        mcp_server_names: vec![CANARY_MCP.into()],
        plugin_names: vec![],
        skill_names: vec![],
        lsp_server_names: vec![],
        hook_names: vec![],
        agents_md_dir_names: vec![],
        memory_enabled: false,
        is_git_repo: true,
        auto_update: None,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
        prompt_length: CANARY_PROMPT.len(),
        model_id: "grok-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some(CANARY_PROMPT.into()),
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
        model_id: CANARY_MODEL.into(),
        duration_ms: 5,
        stop_reason: Some("stop".into()),
        prompt_tokens: Some(11),
        completion_tokens: Some(7),
        reasoning_tokens: None,
        cached_prompt_tokens: None,
    });

    xai_grok_telemetry::external::flush();

    // Give any (erroneously constructed) exporter ample time to phone home.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(
        collected.logs_len(),
        0,
        "disabled external stream must export no logs"
    );
    assert_eq!(
        collected.metrics_len(),
        0,
        "disabled external stream must export no metrics"
    );

    // Shutdown is a no-op here, but must stay idempotent and leave the stream
    // inactive.
    xai_grok_telemetry::external::shutdown();
    assert!(!xai_grok_telemetry::external::is_active());
    xai_grok_telemetry::external::shutdown();
}
