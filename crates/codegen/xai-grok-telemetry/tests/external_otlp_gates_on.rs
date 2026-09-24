//! Wire test pinning the **build-baseline disabled contract** for the external
//! OTEL stream on its highest-risk configuration: both content gates ON (prompt
//! text and tool parameters would leave the process) *and* identity attributes
//! set. Because `external::build_handle` returns `None` in this build, even this
//! fully-opted-in, gates-on config never activates: `is_active()` stays false,
//! `set_identity` / `apply_remote_policy` are inert no-ops, and the
//! `MockOtelServer` receives nothing.
//!
//! Single sequential test because the `EXTERNAL` registry is a
//! process-global `OnceLock`, so each init-config scenario is its own test
//! binary.

use std::time::Duration;

use xai_grok_telemetry::external::{self, ExternalOtelRemotePolicy, IdentityAttrs};
use xai_grok_test_support::{MockOtelServer, OtelSignal};

const SECRET_KEY: &str = "sk-LEAKaaaaaaaaaaaaaaaa1234567890";
const SECRET_MODEL: &str = "grok-4-sk-LEAKmodel1234567890abcd";
const PROMPT_MARK: &str = "promptbodymarker";
const PARAM_MARK: &str = "parammarker";
const LONG_CMD_MARK: &str = "longcmdmarker";
const DENY_CMD_MARK: &str = "denycmdmarker";
const RESPONSE_MARK: &str = "assistantresponsemarker";
const OAUTH_EMAIL: &str = "otel.parity.on@example.com";
const CLIENT_VERSION: &str = "9.9.9-cv";

#[tokio::test]
async fn external_stream_gates_on_end_to_end() {
    let server = MockOtelServer::start().await.unwrap();

    let mut env = server.exporter_env();
    env.extend([
        ("OTEL_LOG_USER_PROMPTS", "1".to_owned()),
        ("OTEL_LOG_TOOL_DETAILS", "1".to_owned()),
        ("OTEL_LOG_ASSISTANT_RESPONSES", "1".to_owned()),
        ("OTEL_LOG_TOOL_CONTENT", "1".to_owned()),
        (
            "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE",
            "cumulative".to_owned(),
        ),
        ("OTEL_METRICS_INCLUDE_VERSION", "1".to_owned()),
    ]);
    let mut cfg = external::ExternalOtelConfig::resolve_with(|name| env.get(name).cloned(), None)
        .expect("double opt-in must resolve");
    assert!(cfg.gates.log_user_prompts && cfg.gates.log_tool_details);
    assert!(cfg.gates.log_tool_content, "content gate must be on");
    assert!(
        cfg.gates.log_assistant_responses,
        "assistant gate must be on in this binary"
    );
    cfg.client = external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: CLIENT_VERSION.into(),
        app_entrypoint: "cli".into(),
    };

    external::init(Some(cfg));
    assert!(
        !external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline (gates on)"
    );

    external::set_identity(IdentityAttrs {
        user_id: Some("user-x".into()),
        email: Some(OAUTH_EMAIL.into()),
        organization_id: Some("org-acme".into()),
        team_id: Some("team-7".into()),
        deployment_id: Some("deploy-eu".into()),
    });

    assert!(!xai_grok_telemetry::is_enabled());

    xai_grok_telemetry::log_event(xai_grok_telemetry::events::SessionHarness {
        session_id: "sess-gates-on".into(),
        client_identifier: Some("grok-pager".into()),
        model_id: "grok-4".into(),
        agent_name: "grok-build-plan".into(),
        permission_mode: xai_grok_telemetry::enums::PermissionMode::Ask,
        mcp_server_names: vec!["internal-mcp".into()],
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
        prompt_length: 100,
        model_id: "grok-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some(format!("refactor {PROMPT_MARK} with key {SECRET_KEY} now")),
        command_name: Some("compact".into()),
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
        model_id: SECRET_MODEL.into(),
        duration_ms: 5,
        stop_reason: Some("stop".into()),
        prompt_tokens: Some(11),
        completion_tokens: Some(7),
        reasoning_tokens: Some(3),
        cached_prompt_tokens: Some(9),
        cache_creation_tokens: None,
        context_tokens: None,
        cost_usd_ticks: None,
    });
    let mut github = xai_grok_telemetry::events::completed_for_test("github__create_issue", "grok");
    github.duration_ms = 12;
    github.file_path = Some("/tmp/projectdir/config.toml".into());
    github.parameters = Some(serde_json::json!({
        "marker": PARAM_MARK,
        "token": SECRET_KEY,
        "deep": {"a": {"b": "c"}},
    }));
    github.tool_use_id = Some("call-github".into());
    github.tool_output = Some(format!("ok {PARAM_MARK}"));
    xai_grok_telemetry::log_event(github);
    let long_command = format!("{LONG_CMD_MARK}{}", "x".repeat(600));
    let mut bash = xai_grok_telemetry::events::completed_for_test("run_terminal_cmd", "grok");
    bash.duration_ms = 8;
    bash.parameters = Some(serde_json::json!({ "command": long_command }));
    bash.tool_use_id = Some("call-bash-long".into());
    xai_grok_telemetry::log_event(bash);
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::PermissionDecisionRecord {
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
            parameters: Some(serde_json::json!({ "command": DENY_CMD_MARK })),
            tool_use_id: Some("call-deny-1".into()),
        },
    });
    xai_grok_telemetry::external::emit(&xai_grok_telemetry::events::AssistantResponse {
        response_length: RESPONSE_MARK.len(),
        response_text: Some(RESPONSE_MARK.into()),
    });

    tokio::task::spawn_blocking(external::flush).await.unwrap();
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

    tokio::task::spawn_blocking(external::flush).await.unwrap();
    tokio::task::spawn_blocking(|| {
        external::apply_remote_policy(ExternalOtelRemotePolicy {
            force_disable: true,
            lock_content_gates: false,
        })
    })
    .await
    .unwrap();
    assert!(
        !external::is_active(),
        "kill switch must clear the emission gate"
    );
    let (silence, ()) = tokio::join!(
        server
            .recorder()
            .wait_for_silence(Duration::from_millis(400), |events| {
                events
                    .iter()
                    .any(|event| event.signal() == OtelSignal::Logs)
            }),
        async {
            xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
                prompt_length: 1,
                model_id: "grok-4".into(),
                client_identifier: None,
                screen_mode: None,
                prompt_text: Some("post-kill".into()),
                command_name: None,
            });
        },
    );
    silence.expect("no log exports after the remote kill switch");

    tokio::task::spawn_blocking(external::shutdown)
        .await
        .unwrap();
}
