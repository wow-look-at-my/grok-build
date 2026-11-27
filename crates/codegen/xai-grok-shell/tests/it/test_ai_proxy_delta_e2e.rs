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

/// How the config names the relay.
#[derive(Clone, Copy, PartialEq)]
enum Path {
    /// The provider's `base_url` is the relay's own URL.
    BaseUrl,
    /// The provider's `base_url` is the provider. The relay is a hop of its chain.
    Hop,
}

/// What a round left behind for a test to read.
struct Round {
    /// One `shell.turn.request_transport` context per model request, in order.
    transports: Vec<serde_json::Value>,
    /// The relay build reports its own share of the wait in `Server-Timing`.
    relay_reports_timing: bool,
}

/// Ask the relay for one upstream answer and read its `Server-Timing` header.
async fn relay_reports_timing(relay_origin: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let authority = relay_origin.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("connect to the relay");
    let request =
        format!("GET /up/v1/models HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send to the relay");
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer).await;
    let answer = String::from_utf8_lossy(&answer).to_ascii_lowercase();
    let head = answer.split("\r\n\r\n").next().unwrap_or_default();
    head.lines()
        .any(|line| line.starts_with("server-timing:") && line.contains("relay-upstream"))
}

/// The transport record the shell logged for each model request.
fn logged_transports(sandbox: &TestSandbox) -> Vec<serde_json::Value> {
    let path = sandbox.grok_home().join("logs").join("unified.jsonl");
    let log = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("no unified log at {}: {error}", path.display()));
    log.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["msg"] == "shell.turn.request_transport")
        .map(|entry| entry["ctx"].clone())
        .collect()
}

async fn run_round_through_relay(api_backend: &str, upstream_path: &str, path: Path) -> Round {
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
    let provider_block = match path {
        Path::BaseUrl => format!(
            "base_url = \"{tap}/up/v1\"\napi_key = \"{PROVIDER_KEY}\"\n",
            tap = tap.origin()
        ),
        // The chain puts the relay's route in place of the provider's origin.
        Path::Hop => format!(
            "base_url = \"{provider}/v1\"\napi_key = \"{PROVIDER_KEY}\"\n\n\
             [[model_providers.relay.hops]]\nid = \"nas\"\nkind = \"relay\"\noptional = true\n\
             base_url = \"{tap}\"\nroutes = {{ relay = \"up\" }}\n",
            provider = provider.origin(),
            tap = tap.origin()
        ),
    };
    std::fs::write(
        sandbox.grok_home().join("config.toml"),
        format!(
            "[models]\ndefault = \"relay/{MODEL}\"\n\n[model_providers.relay]\n{provider_block}"
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
    for r in &wire {
        eprintln!(
            "{label}: {} {} bytes on the wire, base {}",
            r.path,
            r.wire_bytes,
            r.zstd_base.as_deref().unwrap_or("none")
        );
    }
    Round {
        transports: logged_transports(&sandbox),
        relay_reports_timing: relay_reports_timing(&relay.origin).await,
    }
}

#[tokio::test]
async fn messages_round_rides_zstd_deltas_through_ai_proxy() {
    run_round_through_relay("messages", "/v1/messages", Path::BaseUrl).await;
}

#[tokio::test]
async fn chat_completions_round_rides_zstd_deltas_through_ai_proxy() {
    run_round_through_relay("chat_completions", "/v1/chat/completions", Path::BaseUrl).await;
}

/// The relay is a hop of the provider's chain. Every request still reaches
/// the provider through it, and the shell records how each travelled.
#[tokio::test]
async fn a_hop_chain_sends_the_round_through_ai_proxy_and_records_its_transport() {
    let round =
        run_round_through_relay("chat_completions", "/v1/chat/completions", Path::Hop).await;
    let transports = &round.transports;
    let summary = format!("{transports:#?}");

    let deltas: Vec<&serde_json::Value> = transports
        .iter()
        .filter(|transport| transport["encoding"] == "zstd_delta")
        .collect();
    assert!(
        deltas.len() >= 2,
        "each turn after the first must be recorded as a delta:\n{summary}"
    );
    for delta in &deltas {
        let plain = delta["plain_bytes"].as_u64().unwrap_or(0);
        let wire = delta["wire_bytes"].as_u64().unwrap_or(u64::MAX);
        assert!(
            plain > COMPRESSION_FLOOR as u64 && wire.saturating_mul(4) < plain,
            "a delta record must show what the link saved:\n{summary}"
        );
    }
    assert!(
        transports
            .iter()
            .any(|transport| transport["encoding"] == "zstd"),
        "the first request past the floor is a full zstd frame:\n{summary}"
    );

    // An older relay sends no `Server-Timing`. The record then has no link time.
    for transport in transports.iter().filter(|t| t["encoding"] != "plain") {
        let names: Vec<&str> = transport["server_timing"]
            .as_array()
            .map(|metrics| metrics.iter().filter_map(|m| m["name"].as_str()).collect())
            .unwrap_or_default();
        if round.relay_reports_timing {
            assert!(
                names.contains(&"relay-upstream") && names.contains(&"relay-decode"),
                "the relay's own share of the wait must be recorded:\n{summary}"
            );
            let link = transport["link_ms"].as_u64();
            let headers = transport["headers_ms"].as_u64().unwrap_or(0);
            assert!(
                link.is_some_and(|link| link <= headers),
                "the link time is the part of the header wait the relay did not spend:\n{summary}"
            );
        } else {
            assert!(
                !names.contains(&"relay-upstream") && transport["link_ms"].is_null(),
                "a relay that reports no timing gives no link time:\n{summary}"
            );
        }
    }
    eprintln!(
        "relay reports timing: {}; transports:\n{summary}",
        round.relay_reports_timing
    );
}
