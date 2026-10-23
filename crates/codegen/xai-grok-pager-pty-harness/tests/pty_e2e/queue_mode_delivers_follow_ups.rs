// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Spawn the pager on `content` and submit the first prompt.
#[cfg(unix)]
fn spawn_and_submit(content: &ContentController) -> PtyHarness {
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
        &["--yolo", "--trust"],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
}

/// Type `text` and press Enter once.
#[cfg(unix)]
fn type_follow_up(harness: &mut PtyHarness, text: &str) {
    harness
        .inject_keys(text.as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit follow-up");
}

/// Wait until a request body carries `marker`, with no further key press.
#[cfg(unix)]
fn assert_reaches_model(
    content: &ContentController,
    harness: &PtyHarness,
    marker: &str,
    within: Duration,
) {
    let reached = poll_for(within, || {
        content
            .request_bodies()
            .iter()
            .any(|b| b.to_string().contains(marker))
            .then_some(())
    });
    assert!(
        reached.is_some(),
        "the follow-up never reached the model on its own\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );
}

/// Queue mode: a follow-up typed while a foreground tool runs goes into the request after that tool.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_folds_follow_up_typed_during_a_tool() {
    const MARKER: &str = "QUEUE_TOOL_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_hold",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_TOOL_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{MARKER} also check this"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(30));
    harness
        .wait_for_text("QUEUE_TOOL_SETTLED", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the turn never settled; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_secs(2));
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(MARKER))
        .expect("a carrier");
    assert!(
        first.to_string().contains("call_queue_hold"),
        "the first request that carries the follow-up must be the one after the tool\n{}",
        dump_non_system_messages(&bodies)
    );
    assert_eq!(
        block_lines_containing(&harness, MARKER),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: a follow-up typed while the final answer streams runs as the next turn when that one ends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_final_answer() {
    const MARKER: &str = "QUEUE_FINAL_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    content.set_chunk_delay(Some(Duration::from_millis(150)));
    let _final = content.expect_agent_turn("final answer", slow_turn_text("FINALSTREAM"));
    content.set_response("QUEUE_AFTER_FINAL");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("FINALSTREAM", Duration::from_secs(45))
        .expect("final answer streaming");
    type_follow_up(&mut harness, &format!("{MARKER} one more"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(60));
    harness
        .wait_for_text("QUEUE_AFTER_FINAL", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("never answered; screen:\n{}", harness.screen_contents()));
    harness.update(Duration::from_secs(2));
    assert_eq!(
        block_lines_containing(&harness, MARKER),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: follow-ups typed during one tool both reach the model with one Enter each.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_delivers_two_follow_ups_typed_during_a_tool() {
    const FIRST: &str = "QUEUE_TWO_FIRST_XYZ";
    const SECOND: &str = "QUEUE_TWO_SECOND_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_two",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_TWO_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{FIRST} one"));
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{SECOND} two"));

    assert_reaches_model(&content, &harness, FIRST, Duration::from_secs(30));
    assert_reaches_model(&content, &harness, SECOND, Duration::from_secs(30));
    harness.quit().expect("clean quit");
}
