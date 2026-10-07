//! Runs the built binary against one `[model_providers.<id>]` setup: the
//! provider lists its models, and `[model.<id>]` blocks tune some of them.
//! Every prompt must reach the provider's URL and nothing else.
//!
//! Not `#[ignore]`d: CI builds the binary before the workspace run, so this
//! runs on every push.

use xai_grok_test_support::mock_server::LogEntry;
use xai_grok_test_support::*;

const PROVIDER_KEY: &str = "internal-provider-key";

/// The provider lists these. `slug-a` and `slug-b` also have a config block.
fn listed_models() -> Vec<MockModelEntry> {
    ["slug-a", "slug-b", "slug-c", "slug-d"]
        .into_iter()
        .map(MockModelEntry::new)
        .collect()
}

fn write_config(sandbox: &TestSandbox, provider_url: &str, default: Option<&str>) {
    let default = default
        .map(|d| format!("[models]\ndefault = \"{d}\"\n\n"))
        .unwrap_or_default();
    std::fs::write(
        sandbox.grok_home().join("config.toml"),
        format!(
            r#"{default}[model_providers.internal]
base_url = "{provider_url}"
api_key = "{PROVIDER_KEY}"

[model.my-model]
model = "slug-a"
model_provider = "internal"
context_window = 200000

[model.tuned-b]
model = "slug-b"
model_provider = "internal"
temperature = 0.2
"#
        ),
    )
    .expect("write config.toml");
}

/// Adds `authority` to the endpoint allowlist the sandbox already carries.
fn allow(sandbox: &mut TestSandbox, url: &str) {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let current = sandbox
        .env()
        .into_iter()
        .find(|(k, _)| k == "GROK_ALLOWED_ENDPOINTS")
        .map(|(_, v)| v.to_string_lossy().into_owned());
    let allowed = match current {
        Some(existing) if !existing.is_empty() => format!("{existing},{authority}"),
        _ => authority,
    };
    sandbox.set_env("GROK_ALLOWED_ENDPOINTS", allowed);
}

async fn run_prompt(sandbox: &TestSandbox, model: Option<&str>) -> HeadlessResult {
    let mut cmd = tokio::process::Command::new(grok_binary());
    cmd.args(["-p", "say hi", "--yolo", "--max-turns", "1"]);
    if let Some(model) = model {
        cmd.args(["--model", model]);
    }
    cmd.args(["--output-format", "json"])
        .arg("--cwd")
        .arg(sandbox.workspace())
        .current_dir(sandbox.workspace())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    run_headless_in_sandbox_borrowed(cmd, sandbox).await
}

fn is_inference(e: &LogEntry) -> bool {
    e.method == "POST"
        && ["chat/completions", "/responses", "/messages"]
            .iter()
            .any(|p| e.path.contains(p))
}

/// The slugs the provider received, in order, from the request bodies.
fn inference_slugs(server: &MockInferenceServer) -> Vec<String> {
    server
        .request_bodies()
        .iter()
        .filter_map(|b| b.get("model").and_then(|m| m.as_str()).map(str::to_owned))
        .collect()
}

/// The first-party URLs point at a trap server, the way the old default
/// pointed them at cli-chat-proxy. A model the user took from the provider
/// must never send its prompt there.
#[tokio::test]
async fn provider_listed_and_configured_models_send_only_to_the_provider() {
    let provider = MockInferenceServer::start_with_models(listed_models())
        .await
        .expect("start provider mock");
    let trap = MockInferenceServer::start().await.expect("start trap mock");

    // (model flag, slug the provider must receive)
    let cases = [
        (None, "slug-c"),
        (Some("my-model"), "slug-a"),
        (Some("tuned-b"), "slug-b"),
        (Some("internal/slug-d"), "slug-d"),
    ];
    for (model, slug) in cases {
        let mut sandbox = TestSandbox::builder().git().build();
        sandbox.set_mock_url(trap.url());
        allow(&mut sandbox, &provider.url());
        write_config(&sandbox, &provider.url(), Some("internal/slug-c"));

        let before = inference_slugs(&provider).len();
        let result = run_prompt(&sandbox, model).await;
        assert_headless_success(&result, &format!("model {model:?}"), Some(&provider));

        let sent = inference_slugs(&provider);
        assert!(
            sent[before..].iter().any(|s| s == slug),
            "model {model:?} must send slug {slug} to the provider; provider requests:\n{}",
            provider.request_log_summary()
        );
        let leaked: Vec<_> = trap.requests().into_iter().filter(is_inference).collect();
        assert!(
            leaked.is_empty(),
            "model {model:?} sent a prompt to the first-party URL; trap requests:\n{}\nleaked bodies:\n{:#?}",
            trap.request_log_summary(),
            leaked.iter().map(|e| &e.body).collect::<Vec<_>>()
        );
        for stream in [&result.stdout, &result.stderr] {
            assert!(
                !stream.contains("cli-chat-proxy"),
                "model {model:?} named cli-chat-proxy:\n{stream}"
            );
        }
    }
}

/// No first-party URL is set and no default is named. The session must start
/// on a provider model, because the built-in models have nowhere to go.
#[tokio::test]
async fn with_no_first_party_url_the_default_is_a_provider_model() {
    let provider = MockInferenceServer::start_with_models(listed_models())
        .await
        .expect("start provider mock");
    let mut sandbox = TestSandbox::builder().git().build();
    allow(&mut sandbox, &provider.url());
    write_config(&sandbox, &provider.url(), None);

    let result = run_prompt(&sandbox, None).await;
    assert_headless_success(&result, "no first-party url", Some(&provider));

    let sent = inference_slugs(&provider);
    assert!(
        sent.iter()
            .any(|s| ["slug-a", "slug-b", "slug-c", "slug-d"].contains(&s.as_str())),
        "the prompt must reach the provider; provider requests:\n{}",
        provider.request_log_summary()
    );
    for stream in [&result.stdout, &result.stderr] {
        assert!(
            !stream.contains("cli-chat-proxy") && !stream.contains("not in the catalog"),
            "startup picked a model that cannot answer:\n{stream}"
        );
    }
}
