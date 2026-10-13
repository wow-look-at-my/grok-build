// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

const DONE_SENTINEL: &str = "TODO_PANE_SYNC_DONE";

/// PTY. The todo pane on screen lists the same items with the same statuses as the list
/// the agent reads back in its next request. This happens after the model writes its todo
/// list twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn todo_pane_matches_agent_list_pty() {
    let content = ContentController::start().await.expect("start content");
    let create = json!({
        "merge": true,
        "todos": [
            {"id": "a", "content": "write the docs", "status": "pending"},
            {"id": "b", "content": "ship the parser", "status": "in_progress"},
            {"id": "c", "content": "tag the release", "status": "pending"},
        ]
    })
    .to_string();
    let finish = json!({"merge": true, "todos": [{"id": "b", "status": "completed"}]}).to_string();
    let _create_turn = expect_tool_turn(&content, "call_todo_1", "todo_write", create);
    let _finish_turn = expect_tool_turn(&content, "call_todo_2", "todo_write", finish);
    content.set_response(DONE_SENTINEL);

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(DONE_SENTINEL, Duration::from_secs(90))
        .unwrap_or_else(|_| {
            panic!(
                "expected the turn to settle; got:\n{}",
                harness.screen_contents()
            )
        });

    // Ctrl+T toggles the todo pane.
    harness.inject_keys(b"\x14").expect("toggle the todo pane");
    harness
        .wait_for_text("1/3", Duration::from_secs(20))
        .unwrap_or_else(|_| {
            panic!(
                "the todo pane must show one of three done; got:\n{}",
                harness.screen_contents()
            )
        });
    let screen = harness.screen_contents();
    for item in ["write the docs", "ship the parser", "tag the release"] {
        assert!(
            screen.contains(item),
            "the todo pane must list {item:?}; got:\n{screen}"
        );
    }

    // The agent's own list: the read-back the model got after its second write.
    let last_request = content
        .requests()
        .into_iter()
        .rev()
        .find_map(|entry| entry.body)
        .expect("an inference request")
        .to_string();
    for line in [
        "- [pending] a: write the docs",
        "- [completed] b: ship the parser",
        "- [pending] c: tag the release",
    ] {
        assert!(
            last_request.contains(line),
            "the agent's list must hold {line:?}; request was:\n{last_request}"
        );
    }
}
