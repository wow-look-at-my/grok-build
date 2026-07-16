//! Wire test pinning the **build-baseline disabled contract** for the external
//! OTEL stream on its highest-risk configuration: both content gates ON (prompt
//! text and tool parameters would leave the process) *and* identity attributes
//! set. Because `external::build_handle` returns `None` in this build, even this
//! fully-opted-in, gates-on config never activates: `is_active()` stays false,
//! `set_identity` / `apply_remote_policy` are inert no-ops, and the in-process
//! OTLP collector receives nothing.
//!
//! Single sequential `#[test]` because the `EXTERNAL` registry is a
//! process-global `OnceLock`, so each init-config scenario is its own test
//! binary.

mod otlp_collector;

use otlp_collector as col;
use xai_grok_telemetry::external::{self, ExternalOtelRemotePolicy, IdentityAttrs};

// Secret shapes — MUST NOT reach the wire (here: nothing reaches the wire).
const SECRET_KEY: &str = "sk-LEAKaaaaaaaaaaaaaaaa1234567890";
const SECRET_MODEL: &str = "grok-4-sk-LEAKmodel1234567890abcd";
// Benign markers that a live gates-on stream would have exported.
const PROMPT_MARK: &str = "promptbodymarker";
const PARAM_MARK: &str = "parammarker";
const CLIENT_VERSION: &str = "9.9.9-cv";

#[test]
fn external_stream_gates_on_end_to_end() {
    let collected = col::Collected::default();
    let endpoint = col::start_collector(collected.clone());

    let mut cfg = external::ExternalOtelConfig::resolve_with(
        |name| match name {
            "GROK_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" | "OTEL_METRICS_EXPORTER" => Some("otlp".into()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(endpoint.clone()),
            // Both content gates ON.
            "OTEL_LOG_USER_PROMPTS" | "OTEL_LOG_TOOL_DETAILS" => Some("1".into()),
            "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE" => Some("cumulative".into()),
            "OTEL_METRICS_INCLUDE_VERSION" => Some("1".into()),
            "OTEL_METRIC_EXPORT_INTERVAL" => Some("200".into()),
            "OTEL_BLRP_SCHEDULE_DELAY" => Some("100".into()),
            _ => None,
        },
        None,
    )
    .expect("double opt-in must resolve");
    // Config resolution still reflects the requested gates; only activation is
    // disabled downstream.
    assert!(cfg.gates.log_user_prompts && cfg.gates.log_tool_details);
    cfg.client = external::config::ExternalClientInfo {
        service_version: "0.0.0-test".into(),
        client_version: CLIENT_VERSION.into(),
        app_entrypoint: "cli".into(),
    };

    external::init(Some(cfg));
    assert!(
        !external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline (gates-on)"
    );

    // Identity attrs on an inactive stream are an inert no-op — must not
    // activate it.
    external::set_identity(IdentityAttrs {
        user_id: Some("user-x".into()),
        organization_id: Some("org-acme".into()),
        team_id: Some("team-7".into()),
        deployment_id: Some("deploy-eu".into()),
    });
    assert!(!external::is_active(), "set_identity must not activate a disabled stream");
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
        is_git_repo: true,
        auto_update: None,
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::PromptSubmitted {
        prompt_length: 100,
        model_id: "grok-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: Some(format!("refactor {PROMPT_MARK} with key {SECRET_KEY} now")),
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::ModelResponseReceived {
        model_id: SECRET_MODEL.into(),
        duration_ms: 5,
        stop_reason: Some("stop".into()),
        prompt_tokens: Some(11),
        completion_tokens: Some(7),
        reasoning_tokens: Some(3),
        cached_prompt_tokens: Some(9),
    });
    xai_grok_telemetry::log_event(xai_grok_telemetry::events::ToolCallCompleted {
        tool_name: "github__create_issue".into(),
        outcome: xai_file_utils::events::types::ToolOutcome::Success,
        duration_ms: 12,
        file_path: Some("/tmp/projectdir/config.toml".into()),
        parameters: Some(serde_json::json!({
            "marker": PARAM_MARK,
            "token": SECRET_KEY,
            "deep": {"a": {"b": "c"}},
        })),
    });

    external::flush();

    // Give any (erroneous) exporter ample time to phone home.
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(
        collected.logs_len(),
        0,
        "disabled gates-on stream must export no logs"
    );
    assert_eq!(
        collected.metrics_len(),
        0,
        "disabled gates-on stream must export no metrics"
    );

    // The remote fleet kill switch is likewise inert against an already-disabled
    // stream — it must not error and must leave the stream inactive.
    external::apply_remote_policy(ExternalOtelRemotePolicy {
        force_disable: true,
        lock_content_gates: false,
    });
    assert!(!external::is_active());

    external::shutdown();
    assert!(!external::is_active());
}
