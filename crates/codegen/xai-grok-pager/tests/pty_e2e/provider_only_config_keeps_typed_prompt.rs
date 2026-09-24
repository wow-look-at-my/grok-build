// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::EnvOp;

/// The earliest-party URLs and the API key the harness baseline sets. A user
/// who runs only a `[model_providers.*]` block has none of them.
const FIRST_PARTY_ENV: &[&str] = &[
    "GROK_CLI_CHAT_PROXY_BASE_URL",
    "GROK_XAI_API_BASE_URL",
    "GROK_MODELS_BASE_URL",
    "GROK_FEEDBACK_BASE_URL",
    "GROK_TRACE_UPLOAD_URL",
    "GROK_MANAGED_CONFIG_URL",
    "GROK_CODE_WEB_URL",
    "GROK_CONVERSATIONS_BASE_URL",
    "XAI_API_KEY",
];

fn write_provider_config(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.internal]\nbase_url = \"{}\"\napi_key = \"internal-key\"\n\n\
             [model.my-model]\nmodel = \"test-model\"\nmodel_provider = \"internal\"\n",
            content.url()
        ),
    )
    .expect("write config.toml");
}

fn spawn_provider_only(content: &ContentController, extra: &[EnvOp<'_>]) -> PtyHarness {
    write_provider_config(content);
    let mut ops: Vec<EnvOp<'_>> = FIRST_PARTY_ENV.iter().map(|k| EnvOp::remove(k)).collect();
    ops.extend_from_slice(extra);
    let binary = pager_binary().expect("resolve pager binary");
    PtyHarness::spawn_with_content_env_ops(&binary, DEFAULT_ROWS, DEFAULT_COLS, content, &[], &ops)
        .expect("spawn pager")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn provider_only_config_answers_a_prompt() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} provider answered."));
    let mut harness = spawn_provider_only(&content, &[]);
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    harness.inject_keys(b"hello provider").expect("type");
    harness.update(Duration::from_secs(2));
    assert!(
        harness.screen_contents().contains("hello provider"),
        "typed text vanished from the composer:\n{}",
        harness.screen_contents()
    );
    harness.inject_keys(b"\r").expect("submit");
    if harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .is_err()
    {
        panic!(
            "the provider's answer never reached the screen:\n{}\nrequests:\n{:?}",
            harness.screen_contents(),
            content
                .requests()
                .iter()
                .map(|e| &e.path)
                .collect::<Vec<_>>()
        );
    }
    harness.quit().expect("clean quit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn provider_outside_the_allowlist_keeps_the_prompt_and_says_why() {
    let content = ContentController::start().await.expect("start content");
    let mut harness = spawn_provider_only(
        &content,
        &[EnvOp::set("GROK_ALLOWED_ENDPOINTS", "allowed.invalid")],
    );
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    harness.inject_keys(b"keep this text").expect("type");
    harness.update(Duration::from_secs(3));
    harness.inject_keys(b"\r").expect("submit");
    harness.update(Duration::from_secs(5));
    let screen = harness.screen_contents();
    assert!(
        screen.contains("keep this text"),
        "the typed prompt was thrown away:\n{screen}"
    );
    assert!(
        screen.contains("allowed_endpoints"),
        "the screen does not say how to allow the endpoint:\n{screen}"
    );
    assert!(
        !content.requests().iter().any(|e| e.method == "POST"),
        "a request reached an endpoint that is not allowed"
    );
    harness.quit().expect("clean quit");
}
