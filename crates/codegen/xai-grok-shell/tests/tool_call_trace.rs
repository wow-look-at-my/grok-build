//! This binary owns the process-global tracer.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::Context;
use xai_grok_telemetry::config::{TelemetryConfig, TelemetryMode};
use xai_grok_test_support::{MockInferenceServer, MockOtelServer};

#[derive(Clone, Debug)]
struct CapturedSpan {
    name: String,
    attributes: BTreeMap<String, String>,
}

struct SpanCapture(Arc<Mutex<HashMap<u64, CapturedSpan>>>);

struct FieldVisitor<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl<S> tracing_subscriber::Layer<S> for SpanCapture
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut attributes = BTreeMap::new();
        attrs.record(&mut FieldVisitor(&mut attributes));
        self.0.lock().unwrap().insert(
            id.into_u64(),
            CapturedSpan {
                name: attrs.metadata().name().to_owned(),
                attributes,
            },
        );
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        if let Some(span) = self.0.lock().unwrap().get_mut(&id.into_u64()) {
            values.record(&mut FieldVisitor(&mut span.attributes));
        }
    }
}

impl SpanCapture {
    fn find(&self, invocation: &str) -> Option<CapturedSpan> {
        self.0
            .lock()
            .unwrap()
            .values()
            .find(|span| attr(span, "invocation_id") == Some(invocation))
            .cloned()
    }
}

const SILENT: &str = "018f6b6c-7b3a-7c3a-8c3a-000000000001";
const ENABLED: &str = "018f6b6c-7b3a-7c3a-8c3a-000000000002";
const CANARY_PATH: &str = "/tmp/secret-project/note.txt";
const CANARY_BODY: &str = "CANARY_BODY";

fn set_env(key: &str, value: &str) {
    // SAFETY: this binary has one test, and env is set before other threads start.
    unsafe { std::env::set_var(key, value) }
}

fn unset_env(key: &str) {
    // SAFETY: see [`set_env`].
    unsafe { std::env::remove_var(key) }
}

fn install_tracing(traces: &str, capture: SpanCapture) {
    set_env("GROK_INTERNAL_OTLP_TRACES_ENDPOINT", traces);
    set_env("GROK_INSTRUMENTATION", "server");
    set_env("GROK_OTEL_FILTER", "info");
    set_env("OTEL_BSP_SCHEDULE_DELAY", "50");
    set_env("OTEL_EXPORTER_OTLP_TIMEOUT", "2000");
    set_env("OTEL_TRACES_EXPORTER", "otlp");
    set_env("GROK_TELEMETRY_ENABLED", "true");
    unset_env("DISABLE_TELEMETRY");
    unset_env("GROK_EXTERNAL_OTEL");
    let config = xai_grok_shell::agent::init::build_default_otel_layer_config();
    xai_grok_shell::auth::credential_provider::wire_otel_deployment_key("test-key".into());
    let layer = xai_grok_telemetry::otel_layer::build_otel_layer(
        xai_grok_telemetry::otel_layer::OtelClientInfo {
            client_name: "grok-test",
            client_version: "test",
            service_version: "test",
            app_entrypoint: "cli",
        },
        config,
    );
    use tracing_subscriber::layer::SubscriberExt as _;
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(layer).with(capture),
    )
    .expect("install subscriber");
}

fn init_product(server: &MockInferenceServer, mode: TelemetryMode) {
    let config = TelemetryConfig {
        events_url: Some(format!("{}/events", server.url())),
        events_api_key: Some("test-key".into()),
        mixpanel_enabled: false,
        mixpanel_token: None,
        ..TelemetryConfig::default()
    };
    xai_grok_telemetry::init(
        config,
        mode,
        None,
        None,
        None,
        None,
        "test".into(),
        None,
        xai_grok_shell::http::shared_client(),
    );
}

