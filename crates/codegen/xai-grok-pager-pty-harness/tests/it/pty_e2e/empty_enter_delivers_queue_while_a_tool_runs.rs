// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

const MARKER: &str = "ENTER_DELIVER_MARKER_XYZ";

/// Enter on an empty composer says "Interrupting". While a foreground command holds the turn there is no model stream to cut.
/// The queued row must still reach the model at once, not when the command ends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn empty_enter_delivers_queue_while_a_tool_runs() {
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 30",
        "description": "long hold"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_enter_hold",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("ENTER_DELIVERED");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
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
        .wait_for_text("long hold", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));

    harness
        .inject_keys(format!("{MARKER} now please").as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("queue the follow-up");
    harness
        .wait_for_text(MARKER, Duration::from_secs(10))
        .expect("follow-up queued");
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\r").expect("empty Enter interrupts");

    let started = Instant::now();
    let reached = poll_for(Duration::from_secs(15), || {
        content
            .request_bodies()
            .iter()
            .any(|b| b.to_string().contains(MARKER))
            .then_some(())
    });
    assert!(
        reached.is_some(),
        "empty Enter said it was interrupting but the row waited for the command\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the row must not wait for the 30s command"
    );
    harness
        .wait_for_text("ENTER_DELIVERED", Duration::from_secs(30))
        .expect("the delivered row was answered");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
