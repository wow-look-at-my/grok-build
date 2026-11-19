//! Memory works with no command: a turn is captured, a stored note is recalled.

use crate::acp_harness;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_harness::{
    AutoApproveClient, connect_and_auth_with_memory, connect_and_auth_with_memory_config,
    new_session, prompt_turn, run_agent_test,
};
use agent_client_protocol as acp;

const RECALL_OPEN_TAG: &str = "<memory-recall>";

/// A workspace outside the system temp directory. Memory is off for a temp workspace.
struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("memory-recall-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create workspace");
        Self(dir)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn global_scope_dir() -> PathBuf {
    PathBuf::from(std::env::var("GROK_HOME").expect("GROK_HOME"))
        .join("memory-v2")
        .join("global")
}

/// Bodies of the main-loop requests that carry `prompt`. The memory extractor and the session
/// title request also quote the prompt, so both are left out.
fn turn_bodies(server: &xai_grok_test_support::MockInferenceServer, prompt: &str) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|request| {
            !request
                .header("x-grok-req-id")
                .is_some_and(|id| id.starts_with("xai-memory-capture-"))
        })
        .filter_map(|request| request.body.map(|body| body.to_string()))
        .filter(|body| body.contains(prompt) && !body.contains("\"name\":\"session_title\""))
        .collect()
}

async fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn a_stored_topic_is_recalled_once_when_the_conversation_reaches_it() {
    run_agent_test(|_cwd, server| async move {
        let workspace = Workspace::new("topic");
        let (conn, _init) =
            connect_and_auth_with_memory(AutoApproveClient, "memory-recall-test").await;
        let session_id = new_session(&conn, &workspace.0).await;

        // The scope directories appear with the first prompt, so the test makes this.
        let topics = global_scope_dir().join("topics");
        std::fs::create_dir_all(&topics).expect("create topics dir");
        std::fs::write(
            topics.join("release.md"),
            "# Release steps\nHow a release is cut.\n\n## Zebrafish deploy pipeline\n\
             - The zebrafish deploy pipeline starts with `make quokka-release`.\n",
        )
        .expect("write topic");

        // The mock echoes the last message by default. An echo would repeat the reminder.
        server.set_response("Noted.");
        let first_prompt = "How do I run the zebrafish deploy pipeline?";
        prompt_turn(&conn, &session_id, first_prompt).await;
        let bodies = turn_bodies(&server, first_prompt);
        let body = bodies.first().expect("the turn must reach the model");
        assert!(
            body.contains(RECALL_OPEN_TAG),
            "the first request must carry a recall reminder: {body}"
        );
        assert!(
            body.contains("make quokka-release"),
            "the reminder must carry the note's text: {body}"
        );
        assert!(
            body.contains("- Release steps (global topic, "),
            "the reminder must name the note and its scope: {body}"
        );
        assert!(body.contains("release.md"), "{body}");

        let second_prompt = "Run the zebrafish deploy pipeline one more time, please.";
        prompt_turn(&conn, &session_id, second_prompt).await;
        let bodies = turn_bodies(&server, second_prompt);
        let body = bodies
            .first()
            .expect("the second turn must reach the model");
        assert_eq!(
            body.matches(RECALL_OPEN_TAG).count(),
            1,
            "a note is shown one time for a conversation: {body}"
        );
    });
}

