//! Dedicated binary: the external-stream `OnceLock` is process-global, so this
//! construction canary cannot live in the lib test suite.
//!
//! The external stream is hard-disabled in this build, so even a valid double
//! opt-in leaves it inactive and the prompt text never rides the event.

use xai_grok_telemetry::external::{self, ExternalOtelConfig};

#[test]
fn prompt_submitted_prompt_text_is_none_while_stream_disabled() {
    let mut cfg = ExternalOtelConfig::resolve_with(
        |name| match name {
            "GROK_EXTERNAL_OTEL" => Some("1".into()),
            "OTEL_LOGS_EXPORTER" => Some("console".into()),
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
    external::init(Some(cfg));
    assert!(
        !external::is_active(),
        "external OTLP stream is hard-disabled in the build baseline (console exporter)"
    );

    // The same gate `turn.rs` applies when it builds `PromptSubmitted`.
    let user_message = "PARITY_PROMPT live construction";
    let ev = xai_grok_telemetry::events::PromptSubmitted {
        prompt_length: user_message.len(),
        model_id: "grok-4".into(),
        client_identifier: None,
        screen_mode: None,
        prompt_text: external::is_active().then(|| user_message.to_owned()),
        command_name: None,
    };
    assert_eq!(ev.prompt_text, None);
    external::shutdown();
}
