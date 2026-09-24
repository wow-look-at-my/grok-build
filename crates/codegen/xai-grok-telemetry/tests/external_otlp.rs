//! Integration test pinning the **build-baseline disabled contract** for the
//! external OTEL stream. In this build `external::build_handle` returns `None`
//! unconditionally, so a full double opt-in (standard `OTEL_*` env vars + the
//! `GROK_EXTERNAL_OTEL` master switch) still *resolves* but never *activates*:
//! `is_active()` stays false and the `MockOtelServer` must receive nothing —
//! no logs, no metrics — even after an explicit flush.

use std::time::Duration;

use xai_grok_telemetry::external::IdentityAttrs;
use xai_grok_test_support::MockOtelServer;

const CANARY_MODEL: &str = "sk-CANARYabcdefghij1234567890";
const CANARY_PROMPT: &str = "CANARY_PROMPT_TEXT do not export";
const CANARY_MCP: &str = "canary-internal-mcp-server";
const CANARY_CMD: &str = "CANARY_BASH_COMMAND_ls_la";
const CANARY_RESPONSE: &str = "CANARY_ASSISTANT_PROSE do not export";
const OAUTH_EMAIL: &str = "otel.parity@example.com";

fn deny_decision(
    command: &str,
    tool_use_id: &str,
) -> xai_grok_telemetry::events::PermissionDecisionRecord {
    xai_grok_telemetry::events::PermissionDecisionRecord {
        payload: xai_grok_telemetry::events::PermissionDecisionPayload {
            tool_name: "run_terminal_cmd".into(),
            access_kind: xai_grok_telemetry::events::AccessKind::Bash,
            decision: xai_grok_telemetry::events::PermissionOutcome::Deny,
            wait_ms: 10,
            permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
            source: Some("user_reject".into()),
            subagent_session_id: None,
            subagent_type: None,
            manager_prompt_attempted: None,
            prompt_outcome: None,
            prompt_outcome_detail: None,
            remember_tool_approvals: None,
            decision_reason: None,
            classifier_source: None,
            classifier_verdict: None,
            security_findings: None,
            classifier_latency_ms: None,
            auto_denials_consecutive: None,
            auto_denials_total: None,
        },
        tool_input: xai_grok_telemetry::events::ExternalToolInput {
            parameters: Some(serde_json::json!({ "command": command })),
            tool_use_id: Some(tool_use_id.into()),
        },
    }
}

#[tokio::test]
async fn external_stream_end_to_end() {
    let server = MockOtelServer::start().await.unwrap();

    let env = server.exporter_env();
    let mut cfg = xai_grok_telemetry::external::ExternalOtelConfig::resolve_with(
        |name| env.get(name).cloned(),
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

    xai_grok_telemetry::external::set_identity(IdentityAttrs {
        user_id: Some("user-gates-off".into()),
        email: Some(OAUTH_EMAIL.into()),
        organization_id: None,
        team_id: None,
        deployment_id: None,
    });

    assert!(!xai_grok_telemetry::is_enabled());
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
        memory_retrieval_mode: xai_grok_telemetry::events::MemoryRetrievalMode::Disabled,
        is_git_repo: true,
        auto_update: None,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
        prompt_length: CANARY_PROMPT.len(),
        model_id: "grok-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some(CANARY_PROMPT.into()),
        command_name: None,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
        model_id: CANARY_MODEL.into(),
        duration_ms: 5,
        stop_reason: Some("stop".into()),
        prompt_tokens: Some(11),
        completion_tokens: Some(7),
        reasoning_tokens: None,
        cached_prompt_tokens: None,
        cache_creation_tokens: None,
        context_tokens: None,
        cost_usd_ticks: None,
    });
    let mut tool_completed =
        xai_grok_telemetry::events::completed_for_test("run_terminal_cmd", "grok");
    tool_completed.duration_ms = 3;
    tool_completed.parameters = Some(serde_json::json!({ "command": CANARY_CMD }));
    tool_completed.tool_use_id = Some("call-gates-off".into());
    xai_grok_telemetry::log_event(tool_completed);
    xai_grok_telemetry::log_event(deny_decision(CANARY_CMD, "call-deny-off"));
    xai_grok_telemetry::external::emit(&xai_grok_telemetry::events::AssistantResponse {
        response_length: CANARY_RESPONSE.len(),
        response_text: Some(CANARY_RESPONSE.into()),
    });

    tokio::task::spawn_blocking(xai_grok_telemetry::external::flush)
        .await
        .unwrap();
    // Give any (erroneously constructed) exporter ample time to phone home.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        server.recorder().log_records().is_empty(),
        "disabled external stream must export no logs"
    );
    assert!(
        server.recorder().metric_points().is_empty(),
        "disabled external stream must export no metrics"
    );

    let start = std::time::Instant::now();
    tokio::task::spawn_blocking(xai_grok_telemetry::external::shutdown)
        .await
        .unwrap();
    assert!(
        start.elapsed() <= Duration::from_millis(2500),
        "shutdown watchdog must bound exit at ~2s (took {:?})",
        start.elapsed()
    );
    assert!(!xai_grok_telemetry::external::is_active());

    let (silence, ()) = tokio::join!(
        server
            .recorder()
            .wait_for_silence(Duration::from_millis(400), |events| !events.is_empty()),
        async {
            xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
                prompt_length: 1,
                model_id: "grok-4".into(),
                client_identifier: None,
                screen_mode: None,
                prompt_text: None,
                command_name: None,
            });
        },
    );
    silence.expect("no exports after shutdown");

    tokio::task::spawn_blocking(xai_grok_telemetry::external::shutdown)
        .await
        .unwrap();
}