#[test]
fn a_preference_from_one_workspace_is_recalled_in_another_without_a_command() {
    run_agent_test(|_cwd, server| async move {
        let first_workspace = Workspace::new("capture");
        let second_workspace = Workspace::new("recall");
        let (conn, _init) =
            connect_and_auth_with_memory(AutoApproveClient, "memory-capture-test").await;

        // The mock gives every request this reply, so the extractor reads it as its output.
        server.set_response(
            serde_json::json!({
                "outcome": "observations",
                "observations": [{
                    "type": "user",
                    "scope": "global",
                    "topic_hint": "code style",
                    "statement": "The user wants every indent to be a tab, never spaces.",
                    "keywords": ["indent", "indentation", "tabs", "whitespace"],
                    "aliases": ["tab"],
                    "body": null
                }]
            })
            .to_string(),
        );
        let first_session = new_session(&conn, &first_workspace.0).await;
        prompt_turn(
            &conn,
            &first_session,
            "Reformat this file the way I like it.",
        )
        .await;

        let inbox = global_scope_dir().join("observations/_inbox");
        let note = wait_for("the captured note in global memory", || {
            std::fs::read_dir(&inbox)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    std::fs::read_to_string(path)
                        .is_ok_and(|text| text.contains("every indent to be a tab"))
                })
        })
        .await;

        server.set_response("Done.");
        let second_session = new_session(&conn, &second_workspace.0).await;
        let prompt = "Please fix the indentation and whitespace in main.rs.";
        prompt_turn(&conn, &second_session, prompt).await;
        let bodies = turn_bodies(&server, prompt);
        let body = bodies.first().expect("the turn must reach the model");
        assert!(
            body.contains(RECALL_OPEN_TAG),
            "another workspace must recall the global note: {body}"
        );
        assert!(
            body.contains("never spaces. (global note, "),
            "the reminder must carry the preference as a global note: {body}"
        );
        let file_name = note
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name");
        assert!(body.contains(file_name), "{body}");
    });
}

/// An OpenAI-style `/v1/embeddings` server on its own thread. A text about a
/// lighthouse or a beacon gets one vector.
struct EmbeddingServer {
    base_url: String,
    authorization: Arc<Mutex<Vec<String>>>,
}

impl EmbeddingServer {
    fn start() -> Self {
        let authorization = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&authorization);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("address").port();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("embedding server runtime");
            runtime.block_on(async move {
                let embed =
                    move |headers: axum::http::HeaderMap,
                          axum::Json(body): axum::Json<serde_json::Value>| {
                        let seen = Arc::clone(&seen);
                        async move {
                            if let Some(value) = headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                            {
                                seen.lock().expect("lock").push(value.to_owned());
                            }
                            let data: Vec<serde_json::Value> = body["input"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|text| {
                                    let text = text.as_str().unwrap_or_default().to_lowercase();
                                    let embedding =
                                        if text.contains("lighthouse") || text.contains("beacon") {
                                            [1.0, 0.0]
                                        } else {
                                            [1.0, 0.3]
                                        };
                                    serde_json::json!({ "embedding": embedding })
                                })
                                .collect();
                            axum::Json(serde_json::json!({
                                "model": body["model"],
                                "data": data,
                                "usage": { "prompt_tokens": 1, "total_tokens": 1 }
                            }))
                        }
                    };
                let app = axum::Router::new().route("/v1/embeddings", axum::routing::post(embed));
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                axum::serve(listener, app).await.expect("serve embeddings");
            });
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            authorization,
        }
    }
}

/// An auto-approving client that keeps every session update the agent sends.
#[derive(Clone, Default)]
struct UpdateRecorder {
    updates: Rc<RefCell<Vec<serde_json::Value>>>,
}

impl UpdateRecorder {
    fn of_kind(&self, kind: &str) -> Vec<serde_json::Value> {
        self.updates
            .borrow()
            .iter()
            .filter(|update| update["sessionUpdate"] == kind)
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait(?Send)]
impl acp::Client for UpdateRecorder {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Ok(acp::RequestPermissionResponse::new(
            acp_harness::allow_once(&args),
        ))
    }

    async fn session_notification(&self, _args: acp::SessionNotification) -> acp::Result<()> {
        Ok(())
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        if args.method.as_ref() != "example.invalid/session_notification" {
            return Ok(());
        }
        if let Ok(params) = serde_json::from_str::<serde_json::Value>(args.params.get()) {
            self.updates.borrow_mut().push(params["update"].clone());
        }
        Ok(())
    }
}

fn write_global_topic(file: &str, content: &str) {
    let topics = global_scope_dir().join("topics");
    std::fs::create_dir_all(&topics).expect("create topics dir");
    std::fs::write(topics.join(file), content).expect("write topic");
}

