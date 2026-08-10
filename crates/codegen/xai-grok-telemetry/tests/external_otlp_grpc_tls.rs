//! HTTPS (TLS) gRPC transport coverage of the **build-baseline disabled
//! contract** for the external OTEL stream. Mirrors `external_otlp_grpc.rs`,
//! but points the double opt-in at a live TLS collector whose CA the client
//! trusts through the standard `OTEL_EXPORTER_OTLP_CERTIFICATE` variable —
//! the one configuration that would otherwise export successfully. Because
//! `external::build_handle` returns `None` in this build the stream never
//! activates, so the collector must receive nothing over TLS either.
//!
//! Lives in its own integration-test binary because the external telemetry
//! registry is a process-global `OnceLock`.

mod otlp_collector;

use otlp_collector as col;

#[test]
fn external_stream_grpc_over_tls_end_to_end() {
    let tls = col::generate_tls_material();
    let ca_file = tempfile::NamedTempFile::new().expect("CA temp file");
    std::fs::write(ca_file.path(), &tls.ca_cert_pem).expect("write CA pem");
    let ca_path = ca_file.path().to_str().expect("utf-8 CA path").to_string();

    let collected = col::Collected::default();
    let endpoint = col::start_grpc_tls_collector(
        collected.clone(),
        tls.server_cert_pem.clone(),
        tls.server_key_pem.clone(),
    );
    assert!(endpoint.starts_with("https://"), "{endpoint}");

    let mut cfg = xai_grok_telemetry::external::ExternalOtelConfig::resolve_with(
        |name| match name {
            "GROK_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" | "OTEL_METRICS_EXPORTER" => Some("otlp".into()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(endpoint.clone()),
            "OTEL_EXPORTER_OTLP_PROTOCOL" => Some("grpc".into()),
            "OTEL_EXPORTER_OTLP_CERTIFICATE" => Some(ca_path.clone()),
            "OTEL_METRIC_EXPORT_INTERVAL" => Some("200".into()),
            "OTEL_BLRP_SCHEDULE_DELAY" => Some("100".into()),
            _ => None,
        },
        None,
    )
    .expect("double opt-in must resolve");
    assert_eq!(cfg.logs_ca_certificate.as_deref(), Some(ca_path.as_str()));
    cfg.client = xai_grok_telemetry::external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: "0.0.0-test".into(),
        app_entrypoint: "cli".into(),
    };

    xai_grok_telemetry::external::init(Some(cfg));
    assert!(
        !xai_grok_telemetry::external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline (gRPC over TLS)"
    );

    // `SessionNew` maps to the `session.count` metric; `SessionHarness` maps
    // to the `session_start` log record — emit both so neither signal's TLS
    // export path can be the one that leaks.
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionNew {
        session_id: "sess-grpc-tls-1".into(),
        client_identifier: None,
        client_version: None,
        is_git_repo: true,
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionHarness {
        session_id: "sess-grpc-tls-1".into(),
        client_identifier: Some("grok-pager".into()),
        model_id: "grok-4".into(),
        agent_name: "grok-build-plan".into(),
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
        mcp_server_names: vec![],
        plugin_names: vec![],
        skill_names: vec![],
        lsp_server_names: vec![],
        hook_names: vec![],
        agents_md_dir_names: vec![],
        memory_enabled: false,
        is_git_repo: true,
        auto_update: None,
    });

    xai_grok_telemetry::external::flush();

    // Give any (erroneous) TLS exporter ample time to complete a handshake and
    // phone home; the metric interval above is 200ms.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(
        collected.logs_len(),
        0,
        "disabled external stream must export no logs over TLS"
    );
    assert_eq!(
        collected.metrics_len(),
        0,
        "disabled external stream must export no metrics over TLS"
    );

    xai_grok_telemetry::external::shutdown();
    assert!(!xai_grok_telemetry::external::is_active());
}