fn projected(invocation: &str, exit_code: i32) -> xai_grok_telemetry::events::ToolCallCompleted {
    let parent = tracing::info_span!("tools.execute");
    let span = xai_grok_shell::session::tool_execution_span(
        &parent,
        "session",
        "grep",
        "grep",
        "grep-call",
        12,
        false,
    );
    let output = xai_grok_shell::session::grep_output(exit_code);
    xai_grok_shell::session::complete_projected_call(span, invocation, &output, "success")
}

fn attr<'a>(span: &'a CapturedSpan, key: &str) -> Option<&'a str> {
    span.attributes.get(key).map(String::as_str)
}

#[tokio::test]
async fn span_and_product_row_agree_and_product_gate_is_independent() {
    let home = std::env::temp_dir().join(format!("tool-call-trace-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    set_env("GROK_HOME", home.to_str().unwrap());
    let traces = MockOtelServer::start().await.expect("traces");
    let product = MockInferenceServer::start().await.expect("product");
    let capture = SpanCapture::default();
    install_tracing(&format!("{}/v1/traces", traces.origin()), capture.clone());
    init_product(&product, TelemetryMode::SessionMetrics);

    let silent = projected(SILENT, -1);
    xai_grok_telemetry::session_ctx::log_event_now(silent).await;
    assert!(capture.find(SILENT).is_some(), "session-metrics span");
    assert_eq!(Vec::<Value>::new(), product.telemetry_events());

    init_product(&product, TelemetryMode::Enabled);
    let event = projected(ENABLED, -1);
    // The product row's metadata is this serialization.
    let metadata = serde_json::to_value(&event).expect("serialize event");
    xai_grok_telemetry::session_ctx::log_event_now(event).await;
    xai_grok_telemetry::otel_layer::shutdown_otel();
    traces
        .recorder()
        .wait_for_span_silence(Duration::from_millis(300))
        .await
        .expect("disabled trace export stays silent");
    assert!(
        traces.recorder().spans().is_empty(),
        "OTLP trace export is hard-disabled: no span may leave"
    );
    assert_eq!(
        Vec::<Value>::new(),
        product.telemetry_events(),
        "product events are hard-disabled: no row may post"
    );
    let span = &capture.find(ENABLED).expect("matching span");
    assert_eq!(span.name, "tool.execution");
    assert_eq!(attr(span, "session_id"), Some("session"));
    assert_eq!(
        metadata.get("model_id").and_then(Value::as_str),
        Some("grok-4.6")
    );
    assert_eq!(
        metadata.get("invocation_id").and_then(Value::as_str),
        Some(ENABLED)
    );
    assert_eq!(
        metadata.get("tool_id").and_then(Value::as_str),
        Some("GrokBuild:grep")
    );
    assert_eq!(
        metadata.get("tool_version").and_then(Value::as_str),
        Some("current")
    );
    assert_eq!(
        metadata.get("source_status").and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        metadata.get("source_reason").and_then(Value::as_str),
        Some("search.unclassified_exit")
    );
    assert_eq!(
        metadata.get("outcome").and_then(Value::as_str),
        Some("error")
    );
    assert_eq!(attr(span, "model_id"), Some("grok-4.6"));
    assert_eq!(attr(span, "invocation_id"), Some(ENABLED));
    assert_eq!(attr(span, "tool_id"), Some("GrokBuild:grep"));
    assert_eq!(attr(span, "tool_version"), Some("current"));
    assert_eq!(attr(span, "source_status"), Some("failed"));
    assert_eq!(
        attr(span, "source_reason"),
        Some("search.unclassified_exit")
    );
    assert_eq!(attr(span, "outcome"), Some("error"));
    assert!(!span.attributes.contains_key("path_scope"));
    let rendered = format!("{metadata} {span:?}");
    assert!(!rendered.contains(CANARY_PATH));
    assert!(!rendered.contains("secret-project"));
    assert!(!rendered.contains("CANARY_PATTERN"));
    assert!(!rendered.contains("CANARY_STDOUT"));
    assert!(!rendered.contains("CANARY_STDERR"));
    assert!(!rendered.contains(CANARY_BODY));
}