#[test]
fn a_paraphrase_is_recalled_through_the_configured_embedding_endpoint() {
    run_agent_test(|_cwd, server| async move {
        let embeddings = EmbeddingServer::start();
        let workspace = Workspace::new("embedding");
        let config = format!(
            "[memory.embedding]\nmodel = \"test-embed\"\nbase_url = \"{}\"\n",
            embeddings.base_url
        );
        let (conn, _init) = connect_and_auth_with_memory_config(
            AutoApproveClient,
            "memory-embedding-test",
            &config,
        )
        .await;
        let session_id = new_session(&conn, &workspace.0).await;
        write_global_topic(
            "coast.md",
            "# Lighthouse\nThe keeper trims the lamp wick nightly.\n",
        );

        // The prompt shares no word with the topic, so only a vector can match it.
        server.set_response("Noted.");
        let prompt = "Tell me about a beacon that guides ships.";
        prompt_turn(&conn, &session_id, prompt).await;
        let bodies = turn_bodies(&server, prompt);
        let body = bodies.first().expect("the turn must reach the model");
        assert!(
            body.contains(RECALL_OPEN_TAG),
            "the request must carry a recall reminder: {body}"
        );
        assert!(
            body.contains("The keeper trims the lamp wick nightly."),
            "the reminder must carry the topic the vector matched: {body}"
        );

        let sent = embeddings.authorization.lock().expect("lock").clone();
        assert!(!sent.is_empty(), "the embedding endpoint must be called");
        assert!(
            sent.iter().all(|value| !value.contains("test-key-for-ci")),
            "the chat key must not reach the embedding endpoint: {sent:?}"
        );
    });
}

#[test]
fn a_dead_embedding_endpoint_is_reported_one_time_and_word_recall_goes_on() {
    run_agent_test(|_cwd, server| async move {
        let workspace = Workspace::new("dead-embedding");
        // Nothing listens on this port after the listener drops.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let config = format!(
            "[memory.embedding]\nmodel = \"test-embed\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\n"
        );
        let recorder = UpdateRecorder::default();
        let (conn, _init) = connect_and_auth_with_memory_config(
            recorder.clone(),
            "memory-dead-embedding-test",
            &config,
        )
        .await;
        let session_id = new_session(&conn, &workspace.0).await;
        write_global_topic(
            "release.md",
            "# Release steps\nHow a release is cut.\n\n## Zebrafish deploy pipeline\n\
             - The zebrafish deploy pipeline starts with `make quokka-release`.\n",
        );

        server.set_response("Noted.");
        let first_prompt = "How do I run the zebrafish deploy pipeline?";
        prompt_turn(&conn, &session_id, first_prompt).await;
        let bodies = turn_bodies(&server, first_prompt);
        let body = bodies.first().expect("the turn must reach the model");
        assert!(
            body.contains(RECALL_OPEN_TAG) && body.contains("make quokka-release"),
            "word matching must still recall the topic: {body}"
        );

        let early = recorder.of_kind("memory_recall_degraded");
        assert!(
            early.is_empty(),
            "one failed lookup can be a model that still loads, and is not reported: {early:?} turn requests {} all requests {:?}",
            bodies.len(),
            server
                .requests()
                .iter()
                .map(|request| request
                    .header("x-grok-req-id")
                    .unwrap_or_default()
                    .to_owned())
                .collect::<Vec<_>>()
        );

        prompt_turn(&conn, &session_id, "Now tell me about the weather.").await;
        prompt_turn(&conn, &session_id, "And what about tomorrow?").await;
        let degraded = recorder.of_kind("memory_recall_degraded");
        assert_eq!(
            degraded.len(),
            1,
            "the client must hear one time that the embedding lookup cannot run: {degraded:?}"
        );
        assert!(
            degraded
                .first()
                .and_then(|update| update["reason"].as_str())
                .is_some_and(|reason| !reason.is_empty()),
            "{degraded:?}"
        );
    });
}
