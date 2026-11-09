//! The built binary talks to a real `ai-proxy` relay in front of a mock
//! provider, once per upstream dialect.

use std::path::PathBuf;
use std::time::Duration;

use xai_grok_test_support::recording_proxy::{RecordedRequest, RecordingProxy};
use xai_grok_test_support::*;

const PROVIDER_KEY: &str = "relay-test-key";
const MODEL: &str = "relay-model";
const DONE: &str = "RELAY_ROUND_DONE";
const FIRST_MARKER: &str = "FIRST_FILE_MARKER";
const SECOND_MARKER: &str = "SECOND_FILE_MARKER";
/// The sampler compresses no body smaller than this.
const COMPRESSION_FLOOR: usize = 64 * 1024;

/// `ai-proxy` from `$AI_PROXY`, else from `PATH`. CI installs it with the org tools.
fn ai_proxy_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("AI_PROXY") {
        return PathBuf::from(path);
    }
    std::env::var_os("PATH")
		.and_then(|paths| {
			std::env::split_paths(&paths)
				.map(|dir| dir.join("ai-proxy"))
				.find(|candidate| candidate.is_file())
		})
		.expect("ai-proxy is not on PATH and $AI_PROXY is unset; install it from dl.pazer.build/ai-proxy")
}

/// A running relay. Dropping it kills the process.
struct Relay {
    origin: String,
    _child: tokio::process::Child,
}

async fn start_relay(upstream: &str) -> Relay {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("pick a free port")
        .port();
    let listen = format!("127.0.0.1:{port}");
    let child = tokio::process::Command::new(ai_proxy_binary())
        .args(["serve", "--listen", &listen, "--route"])
        .arg(format!("up={upstream}"))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ai-proxy");
    let deadline = tokio::time::Instant::now() + scaled(Duration::from_secs(10));
    while tokio::net::TcpStream::connect(&listen).await.is_err() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "ai-proxy never listened on {listen}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Relay {
        origin: format!("http://{listen}"),
        _child: child,
    }
}

/// Text zstd cannot shrink much by itself, so only a delta makes a repeat of it small.
fn incompressible_text(len: usize) -> String {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(
                b"abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+/"
                    [(state % 64) as usize],
            )
        })
        .collect()
}

fn add_allowed(sandbox: &mut TestSandbox, origin: &str) {
    let authority = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
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

fn is_inference(path: &str) -> bool {
    path.ends_with("/messages") || path.ends_with("/chat/completions")
}

async fn run_round_through_relay(api_backend: &str, upstream_path: &str) {
    let provider = MockInferenceServer::start_with_models(vec![
        MockModelEntry::new(MODEL).with_api_backend(api_backend),
    ])
    .await
    .expect("start provider mock");
    provider.set_keep_requests(true);
    let read = |file: &str, marker: &str| {
        MockToolCall::new(Tool::Read, serde_json::json!({ "target_file": file }))
            .result_contains(marker)
    };
    provider.set_conversations([Conversation::nth(1)
        .calls([
            read("first.txt", FIRST_MARKER),
            read("second.txt", SECOND_MARKER),
        ])
        .reply(DONE)]);
    provider.set_response(DONE);

    let relay = start_relay(&provider.origin()).await;
    let tap = RecordingProxy::start(&relay.origin)
        .await
        .expect("start tap");

    let mut sandbox = TestSandbox::builder().git().build();
    add_allowed(&mut sandbox, &tap.origin());
    std::fs::write(sandbox.workspace().join("first.txt"), FIRST_MARKER).expect("write first.txt");
    std::fs::write(sandbox.workspace().join("second.txt"), SECOND_MARKER)
        .expect("write second.txt");
    std::fs::write(
        sandbox.grok_home().join("config.toml"),
        format!(
            r#"[models]
default = "relay/{MODEL}"

[model_providers.relay]
base_url = "{tap}/up/v1"
api_key = "{PROVIDER_KEY}"
"#,
            tap = tap.origin()
        ),
    )
    .expect("write config.toml");

    // Every request carries this prompt, so each is past the compression floor.
    let prompt = format!(
        "Read first.txt, then second.txt. Reference text follows.\n{}",
        incompressible_text(96 * 1024)
    );
    let mut cmd = tokio::process::Command::new(grok_binary());
    cmd.arg("-p")
        .arg(&prompt)
        .args([
            "--yolo",
            "--max-turns",
            "10",
            "--output-format",
            "json",
            "--cwd",
        ])
        .arg(sandbox.workspace())
        .current_dir(sandbox.workspace())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let result = run_headless_in_sandbox_borrowed(cmd, &sandbox).await;
    let label = format!("{api_backend} through ai-proxy");
    assert_headless_success(&result, &label, Some(&provider));
    assert!(
        result.stdout.contains(DONE),
        "{label}: the round must reach its reply; stdout:\n{}",
        result.stdout
    );
    provider.assert_no_script_violations();

    // The provider read every body as JSON in its own dialect, tool results included.
    let bodies: Vec<Option<String>> = provider
        .requests()
        .into_iter()
        .filter(|entry| entry.method == "POST" && entry.path == upstream_path)
        .map(|entry| entry.body.map(|body| body.to_string()))
        .collect();
    assert!(
        bodies.iter().all(Option::is_some),
        "{label}: every body the relay forwarded must parse as JSON; log:\n{}",
        provider.request_log_summary()
    );
    let bodies: Vec<String> = bodies.into_iter().flatten().collect();
    assert!(
        bodies.iter().any(|body| body.contains(SECOND_MARKER)),
        "{label}: the second tool result must reach the provider; log:\n{}",
        provider.request_log_summary()
    );

    let wire: Vec<RecordedRequest> = tap
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && is_inference(&r.path))
        .collect();
    let summary = format!("{wire:#?}");
    assert!(
        wire.iter().all(|r| r.status == 200),
        "{label}: every request must succeed on its first send, with no lost base:\n{summary}"
    );
    // A side request such as the title can stay under the floor. Its wire size is its plain size.
    assert!(
        wire.iter()
            .all(|r| r.content_encoding.as_deref() == Some("zstd")
                || r.wire_bytes < COMPRESSION_FLOOR),
        "{label}: every body past the floor must go out zstd-compressed:\n{summary}"
    );
    let wire: Vec<RecordedRequest> = wire
        .into_iter()
        .filter(|r| r.content_encoding.is_some())
        .collect();
    let full = wire
        .iter()
        .find(|r| r.zstd_base.is_none())
        .unwrap_or_else(|| {
            panic!("{label}: the first request has no base to start from:\n{summary}")
        });
    let deltas: Vec<&RecordedRequest> = wire.iter().filter(|r| r.zstd_base.is_some()).collect();
    assert!(
        deltas.len() >= 2,
        "{label}: each turn after the first must go out as a delta:\n{summary}"
    );
    for delta in deltas {
        assert!(
            delta.wire_bytes * 4 < full.wire_bytes,
            "{label}: a delta must be far smaller than a full frame of the same history:\n{summary}"
        );
    }
}

#[tokio::test]
async fn messages_round_rides_zstd_deltas_through_ai_proxy() {
    run_round_through_relay("messages", "/v1/messages").await;
}

#[tokio::test]
async fn chat_completions_round_rides_zstd_deltas_through_ai_proxy() {
    run_round_through_relay("chat_completions", "/v1/chat/completions").await;
}
