// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Mid-turn: queue a follow-up with Enter, then bare Enter on the empty
/// composer interrupts — the in-flight model stream is cancelled and the row is
/// handed to the SAME turn as an interjection (the wire carries the mid-turn
/// preamble), rather than waiting for the turn or running as its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn empty_enter_interrupts_with_queued() {
    let content = ContentController::start().await.expect("start content");
    let mut turn_one = content
        .expect_agent_turn_blocked("running turn before the interrupt", slow_turn_text("TURNONE"));
    let mut resubmitted = content.expect_agent_turn(
        "resubmitted request carrying the interjection",
        "TURNTWO reply after the interrupt.",
    );

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text("TURNONE", Duration::from_secs(30))
        .expect("turn 1 streaming");
    tokio::time::timeout(Duration::from_secs(10), turn_one.wait_blocked())
        .await
        .expect("turn 1 reached the completion barrier");

    harness
        .inject_keys(b"please also check the logs\r")
        .expect("queue follow-up via Enter");
    harness
        .wait_for_text("please also check the logs", Duration::from_secs(10))
        .expect("queued text visible");

    // Composer is empty after queue; bare Enter interrupts. The shell cancels
    // the in-flight stream, drains the row as an interjection, and resubmits.
    harness.inject_keys(b"\r").expect("empty Enter interrupt");
    turn_one.release();
    // The delivered row renders as a "❯ " user block (interjections use the
    // standard prompt chrome), replacing the prefix-less queue row.
    harness
        .wait_for_text(
            "\u{276F} please also check the logs",
            Duration::from_secs(15),
        )
        .expect("delivered prompt scrollback chrome");

    harness
        .wait_for_text("TURNTWO", Duration::from_secs(40))
        .expect("resubmitted turn reply");
    tokio::time::timeout(Duration::from_secs(10), resubmitted.wait_satisfied())
        .await
        .expect("resubmitted turn expectation satisfied");

    // Interrupting is not cancelling: the turn continues, so no marker.
    assert!(
        !harness.contains_text("Turn cancelled by user"),
        "an interrupt must not render a cancelled marker\nscreen:\n{}",
        harness.screen_contents()
    );

    let users = all_user_message_blobs(&content);
    let delivered = users
        .iter()
        .find(|u| u.contains("please also check the logs"))
        .unwrap_or_else(|| panic!("queued follow-up never reached the wire: {users:#?}"));
    assert!(
        delivered.contains(INTERJECTION_WIRE_PREFIX),
        "the row must arrive as a mid-turn interjection: {delivered}"
    );
    assert!(
        delivered.contains("<user_query>"),
        "the interjection still wraps the user's text: {delivered}"
    );

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
