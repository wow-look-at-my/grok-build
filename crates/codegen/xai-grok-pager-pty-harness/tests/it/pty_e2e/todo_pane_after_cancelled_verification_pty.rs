// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

const DONE_SENTINEL: &str = "TODO_CANCEL_SYNC_DONE";

fn inference_request_count(content: &ContentController) -> usize {
    content
        .requests()
        .iter()
        .filter(|entry| {
            entry.method == "POST"
                && entry.body.is_some()
                && ["responses", "chat/completions", "messages"]
                    .iter()
                    .any(|p| entry.path.contains(p))
        })
        .count()
}

/// PTY. The model asks to complete an item that has a verifier. The user cancels while the
/// verifier runs. The pane shows the item unfinished. Then the model reads its list back,
/// and the agent must report the item unfinished too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn todo_pane_after_cancelled_verification_pty() {
    let content = ContentController::start().await.expect("start content");
    let create = json!({
        "merge": true,
        "todos": [{
            "id": "b",
            "content": "ship the parser",
            "status": "in_progress",
            "verification": "cargo test -p parser passes",
        }]
    })
    .to_string();
    let finish = json!({"merge": true, "todos": [{"id": "b", "status": "completed"}]}).to_string();
    let _create_turn = expect_tool_turn(&content, "call_todo_1", "todo_write", create);
    let _finish_turn = expect_tool_turn(&content, "call_todo_2", "todo_write", finish);
    content.set_response(DONE_SENTINEL);
    content.set_chunk_delay(Some(Duration::from_millis(600)));

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

    let deadline = Instant::now() + Duration::from_secs(120);
    while inference_request_count(&content) < 3 {
        assert!(
            Instant::now() < deadline,
            "the verifier never started; screen:\n{}",
            harness.screen_contents()
        );
        harness.update(Duration::from_millis(30));
    }
    harness.inject_keys(keys::CTRL_C).expect("cancel the turn");
    harness
        .wait_for_text("Turn cancelled by user", Duration::from_secs(30))
        .unwrap_or_else(|_| panic!("cancel marker; got:\n{}", harness.screen_contents()));

    // What the pane shows before the model writes again.
    content.set_chunk_delay(None);
    harness.inject_keys(b"\x14").expect("toggle the todo pane");
    harness
        .wait_for_text("ship the parser", Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("todo pane rows; got:\n{}", harness.screen_contents()));
    let pane = harness.screen_contents();
    assert!(
        pane.contains("0/1") && !pane.contains("1/1"),
        "the pane must hold the item unfinished; got:\n{pane}"
    );

    // An empty write reads the agent's list back, without changing it.
    let read_back = json!({"merge": true, "todos": []}).to_string();
    let _read_back_turn = expect_tool_turn(&content, "call_todo_3", "todo_write", read_back);
    harness
        .inject_keys(b"carry on\r")
        .expect("submit the next prompt");
    harness
        .wait_for_text(DONE_SENTINEL, Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("next turn; got:\n{}", harness.screen_contents()));

    let last_request = content
        .requests()
        .into_iter()
        .rev()
        .find_map(|entry| entry.body)
        .expect("an inference request")
        .to_string();
    assert!(
        last_request.contains("- [in_progress] b: ship the parser"),
        "the agent must hold the item unfinished, as the pane does; request was:\n{last_request}"
    );
    assert!(
        !last_request.contains("[completed] b"),
        "the agent holds a completion the pane never showed; request was:\n{last_request}"
    );
}
