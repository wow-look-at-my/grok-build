//! Wire test for the **no-double-send invariant** (the credential-leak guard).

use std::time::Duration;

use xai_grok_telemetry::external;
use xai_grok_test_support::{MockOtelServer, OtelExport};

#[tokio::test]
async fn refuses_to_activate_when_internal_consumed_standard_vars() {
    let server = MockOtelServer::start().await.unwrap();

    let env = server.exporter_env();
    let mut cfg = external::ExternalOtelConfig::resolve_with(|name| env.get(name).cloned(), None)
        .expect("config resolves (the refusal happens at init, not resolution)");
    cfg.client = external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: "0.0.0-test".into(),
        app_entrypoint: "cli".into(),
    };
    cfg.internal_pipeline_consumed_otel_vars = true;

    external::init(Some(cfg));
    assert!(
        !external::is_active(),
        "external stream MUST refuse to activate to prevent credential leakage"
    );

    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionNew {
        session_id: "sess-guard".into(),
        client_identifier: None,
        client_version: None,
        is_git_repo: true,
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
    });
    tokio::task::spawn_blocking(external::flush).await.unwrap();

    server
        .recorder()
        .wait_for_silence(Duration::from_millis(600), |events| !events.is_empty())
        .await
        .expect("nothing may be exported when refused");
    assert_eq!(Vec::<OtelExport>::new(), server.recorder().exports());

    tokio::task::spawn_blocking(external::shutdown)
        .await
        .unwrap();
}
