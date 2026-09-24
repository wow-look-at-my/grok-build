use super::*;
use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use std::sync::{Arc, Mutex};
#[test]
fn get_env_keys_parses_strings_and_rejects_non_strings() {
    use crate::agent::config::EnvKeys;
    let parse = |v: serde_json::Value| {
        let obj = serde_json::json!({ "env_key": v });
        get_env_keys(obj.as_object().unwrap(), "env_key")
    };
    assert_eq!(parse(serde_json::json!("A")), Some(EnvKeys::single("A")));
    assert_eq!(
        parse(serde_json::json!(["A", "B"])),
        Some(EnvKeys::new(["A", "B"]))
    );
    assert_eq!(parse(serde_json::json!(["A", 123])), None);
    assert_eq!(parse(serde_json::json!([])), None);
}
/// Mock cli-chat-proxy serving `GET /settings` with a fixed status and body.
async fn start_settings_server(
    status: StatusCode,
    body: String,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new().route(
        "/settings",
        get(move || {
            let body = body.clone();
            async move { (status, body) }
        }),
    );
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, handle)
}
/// `fetch_settings_blocking` maps each HTTP outcome to the [`SettingsFetch`] variant the external-OTEL gate relies on.
/// Only 401 yields `Rejected`; every other non-2xx outcome fails closed as `Retry`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_fetch_maps_status_to_outcome() {
    let auth = GrokAuth::test_default();
    let cases: [(StatusCode, &str, &str); 6] = [
        (StatusCode::OK, "{}", "Fetched"),
        (StatusCode::UNAUTHORIZED, "{}", "Rejected"),
        (StatusCode::FORBIDDEN, "{}", "Retry"),
        (StatusCode::TOO_MANY_REQUESTS, "{}", "Retry"),
        (StatusCode::INTERNAL_SERVER_ERROR, "{}", "Retry"),
        (StatusCode::OK, "not json", "Retry"),
    ];
    for (status, body, expected) in cases {
        let (base, server) = start_settings_server(status, body.to_string()).await;
        let a = auth.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            fetch_settings_blocking_with_attempts(&base, &a, None, 1)
        })
        .await
        .unwrap();
        server.abort();
        let got = match outcome {
            SettingsFetch::Fetched(_) => "Fetched",
            SettingsFetch::Rejected => "Rejected",
            SettingsFetch::Retry => "Retry",
        };
        assert_eq!(got, expected, "status {status}, body {body:?}");
    }
}
#[derive(Debug, Default, Clone)]
struct SeenHeaders {
    authorization: Option<String>,
    token_auth: Option<String>,
    user_id: Option<String>,
    email: Option<String>,
    alpha_test_key: Option<String>,
    client_version: Option<String>,
}
#[derive(Clone)]
struct BundleServerState {
    body: serde_json::Value,
    status_code: StatusCode,
    seen_headers: Arc<Mutex<Vec<SeenHeaders>>>,
}
async fn start_bundle_server(
    status_code: StatusCode,
    body: serde_json::Value,
) -> (
    String,
    Arc<Mutex<Vec<SeenHeaders>>>,
    tokio::task::JoinHandle<()>,
) {
    let seen_headers = Arc::new(Mutex::new(Vec::new()));
    let state = BundleServerState {
        body,
        status_code,
        seen_headers: seen_headers.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route(
            "/v1/subagents/bundle",
            get(
                |State(state): State<BundleServerState>, headers: HeaderMap| async move {
                    state.seen_headers.lock().unwrap().push(SeenHeaders {
                        authorization: headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        token_auth: headers
                            .get("x-xai-token-auth")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        user_id: headers
                            .get("x-userid")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        email: headers
                            .get("x-email")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        alpha_test_key: {
                            let _ = &headers;
                            None
                        },
                        client_version: headers
                            .get("x-grok-client-version")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                    });
                    (state.status_code, axum::Json(state.body))
                },
            ),
        )
        .route(
            "/forward/{tail}",
            get(
                |Path(_tail): Path<String>,
                 State(state): State<BundleServerState>,
                 headers: HeaderMap| async move {
                    state.seen_headers.lock().unwrap().push(SeenHeaders {
                        authorization: headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        token_auth: headers
                            .get("x-xai-token-auth")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        user_id: headers
                            .get("x-userid")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        email: headers
                            .get("x-email")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        alpha_test_key: {
                            let _ = &headers;
                            None
                        },
                        client_version: headers
                            .get("x-grok-client-version")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                    });
                    (state.status_code, axum::Json(state.body))
                },
            ),
        )
        .with_state(state);
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("{base}/v1"), seen_headers, handle)
}
fn test_auth() -> GrokAuth {
    GrokAuth {
        key: "token".to_string(),
        user_id: "user-1".to_string(),
        email: Some("test@example.com".to_string()),
        coding_data_retention_opt_out: false,
        expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        ..GrokAuth::default()
    }
}
fn test_auth_manager() -> Arc<xai_grok_login::AuthManager> {
    let dir = tempfile::tempdir().unwrap();
    let mgr =
        xai_grok_login::AuthManager::new(dir.path(), xai_grok_login::GrokComConfig::default());
    mgr.hot_swap(test_auth());
    std::mem::forget(dir);
    Arc::new(mgr)
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_subagent_bundle_success() {
    let body = serde_json::json!({
        "version": "bundle-v1",
        "personas": {"researcher": "persona"},
        "roles": {"reviewer": "role"},
        "agents": {"default": "agent"}
    });
    let (proxy_base_url, seen_headers, server) =
        start_bundle_server(axum::http::StatusCode::OK, body).await;
    let am = test_auth_manager();
    let bundle = fetch_subagent_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap();
    assert_eq!(bundle.version, "bundle-v1");
    assert_eq!(
        bundle.personas.get("researcher"),
        Some(&"persona".to_string())
    );
    assert_eq!(bundle.roles.get("reviewer"), Some(&"role".to_string()));
    assert_eq!(bundle.agents.get("default"), Some(&"agent".to_string()));
    let headers = seen_headers.lock().unwrap();
    let headers = headers.last().unwrap();
    assert_eq!(headers.authorization.as_deref(), Some("Bearer token"));
    assert_eq!(headers.token_auth.as_deref(), Some("xai-grok-cli"));
    assert_eq!(headers.user_id.as_deref(), Some("user-1"));
    assert_eq!(headers.email.as_deref(), Some("test@example.com"));
    assert_eq!(headers.alpha_test_key, None);
    assert!(headers.client_version.is_some());
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_subagent_bundle_uses_deployment_key_without_user_headers() {
    let body = serde_json::json!({
        "version": "bundle-v1",
        "personas": {},
        "roles": {},
        "agents": {}
    });
    let (proxy_base_url, seen_headers, server) =
        start_bundle_server(axum::http::StatusCode::OK, body).await;
    let am = test_auth_manager();
    let bundle = fetch_subagent_bundle(&proxy_base_url, Some(&am), Some("deploy-key"), None)
        .await
        .unwrap();
    assert_eq!(bundle.version, "bundle-v1");
    let headers = seen_headers.lock().unwrap();
    let headers = headers.last().unwrap();
    assert_eq!(headers.authorization.as_deref(), Some("Bearer deploy-key"));
    assert_eq!(headers.token_auth, None);
    assert_eq!(headers.user_id, None);
    assert_eq!(headers.email, None);
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_subagent_bundle_http_failure() {
    let (proxy_base_url, _seen_headers, server) = start_bundle_server(
        axum::http::StatusCode::UNAUTHORIZED,
        serde_json::json!({"error": "unauthorized"}),
    )
    .await;
    let am = test_auth_manager();
    let error = fetch_subagent_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        BackendError::RequestFailed { status: 401, .. }
    ));
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_subagent_bundle_parse_failure() {
    let (proxy_base_url, _seen_headers, server) = start_bundle_server(
        axum::http::StatusCode::OK,
        serde_json::json!({"version": 42}),
    )
    .await;
    let am = test_auth_manager();
    let error = fetch_subagent_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, BackendError::Serialization(_)));
    server.abort();
}
#[test]
fn parse_openai_format_uses_id_field() {
    let value = serde_json::json!({
        "id": "grok-3",
        "object": "model",
        "owned_by": "xai",
        "context_window": 131072
    });
    let result = parse_remote_model_value(&value, "https://api.x.ai/v1").unwrap();
    assert_eq!(result.model, "grok-3");
    assert_eq!(result.base_url, "https://api.x.ai/v1");
    assert_eq!(result.name.as_deref(), Some("grok-3"));
}
#[test]
fn parse_model_field_takes_priority_over_id() {
    let value = serde_json::json!({
        "id": "display-key",
        "model": "actual-model-id",
        "name": "Display Name",
        "context_window": 131072
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.model, "actual-model-id");
    assert_eq!(result.name.as_deref(), Some("Display Name"));
}
#[test]
fn parse_reads_rate_limit_retry_threshold() {
    let value = serde_json::json!({
        "model": "grok-4.5",
        "context_window": 1_000_000,
        "rateLimitRetryThreshold": 6
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.rate_limit_retry_threshold, Some(6));
    let value = serde_json::json!({
        "model": "grok-4.5",
        "context_window": 1_000_000,
        "rate_limit_retry_threshold": 7
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.rate_limit_retry_threshold, Some(7));
}
#[test]
fn parse_reads_model_family() {
    let value = serde_json::json!({
        "model": "grok-4.5",
        "context_window": 1_000_000,
        "model_family": "xai"
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.model_family.as_deref(), Some("xai"));
    let value = serde_json::json!({
        "model": "acme-1",
        "contextWindow": 400_000,
        "modelFamily": "acme"
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.model_family.as_deref(), Some("acme"));
    let value = serde_json::json!({"model": "x", "context_window": 256_000});
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.model_family.is_none());
}
#[test]
fn parse_reads_reasoning_effort_fields() {
    use xai_grok_sampling_types::ReasoningEffort;
    let value = serde_json::json!({
        "model": "grok-4.5",
        "context_window": 1_000_000,
        "supports_reasoning_effort": true,
        "reasoning_effort": "high"
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.supports_reasoning_effort);
    assert_eq!(result.reasoning_effort, Some(ReasoningEffort::High));
    let value = serde_json::json!({
        "model": "grok-4.5",
        "contextWindow": 1_000_000,
        "supportsReasoningEffort": true,
        "reasoningEffort": "xhigh"
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.supports_reasoning_effort);
    assert_eq!(result.reasoning_effort, Some(ReasoningEffort::Xhigh));
    let value = serde_json::json!({"model": "x", "context_window": 256_000});
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(!result.supports_reasoning_effort);
    assert!(result.reasoning_effort.is_none());
}
#[test]
fn parse_reads_reasoning_efforts_list() {
    use xai_grok_sampling_types::ReasoningEffort;
    let value = serde_json::json!({
        "model": "grok-4.5",
        "context_window": 1_000_000,
        "reasoning_efforts": [
            { "id": "deep", "value": "xhigh", "label": "Deep" },
            { "value": "quantum" },
            "low",
        ]
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let [deep, low] = result.reasoning_efforts.as_slice() else {
        panic!(
            "expected two reasoning efforts: {:?}",
            result.reasoning_efforts
        );
    };
    assert_eq!(deep.id, "deep");
    assert_eq!(deep.value, ReasoningEffort::Xhigh);
    assert_eq!(low.value, ReasoningEffort::Low);
    for value in [
        serde_json::json!({
            "model": "m", "context_window": 256_000,
            "reasoningEfforts": [{ "value": "high" }]
        }),
        serde_json::json!({
            "model": "m", "context_window": 256_000,
            "_meta": { "reasoningEfforts": [{ "value": "high" }] }
        }),
    ] {
        let result = parse_remote_model_value(&value, "https://default.url").unwrap();
        let [effort] = result.reasoning_efforts.as_slice() else {
            panic!(
                "expected one reasoning effort: {:?}",
                result.reasoning_efforts
            );
        };
        assert_eq!(effort.value, ReasoningEffort::High);
    }
    let value = serde_json::json!({"model": "x", "context_window": 256_000});
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.reasoning_efforts.is_empty());
}
/// A public `/v1/models` row carries the menu under `capabilities`; labels come from the shared `effort_label` table.
#[test]
fn parse_reads_reasoning_efforts_from_capabilities() {
    use xai_grok_sampling_types::{ReasoningEffort, ReasoningEffortOption};
    let option =
        |id: &str, value: ReasoningEffort, label: &str, default: bool| ReasoningEffortOption {
            id: id.to_string(),
            value,
            label: label.to_string(),
            description: None,
            default,
        };
    let value = serde_json::json!({
        "id": "grok-4.6",
        "object": "model",
        "owned_by": "xai",
        "capabilities": {
            "reasoning_effort": ["low", "medium", "high", "xhigh"],
            "default_reasoning_effort": "high"
        }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.reasoning_efforts,
        vec![
            option("low", ReasoningEffort::Low, "Low", false),
            option("medium", ReasoningEffort::Medium, "Medium", false),
            option("high", ReasoningEffort::High, "High", true),
            option("xhigh", ReasoningEffort::Xhigh, "X-High", false),
        ]
    );
    assert!(!result.reasoning_effort_server_default);
    let value = serde_json::json!({
        "id": "grok-4.6",
        "reasoning_efforts": ["low"],
        "capabilities": { "reasoning_effort": ["high"], "default_reasoning_effort": "high" }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.reasoning_efforts,
        vec![option("low", ReasoningEffort::Low, "Low", false)]
    );
    let value = serde_json::json!({
        "id": "grok-4.6",
        "reasoning_efforts": [{ "value": "quantum" }],
        "capabilities": { "reasoning_effort": ["high"], "default_reasoning_effort": "high" }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.reasoning_efforts,
        vec![option("high", ReasoningEffort::High, "High", true)]
    );
    let value = serde_json::json!({
        "id": "grok-4.6",
        "capabilities": { "reasoning_effort": ["low", "quantum", "high"] }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.reasoning_efforts,
        vec![
            option("low", ReasoningEffort::Low, "Low", false),
            option("high", ReasoningEffort::High, "High", false),
        ]
    );
    assert!(result.reasoning_effort_server_default);
    let value = serde_json::json!({
        "id": "grok-4.6",
        "capabilities": { "reasoning_effort": ["low", "high"], "default_reasoning_effort": "medium" }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.reasoning_efforts.iter().all(|o| !o.default));
    assert!(result.reasoning_effort_server_default);
}
#[test]
fn parse_reads_meta_fallback_fields() {
    let value = serde_json::json!({
        "_meta": {
            "model": "meta-model-id",
            "contextWindow": 131072,
            "agentType": "concise"
        }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.model, "meta-model-id");
    assert_eq!(
        result.context_window,
        std::num::NonZeroU64::new(131072).unwrap()
    );
    assert_eq!(result.agent_type, "concise");
}
#[test]
fn parse_remote_model_value_no_laziness_detector_block_yields_default() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.laziness_detector,
        crate::agent::config::LazinessDetectorPerModelConfig::default()
    );
}
#[test]
fn parse_remote_model_value_parses_camelcase_key() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "max_nudges_per_session": 2,
            "idle_threshold_ms": 12_000,
            "min_confidence": 0.75,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 2,
        idle_threshold_ms: Some(12_000),
        min_confidence: Some(0.75),
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
#[test]
fn parse_remote_model_value_parses_snake_case_laziness_detector() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "laziness_detector": {
            "enabled": true,
            "max_nudges_per_session": 3,
            "idle_threshold_ms": 8_000,
            "min_confidence": 0.6,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 3,
        idle_threshold_ms: Some(8_000),
        min_confidence: Some(0.6),
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
#[test]
fn parse_remote_model_value_parses_meta_laziness_detector() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "_meta": {
            "lazinessDetector": {
                "enabled": true,
                "max_nudges_per_session": 1,
                "idle_threshold_ms": 15_000,
                "min_confidence": 0.9,
            },
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 1,
        idle_threshold_ms: Some(15_000),
        min_confidence: Some(0.9),
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
#[test]
fn parse_remote_model_value_partial_block_uses_field_defaults() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 0,
        idle_threshold_ms: None,
        min_confidence: None,
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
#[test]
fn parse_remote_model_value_malformed_block_falls_back_to_default() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "max_nudges_per_session": "abc",
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.laziness_detector,
        crate::agent::config::LazinessDetectorPerModelConfig::default()
    );
}
#[test]
fn parse_remote_model_value_non_object_value_falls_back_to_default() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": "not-an-object",
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.laziness_detector,
        crate::agent::config::LazinessDetectorPerModelConfig::default()
    );
}
#[test]
fn parse_remote_model_value_top_level_camelcase_wins_over_snake_case() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "max_nudges_per_session": 7,
        },
        "laziness_detector": {
            "enabled": false,
            "max_nudges_per_session": 99,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 7,
        idle_threshold_ms: None,
        min_confidence: None,
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
/// `include_reasoning: false` parses under the camelCase `lazinessDetector` wrapper with a snake_case inner key.
/// That naming matches the sibling fields `min_confidence` and `idle_threshold_ms`.
#[test]
fn parse_remote_model_value_parses_include_reasoning_under_camelcase_wrapper() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "include_reasoning": false,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.laziness_detector.include_reasoning, Some(false));
}
#[test]
fn parse_remote_model_value_parses_include_reasoning_under_snake_case_wrapper() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "laziness_detector": {
            "enabled": true,
            "include_reasoning": true,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(result.laziness_detector.include_reasoning, Some(true));
}
#[test]
fn parse_remote_model_value_omitted_include_reasoning_defaults_to_none() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "max_nudges_per_session": 2,
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert_eq!(
        result.laziness_detector.include_reasoning, None,
        "absent include_reasoning defers to harness default via None",
    );
}
#[test]
fn parse_remote_model_value_top_level_wins_over_meta() {
    let value = serde_json::json!({
        "model": "grok-4",
        "context_window": 256_000,
        "lazinessDetector": {
            "enabled": true,
            "max_nudges_per_session": 5,
        },
        "_meta": {
            "lazinessDetector": {
                "enabled": false,
                "max_nudges_per_session": 99,
            },
        },
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    let expected = crate::agent::config::LazinessDetectorPerModelConfig {
        enabled: true,
        max_nudges_per_session: 5,
        idle_threshold_ms: None,
        min_confidence: None,
        include_reasoning: None,
    };
    assert_eq!(result.laziness_detector, expected);
}
#[test]
fn parse_reads_show_model_fingerprint_field() {
    let value = serde_json::json!({
        "model": "grok-build",
        "context_window": 256_000,
        "show_model_fingerprint": true
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.show_model_fingerprint);
    let value = serde_json::json!({
        "model": "grok-build",
        "contextWindow": 256_000,
        "showModelFingerprint": true
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.show_model_fingerprint);
    let value = serde_json::json!({
        "model": "grok-build",
        "context_window": 256_000,
        "_meta": { "showModelFingerprint": true }
    });
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(result.show_model_fingerprint);
    let value = serde_json::json!({"model": "x", "context_window": 256_000});
    let result = parse_remote_model_value(&value, "https://default.url").unwrap();
    assert!(!result.show_model_fingerprint);
}
#[test]
fn get_object_returns_none_for_non_object_values() {
    let value = serde_json::json!({
        "string": "hello",
        "number": 42,
        "bool": true,
        "array": [1, 2, 3],
        "null": null,
    });
    let obj = value.as_object().unwrap();
    assert!(get_object(obj, "string").is_none());
    assert!(get_object(obj, "number").is_none());
    assert!(get_object(obj, "bool").is_none());
    assert!(get_object(obj, "array").is_none());
    assert!(get_object(obj, "null").is_none());
    assert!(get_object(obj, "missing").is_none());
}
#[test]
fn get_object_returns_some_for_actual_object() {
    let value = serde_json::json!({
        "nested": { "a": 1, "b": "two" },
    });
    let obj = value.as_object().unwrap();
    let nested = get_object(obj, "nested").expect("nested key should resolve to object");
    assert!(nested.is_object());
    assert_eq!(
        nested.pointer("/a").unwrap_or(&serde_json::Value::Null),
        &serde_json::json!(1)
    );
    assert_eq!(
        nested.pointer("/b").unwrap_or(&serde_json::Value::Null),
        &serde_json::json!("two")
    );
}
fn endpoints(
    proxy: &str,
    models_base_url: Option<&str>,
    models_list_url: Option<&str>,
) -> crate::agent::config::EndpointsConfig {
    crate::agent::config::EndpointsConfig {
        cli_chat_proxy_base_url: Some(proxy.to_owned()),
        models_base_url: models_base_url.map(|s| s.to_owned()),
        models_list_url: models_list_url.map(|s| s.to_owned()),
        ..Default::default()
    }
}
#[test]
fn inference_url_defaults_to_proxy() {
    let ep = endpoints("https://proxy.grok.com/v1", None, None);
    assert_eq!(ep.resolve_inference_base_url(), "https://proxy.grok.com/v1");
}
#[test]
fn inference_url_uses_models_base_url() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://enterprise.acme.com/v1"),
        None,
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://enterprise.acme.com/v1"
    );
}
#[test]
fn inference_url_base_url_wins_over_proxy() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/models"),
    );
    assert_eq!(
        ep.resolve_inference_base_url(),
        "https://inference.acme.com/v1"
    );
}
#[test]
fn list_url_defaults_to_proxy_models() {
    let ep = endpoints("https://proxy.grok.com/v1", None, None);
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://proxy.grok.com/v1/models"
    );
}
#[test]
fn list_url_derived_from_base_url() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://api.x.ai/v1"),
        None,
    );
    assert_eq!(ep.resolve_models_list_url(), "https://api.x.ai/v1/models");
}
#[test]
fn list_url_explicit_overrides_derivation() {
    let ep = endpoints(
        "https://proxy.grok.com/v1",
        Some("https://inference.acme.com/v1"),
        Some("https://registry.acme.com/api/list-models"),
    );
    assert_eq!(
        ep.resolve_models_list_url(),
        "https://registry.acme.com/api/list-models"
    );
}
/// REGRESSION: `grok setup` must send the deployment key to the proxy, never the inference endpoint.
#[test]
#[serial_test::serial]
fn deployment_config_url_uses_cli_chat_proxy_when_not_overridden() {
    use crate::agent::config::EndpointsConfig;
    for k in [
        "GROK_CLI_CHAT_PROXY_BASE_URL",
        "GROK_MANAGED_CONFIG_URL",
        "GROK_XAI_API_BASE_URL",
    ] {
        unsafe { std::env::remove_var(k) };
    }
    unsafe { std::env::set_var("GROK_DEPLOYMENT_KEY", "xai-token-ENTERPRISE") };
    let managed: toml::Value = toml::from_str(
        r#"[endpoints]
            deployment_key = "xai-token-ENTERPRISE"
            xai_api_base_url = "https://inference.acme-corp.example/xai/v1""#,
    )
    .unwrap();
    let url = EndpointsConfig::from_config_value(&managed).resolve_managed_config_url();
    assert_eq!(url, "", "no proxy configured, so the key goes nowhere");
    assert!(
        !url.contains("acme-corp"),
        "deployment key would be sent to the inference host: {url}"
    );
    let pinned: toml::Value = toml::from_str(
        r#"[endpoints]
            xai_api_base_url = "https://inference.acme-corp.example/xai/v1"
            cli_chat_proxy_base_url = "https://proxy.acme-corp.example/v1""#,
    )
    .unwrap();
    assert_eq!(
        EndpointsConfig::from_config_value(&pinned).resolve_managed_config_url(),
        "https://proxy.acme-corp.example/v1/deployment/config"
    );
    unsafe { std::env::remove_var("GROK_DEPLOYMENT_KEY") };
}
#[derive(Clone)]
struct DualBundleServerState {
    archive_status: StatusCode,
    archive_bytes: Vec<u8>,
    legacy_status: StatusCode,
    legacy_body: serde_json::Value,
}
async fn start_dual_bundle_server(
    state: DualBundleServerState,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route(
            "/v1/bundle/archive",
            get(|State(state): State<DualBundleServerState>| async move {
                (state.archive_status, state.archive_bytes)
            }),
        )
        .route(
            "/v1/subagents/bundle",
            get(|State(state): State<DualBundleServerState>| async move {
                (state.legacy_status, axum::Json(state.legacy_body))
            }),
        )
        .with_state(state);
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("{base}/v1"), handle)
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_bundle_returns_archive_on_success() {
    let archive_bytes = b"fake-tar-gz-bytes".to_vec();
    let (proxy_base_url, server) = start_dual_bundle_server(DualBundleServerState {
        archive_status: StatusCode::OK,
        archive_bytes: archive_bytes.clone(),
        legacy_status: StatusCode::OK,
        legacy_body: serde_json::json!({
            "version": "v1", "personas": {}, "roles": {}, "agents": {}
        }),
    })
    .await;
    let am = test_auth_manager();
    let result = fetch_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap();
    match result {
        FetchedBundle::Archive(bytes) => assert_eq!(bytes, archive_bytes),
        FetchedBundle::Legacy(_) => panic!("expected Archive variant"),
    }
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_bundle_falls_back_on_archive_404() {
    let (proxy_base_url, server) = start_dual_bundle_server(DualBundleServerState {
        archive_status: StatusCode::NOT_FOUND,
        archive_bytes: Vec::new(),
        legacy_status: StatusCode::OK,
        legacy_body: serde_json::json!({
            "version": "v1",
            "personas": {"r": "p"},
            "roles": {},
            "agents": {}
        }),
    })
    .await;
    let am = test_auth_manager();
    let result = fetch_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap();
    match result {
        FetchedBundle::Legacy(bundle) => {
            assert_eq!(bundle.version, "v1");
            assert_eq!(bundle.personas.get("r"), Some(&"p".to_string()));
        }
        FetchedBundle::Archive(_) => panic!("expected Legacy variant"),
    }
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_bundle_falls_back_on_archive_503() {
    let (proxy_base_url, server) = start_dual_bundle_server(DualBundleServerState {
        archive_status: StatusCode::SERVICE_UNAVAILABLE,
        archive_bytes: Vec::new(),
        legacy_status: StatusCode::OK,
        legacy_body: serde_json::json!({
            "version": "v1", "personas": {}, "roles": {}, "agents": {}
        }),
    })
    .await;
    let am = test_auth_manager();
    let result = fetch_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap();
    match &result {
        FetchedBundle::Legacy(bundle) => assert_eq!(bundle.version, "v1"),
        FetchedBundle::Archive(_) => panic!("expected Legacy variant"),
    }
    server.abort();
}
/// `BackendClient::save_session_data` resolves auth from the attached `AuthManager` and sends the token as `Bearer <key>` on the wire.
/// This is the writeback path used on every session flush.
#[tokio::test(flavor = "current_thread")]
async fn backend_client_resolves_auth_from_auth_manager() {
    let captured_auth = Arc::new(Mutex::new(None::<String>));
    let captured = captured_auth.clone();
    let app = Router::new().route(
        "/sessions/{id}/data",
        axum::routing::post(move |headers: HeaderMap| async move {
            *captured.lock().unwrap() = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            StatusCode::OK
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let am = test_auth_manager();
    let client = BackendClient::with_base_url(format!("http://{addr}")).with_auth_manager(am);
    client
        .save_session_data("test-session", &[], None)
        .await
        .unwrap();
    let sent = captured_auth
        .lock()
        .unwrap()
        .clone()
        .expect("server must receive Authorization header");
    assert_eq!(sent, "Bearer token", "must use token from AuthManager");
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn fetch_bundle_propagates_legacy_error_after_fallback() {
    let (proxy_base_url, server) = start_dual_bundle_server(DualBundleServerState {
        archive_status: StatusCode::NOT_FOUND,
        archive_bytes: Vec::new(),
        legacy_status: StatusCode::UNAUTHORIZED,
        legacy_body: serde_json::json!({"error": "unauthorized"}),
    })
    .await;
    let am = test_auth_manager();
    let error = fetch_bundle(&proxy_base_url, Some(&am), None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        BackendError::RequestFailed { status: 401, .. }
    ));
    server.abort();
}
/// Regression: reqwest .header() appends, so duplicate or overlapping headers cause Cloudflare to reject the request.
#[tokio::test(flavor = "current_thread")]
#[allow(clippy::disallowed_methods)]
async fn auth_headers_do_not_collide_with_json() {
    let client =
        BackendClient::with_base_url("http://localhost").with_auth_manager(test_auth_manager());
    let auth_headers = client.auth_header_map().await.unwrap();
    assert!(
        !auth_headers.contains_key("content-type"),
        "content-type in auth map would overwrite .json()"
    );
    let request = reqwest::Client::new()
        .put("http://localhost/sessions/test")
        .json(&serde_json::json!({"test": true}))
        .headers(auth_headers)
        .build()
        .unwrap();
    for name in request.headers().keys() {
        let count = request.headers().get_all(name).iter().count();
        assert_eq!(count, 1, "duplicate header {name}");
    }
}
#[test]
fn parse_anthropic_style_listing_max_input_tokens_resolves_context_window() {
    // Anthropic's official `/v1/models` ModelInfo shape (`id`, `created_at`,
    // `display_name`, `type`) carries the input-context-window as
    // `max_input_tokens`, not `contextWindow`/`context_window`. This drives
    // the real shipped parse path and asserts the value lands in `context_window`.
    let value = serde_json::json!({
        "id": "claude-sonnet-4-5",
        "type": "model",
        "display_name": "Claude Sonnet 4.5",
        "created_at": "2025-07-14T00:00:00Z",
        "max_input_tokens": 200_000
    });
    let result = parse_remote_model_value(&value, "https://api.anthropic.com").unwrap();
    assert_eq!(result.model, "claude-sonnet-4-5");
    assert_eq!(result.context_window.get(), 200_000);

    // camelCase spelling is also accepted.
    let value = serde_json::json!({ "id": "claude-opus-4-6", "maxInputTokens": 1_000_000 });
    let result = parse_remote_model_value(&value, "https://api.anthropic.com").unwrap();
    assert_eq!(result.context_window.get(), 1_000_000);

    // An OpenAI-style field still wins when present alongside Anthropic's.
    let value = serde_json::json!({
        "id": "claude-sonnet-4-5",
        "max_input_tokens": 200_000,
        "contextWindow": 350_000
    });
    let result = parse_remote_model_value(&value, "https://api.anthropic.com").unwrap();
    assert_eq!(result.context_window.get(), 350_000);

    // A `0` placeholder (docs example) does not drop the entry; it falls back.
    let value = serde_json::json!({ "id": "claude-3-5-haiku", "max_input_tokens": 0 });
    let result = parse_remote_model_value(&value, "https://api.anthropic.com").unwrap();
    assert_eq!(
        result.context_window.get(),
        crate::remote::DEFAULT_CONTEXT_WINDOW
    );
}
#[test]
fn parse_openrouter_style_listing_context_length_resolves_context_window() {
    // OpenRouter's `/models` endpoint exposes the per-model context window
    // as `context_length`, NOT `contextWindow`/`context_window`/`max_input_tokens`.
    // This drives the real shipped parse path and asserts the value lands in
    // `context_window` (not `DEFAULT_CONTEXT_WINDOW`).
    let value = serde_json::json!({
        "id": "deepseek/deepseek-v4-pro-0813",
        "object": "model",
        "created": 1750000000,
        "owned_by": "deepseek",
        "context_length": 1_000_000,
        "architecture": { "modality": "text+image->text", "tokenizer": "DeepSeek",
                          "instruct_type": null },
        "top_provider": {
            "context_length": 1_000_000,
            "max_completion_tokens": 64_000,
            "is_moderated": false
        }
    });
    let result = parse_remote_model_value(&value, "https://openrouter.ai/api/v1").unwrap();
    assert_eq!(result.model, "deepseek/deepseek-v4-pro-0813");
    assert_eq!(result.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(result.context_window.get(), 1_000_000);
}
#[test]
fn parse_openrouter_context_length_only_under_top_provider_resolves() {
    // Some OpenRouter listings carry `context_length` only under
    // `top_provider`, with no top-level window field at all. The parser must
    // still resolve the real window rather than the 256k default.
    let value = serde_json::json!({
        "id": "x-ai/grok-4.6",
        "object": "model",
        "created": 1750000000,
        "owned_by": "x-ai",
        "top_provider": { "context_length": 1_048_576, "max_completion_tokens": 65_536 }
    });
    let result = parse_remote_model_value(&value, "https://openrouter.ai/api/v1").unwrap();
    assert_eq!(result.model, "x-ai/grok-4.6");
    assert_eq!(result.context_window.get(), 1_048_576);
}
#[test]
fn parse_openrouter_context_length_zero_falls_back_to_default() {
    // A `context_length: 0` placeholder must not drop the entry; it falls
    // back to the documented default, mirroring the existing `0` handling
    // for `max_input_tokens`.
    let value = serde_json::json!({
        "id": "deepseek/deepseek-v4-flash-0731",
        "context_length": 0
    });
    let result = parse_remote_model_value(&value, "https://openrouter.ai/api/v1").unwrap();
    assert_eq!(
        result.context_window.get(),
        crate::remote::DEFAULT_CONTEXT_WINDOW
    );
}
#[test]
fn parse_openai_style_field_wins_over_openrouter_context_length() {
    // Consistency with the existing precedence rules: an explicit
    // OpenAI-style `context_window` wins when present alongside
    // `context_length`.
    let value = serde_json::json!({
        "id": "deepseek/deepseek-v4-pro-0813",
        "context_window": 350_000,
        "context_length": 1_000_000
    });
    let result = parse_remote_model_value(&value, "https://openrouter.ai/api/v1").unwrap();
    assert_eq!(result.context_window.get(), 350_000);
}
#[test]
fn models_list_url_for_openai_base() {
    // OpenAI-style base → its `/v1/models` listing.
    assert_eq!(
        models_list_url_for_base("https://api.openai.com/v1"),
        "https://api.openai.com/v1/models"
    );
    // An OpenAI-compatible gateway base of the same shape.
    assert_eq!(
        models_list_url_for_base("https://gateway.example.com/v1"),
        "https://gateway.example.com/v1/models"
    );
}
#[test]
fn models_list_url_for_anthropic_base() {
    // Anthropic serves its model list at the OpenAI-compatible
    // `{base}/models` URL — no Anthropic-specific divergent path needed.
    assert_eq!(
        models_list_url_for_base("https://api.anthropic.com/v1"),
        "https://api.anthropic.com/v1/models"
    );
}
#[test]
fn models_list_url_for_base_idempotent_for_full_list_url() {
    assert_eq!(
        models_list_url_for_base("https://api.anthropic.com/v1/models"),
        "https://api.anthropic.com/v1/models"
    );
}
#[test]
fn resolve_models_list_url_covers_openai_and_anthropic_bases() {
    use crate::agent::config::EndpointsConfig;
    // OpenAI-style base derived from models_base_url.
    let openai = EndpointsConfig {
        models_base_url: Some("https://api.openai.com/v1".to_owned()),
        ..EndpointsConfig::default()
    };
    assert_eq!(
        openai.resolve_models_list_url(),
        "https://api.openai.com/v1/models"
    );
    // Anthropic base derived from models_base_url.
    let anthropic = EndpointsConfig {
        models_base_url: Some("https://api.anthropic.com/v1".to_owned()),
        ..EndpointsConfig::default()
    };
    assert_eq!(
        anthropic.resolve_models_list_url(),
        "https://api.anthropic.com/v1/models"
    );
}

/// Drive `fetch_models_for_api_base_blocking` (the BYOK /v1/models fetch
/// primitive) against a loopback OpenAI-compatible listing. Proves the
/// shipped fetch parses per-model `context_window` and matches the routing
/// slug, and that a `0`/missing window falls back to the sentinel.
async fn start_models_listing_server(
    body: serde_json::Value,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::routing::get;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = axum::Router::new().route("/v1/models", get(move || async move { axum::Json(body) }));
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, handle)
}
/// The blocking /v1/models fetch parses per-model `context_window` and the
/// Anthropic `max_input_tokens` form, matching the routing slug.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_models_for_api_base_blocking_parses_per_model_windows() {
    let (base, server) = start_models_listing_server(serde_json::json!({
        "data": [
            {
                "model": "openrouter/deepseek/deepseek-test",
                "contextWindow": 1_000_000,
            },
            {
                "model": "vendor-2",
                "max_input_tokens": 700_000,
            },
        ]
    }))
    .await;
    let mock_url = format!("{base}/v1");

    let entries = tokio::task::spawn_blocking(move || {
        fetch_models_for_api_base_blocking(&mock_url, Some("test-key"))
            .expect("mock /v1/models should be reachable")
    })
    .await
    .unwrap();
    server.abort();

    let deepseek = entries
        .iter()
        .find(|m| m.model == "openrouter/deepseek/deepseek-test")
        .unwrap();
    assert_eq!(
        deepseek.context_window.get(),
        1_000_000,
        "per-model contextWindow must parse from the model's own provider listing"
    );
    let vendor2 = entries.iter().find(|m| m.model == "vendor-2").unwrap();
    assert_eq!(
        vendor2.context_window.get(),
        700_000,
        "Anthropic max_input_tokens form must parse too"
    );
}

/// Drive `resolve_context_window_from_provider` (the per-request own-provider
/// resolution primitive) against a loopback OpenRouter-style `/v1/models`
/// listing whose entries expose only `context_length`. Proves the shipped
/// resolver fetches from the model's OWN base URL and returns the exact
/// slug's `context_length` — not `DEFAULT_CONTEXT_WINDOW`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_context_window_from_provider_reads_openrouter_context_length() {
    let (base, server) = start_models_listing_server(serde_json::json!({
        "data": [
            {
                "id": "deepseek/deepseek-v4-pro-0813",
                "object": "model",
                "created": 1750000000,
                "owned_by": "deepseek",
                "context_length": 1_000_000,
                "top_provider": { "context_length": 1_000_000, "max_completion_tokens": 64_000 }
            },
            {
                "id": "x-ai/grok-4.6",
                "object": "model",
                "context_length": 1_048_576
            },
            {
                "id": "moonshotai/kimi-k3",
                "object": "model",
                "top_provider": { "context_length": 128_000 }
            }
        ]
    }))
    .await;
    let api_base = format!("{base}/v1");

    // Run the shipped resolver exactly as the request path does: it spawns a
    // dedicated OS thread and does a blocking reqwest fetch against the model's
    // own base, then matches the requested slug.
    let resolved = {
        let api_base = api_base.clone();
        tokio::task::spawn_blocking(move || {
            crate::agent::remote_config::resolve_context_window_from_provider(
                "deepseek/deepseek-v4-pro-0813",
                &api_base,
                Some("sk-or-v1-test"),
            )
        })
        .await
        .unwrap()
    };
    server.abort();

    let cw = resolved.expect("own-provider listing must resolve the slug");
    assert_eq!(
        cw.get(),
        1_000_000,
        "resolve_context_window_from_provider must return the OpenRouter context_length"
    );
}

/// The per-request resolver returns `None` when the model's own listing
/// does not carry the requested slug, matching the cold/unlisted fallback
/// contract (best-effort, sentinel kept by the caller).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_context_window_from_provider_absent_slug_returns_none() {
    let (base, server) = start_models_listing_server(serde_json::json!({
        "data": [ { "id": "some/other-model", "context_length": 500_000 } ]
    }))
    .await;
    let api_base = format!("{base}/v1");
    let resolved = {
        let api_base = api_base.clone();
        tokio::task::spawn_blocking(move || {
            crate::agent::remote_config::resolve_context_window_from_provider(
                "deepseek/deepseek-v4-pro-0813",
                &api_base,
                Some("sk-or-v1-test"),
            )
        })
        .await
        .unwrap()
    };
    server.abort();
    assert!(resolved.is_none());
}

/// A listing server that counts its requests and answers each with `status`.
async fn start_counting_listing_server(
    status: axum::http::StatusCode,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use axum::routing::get;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/v1/models",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let body = serde_json::json!({"data": [
                    {"id": "vendor/a", "context_length": 400_000},
                    {"id": "vendor/b", "context_length": 500_000},
                ]});
                (status, axum::Json(body))
            }
        }),
    );
    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, hits, handle)
}

