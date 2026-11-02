// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// 10. **Reverse direction mismatch.**
/// A mid-session switch between mismatched harnesses goes through in both
/// directions, so the recovery is not one-way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn reverse_agent_type_mismatch_cursor_to_default() {
    let content = ContentController::start_with_models(vec![
        MockModel::with_agent_type("cursor-model", STRICT_HARNESS_AGENT_TYPE),
        MockModel::new("default-model"),
    ])
    .await
    .expect("start content");
    content.set_response(format!(
        "{MOCK_RESPONSE_SENTINEL} hello from alternate template."
    ));

    let binary = pager_binary().expect("resolve pager binary");

    let mut harness = PtyHarness::spawn_with_content(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--model", "cursor-model"],
    )
    .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // Send a prompt to establish turn_count > 0.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("response rendered");

    // Switch to default-model, which crosses agent types from cursor to grok-build
    harness
        .inject_keys(b"/model default-model\r")
        .expect("type model switch");

    harness
        .wait_for_text("default-model", Duration::from_secs(15))
        .expect("the target model must become the session's model");

    assert!(
        !harness.contains_text("requires starting a new session"),
        "a cross-harness switch must not ask for a new session\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        harness.is_running().expect("poll pager liveness"),
        "pager exited\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
