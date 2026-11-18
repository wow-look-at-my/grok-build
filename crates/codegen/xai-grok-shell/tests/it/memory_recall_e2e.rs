//! Memory works with no command: a turn is captured, a stored note is recalled.

use crate::acp_harness;

use std::path::{Path, PathBuf};
use std::time::Duration;

use acp_harness::{
    AutoApproveClient, connect_and_auth_with_memory, new_session, prompt_turn, run_agent_test,
};

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