/// Resolve every model in `models` against a single provider, from blocking threads.
async fn resolve_all(api_base: &str, models: &[&str]) -> Vec<Option<std::num::NonZeroU64>> {
    let mut out = Vec::new();
    for model in models {
        let (api_base, model) = (api_base.to_owned(), (*model).to_owned());
        out.push(
            tokio::task::spawn_blocking(move || {
                crate::agent::remote_config::resolve_context_window_from_provider(
                    &model,
                    &api_base,
                    Some("sk-test"),
                )
            })
            .await
            .unwrap(),
        );
    }
    out
}

/// A catalog build resolves every model a provider serves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_catalog_build_sends_one_listing_request_per_provider() {
    use std::sync::atomic::Ordering;
    let (base, hits, server) = start_counting_listing_server(axum::http::StatusCode::OK).await;
    let resolved = resolve_all(&format!("{base}/v1"), &["vendor/a", "vendor/b", "vendor/c"]).await;
    server.abort();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "each model re-fetched the listing"
    );
    assert_eq!(resolved[0].map(|w| w.get()), Some(400_000));
    assert_eq!(resolved[1].map(|w| w.get()), Some(500_000));
    assert_eq!(resolved[2], None, "an unlisted model keeps the default");
}

/// A rate-limited provider must not be asked again for each model. The
/// failed answer is reused for the same window.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rate_limited_listing_is_not_retried_for_every_model() {
    use std::sync::atomic::Ordering;
    let (base, hits, server) =
        start_counting_listing_server(axum::http::StatusCode::TOO_MANY_REQUESTS).await;
    let resolved = resolve_all(
        &format!("{base}/v1"),
        &["vendor/a", "vendor/b", "vendor/c", "vendor/d"],
    )
    .await;
    server.abort();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a 429 was answered with a burst"
    );
    assert!(resolved.iter().all(Option::is_none));
}
