//! PTY: with no embedding model in the config, recall matches by meaning through an embedding
//! model that a configured local runtime lists.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const REPLY_MARK: &str = "LOCALEMBEDMARK";
const EMBEDDING_MODEL: &str = "nomic-embed-text:latest";
const TOPIC_LINE: &str = "The keeper trims the lamp wick nightly.";

/// A loopback Ollama: one chat model, one embedding model, and
/// `/v1/embeddings`.
struct LocalRuntime {
    base_url: String,
    embedding_calls: Arc<AtomicUsize>,
}

impl LocalRuntime {
    fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let embedding_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&embedding_calls);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let calls = Arc::clone(&calls);
                std::thread::spawn(move || answer(stream, &calls));
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            embedding_calls,
        }
    }
}

fn answer(mut stream: std::net::TcpStream, embedding_calls: &AtomicUsize) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut content_length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let reply = match path {
        "/api/tags" => serde_json::json!({
            "models": [
                { "name": "llama3.2:latest", "model": "llama3.2:latest" },
                { "name": EMBEDDING_MODEL, "model": EMBEDDING_MODEL },
            ]
        }),
        "/api/ps" => serde_json::json!({ "models": [] }),
        "/api/show" => {
            let capabilities = if body["model"] == EMBEDDING_MODEL {
                serde_json::json!(["embedding"])
            } else {
                serde_json::json!(["completion", "tools"])
            };
            serde_json::json!({ "capabilities": capabilities })
        }
        "/v1/embeddings" => {
            embedding_calls.fetch_add(1, Ordering::SeqCst);
            let data: Vec<serde_json::Value> = body["input"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|text| {
                    let text = text.as_str().unwrap_or_default().to_lowercase();
                    let embedding = if text.contains("lighthouse") || text.contains("beacon") {
                        [1.0, 0.0]
                    } else {
                        [1.0, 0.3]
                    };
                    serde_json::json!({ "embedding": embedding })
                })
                .collect();
            serde_json::json!({
                "model": body["model"],
                "data": data,
                "usage": { "prompt_tokens": 1, "total_tokens": 1 }
            })
        }
        _ => serde_json::json!({ "error": "not found" }),
    };
    let reply = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

/// A workspace outside the system temp directory. Memory is off for a temp workspace.
struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("memory-local-embedding-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create workspace");
        Self(dir)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every chat history under `grok_home`, joined.
fn histories(grok_home: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![grok_home.join("sessions")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name == "chat_history.jsonl")
            {
                text.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    text
}

/// The config names a local runtime and no embedding model. The prompt shares no word with the
/// stored topic. Recall still finds the topic, and `/memory` names the model that matched it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn recall_matches_by_meaning_through_a_local_runtime_with_no_embedding_config() {
    let runtime = LocalRuntime::start();
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{REPLY_MARK} noted."));
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.ollama]\nbase_url = \"{}\"\n",
            runtime.base_url
        ),
    )
    .expect("write config.toml");
    let topics = grok_home.join("memory-v2").join("global").join("topics");
    std::fs::create_dir_all(&topics).expect("create the topics directory");
    std::fs::write(
        topics.join("coast.md"),
        format!("# Lighthouse\n{TOPIC_LINE}\n"),
    )
    .expect("write the topic");
    let workspace = Workspace::new();

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        Some(&workspace.0),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // The first prompt starts the session. It is about nothing stored.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(REPLY_MARK, Duration::from_secs(30))
        .expect("the reply is on screen");
    harness
        .wait_for_turn_idle(Duration::from_secs(30))
        .expect("idle session");

    // `/memory` names the model once the runtime's listing was read.
    let recall_line = format!(
        "Recall: words and meaning \u{00B7} {EMBEDDING_MODEL} \u{00B7} {}",
        runtime.base_url
    );
    let mut shown = false;
    let mut browser = String::new();
    for _ in 0..20 {
        harness.inject_keys(b"/memory\r").expect("submit /memory");
        shown = harness
            .wait_for_text(&recall_line, Duration::from_secs(3))
            .is_ok();
        browser = harness.screen_contents();
        harness.inject_keys(keys::ESC).expect("close /memory");
        harness.update(Duration::from_millis(300));
        if shown {
            break;
        }
        assert!(!browser.contains("panicked"), "pager panicked\n{browser}");
    }
    assert!(
        shown,
        "`/memory` must name the embedding model of the local runtime\nthe browser:\n{browser}"
    );

    // No word of this prompt is in the topic, so only a vector can match it.
    harness
        .inject_keys(b"Tell me about a beacon that guides ships.\r")
        .expect("submit the beacon prompt");
    harness
        .wait_until("the topic is recalled", Duration::from_secs(60), |_| {
            let history = histories(&grok_home);
            history.contains("<memory-recall>") && history.contains(TOPIC_LINE)
        })
        .unwrap_or_else(|error| {
            panic!(
                "the beacon prompt must recall the lighthouse topic: {error}\nembedding calls: {}\nscreen:\n{}",
                runtime.embedding_calls.load(Ordering::SeqCst),
                harness.screen_contents()
            )
        });
    assert!(
        runtime.embedding_calls.load(Ordering::SeqCst) > 0,
        "the local runtime must serve the embedding calls"
    );
    harness.quit().expect("clean quit");
}
