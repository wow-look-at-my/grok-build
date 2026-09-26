//! The external stream sends no earliest party credential. In this build it sends nothing at all.

use std::time::Duration;

use xai_grok_telemetry::enums::PermissionMode;
use xai_grok_telemetry::events::SessionNew;
use xai_grok_telemetry::external::ExternalOtelConfig;
use xai_grok_telemetry::external::config::ExternalClientInfo;
use xai_grok_test_support::MockOtelServer;

#[tokio::test]
async fn external_stream_carries_no_first_party_credential() {
    let server = MockOtelServer::start().await.unwrap();
    let env = server.exporter_env();
    let mut cfg = ExternalOtelConfig::resolve_with(|name| env.get(name).cloned(), None)
        .expect("exporter_env resolves to a double opt-in config");
    cfg.client = ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: "0.0.0-test".into(),
        app_entrypoint: "cli".into(),
    };
    xai_grok_telemetry::external::init(Some(cfg));
    assert!(
        !xai_grok_telemetry::external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline"
    );

    xai_grok_telemetry::log_event(SessionNew {
        session_id: "sess-collector-1".into(),
        client_identifier: None,
        client_version: None,
        is_git_repo: true,
        permission_mode: PermissionMode::Ask,
    });
    tokio::task::spawn_blocking(xai_grok_telemetry::external::flush)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    tokio::task::spawn_blocking(xai_grok_telemetry::external::shutdown)
        .await
        .unwrap();

    let exports = server.recorder().exports();
    assert!(
        exports.is_empty(),
        "disabled external stream must export nothing"
    );
    assert_eq!(
        vec![None; exports.len()],
        exports
            .iter()
            .map(|export| export.header("authorization"))
            .collect::<Vec<_>>()
    );
}
