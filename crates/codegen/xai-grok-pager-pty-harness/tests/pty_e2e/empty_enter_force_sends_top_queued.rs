// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

<<<<<<<< HEAD:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_interrupts_with_queued.rs
/// Mid-turn: queue a follow-up with Enter, then bare Enter on the empty
/// composer interrupts — the in-flight model stream is cancelled and the row is
/// handed to the SAME turn as an interjection (the wire carries the mid-turn
/// preamble), rather than waiting for the turn or running as its own.
========
/// Mid-turn: queue a follow-up with Enter, then bare Enter on the empty composer sends that top row now (cancel-and-send).
/// The running turn is cancelled silently and the row runs as its own next turn.
/// It arrives on the wire as a standard `<user_query>` prompt with the interjection preamble.
>>>>>>>> upstream/main:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_force_sends_top_queued.rs
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn empty_enter_interrupts_with_queued() {
    let content = ContentController::start().await.expect("start content");
<<<<<<<< HEAD:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_interrupts_with_queued.rs
    let mut turn_one = content.expect_agent_turn_blocked(
        "running turn before the interrupt",
        slow_turn_text("TURNONE"),
    );
    let mut resubmitted = content.expect_agent_turn(
        "resubmitted request carrying the interjection",
        "TURNTWO reply after the interrupt.",
========
    content
        .server()
        .set_settings(json!({ "allow_access": true, "dock_enabled": true }));
    std::fs::write(
        content.sandbox().grok_home().join("requirements.toml"),
        "[features]\ndock = true\n",
    )
    .expect("pin dock in test requirements");
    let mut turn_one = content
        .expect_agent_turn_blocked("running turn before send-now", slow_turn_text("TURNONE"));
    let mut turn_two = content.expect_agent_turn(
        "promoted queued follow-up",
        "TURNTWO reply to the promoted follow-up.",
>>>>>>>> upstream/main:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_force_sends_top_queued.rs
    );

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &[("GROK_DOCK", "1")],
        Some(content.home()),
    )
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
    assert!(
        !harness.contains_text("Queued · Enter to send now"),
        "dock-shown queueing must not show the send-now tip\nscreen:\n{}",
        harness.screen_contents()
    );

<<<<<<<< HEAD:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_interrupts_with_queued.rs
    // Composer is empty after queue; bare Enter interrupts. The shell cancels
    // the in-flight stream, drains the row as an interjection, and resubmits.
    harness.inject_keys(b"\r").expect("empty Enter interrupt");
    // The delivered row renders as a "❯ " user block (interjections use the
    // standard prompt chrome), replacing the prefix-less queue row. Wait for it
    // BEFORE releasing the completion barrier: the shell harvests into a
    // RUNNING turn, so releasing first would race the interrupt against turn
    // end and let the row drain as its own turn instead.
========
    // Composer is empty after queue; bare Enter sends the top row now
    // The shell cancels turn 1 (the abort beats the held completion) and promotes the row to run as turn 2
    harness.inject_keys(b"\r").expect("empty Enter send-now");
    turn_one.release();
    // The promoted row renders as a standard "❯ " prompt block via the turn-start adoption
    // The arrow prefix distinguishes the committed block from the prefix-less queue row
>>>>>>>> upstream/main:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_force_sends_top_queued.rs
    harness
        .wait_for_text(
            "\u{276F} please also check the logs",
            Duration::from_secs(30),
        )
        .expect("delivered prompt scrollback chrome");
    turn_one.release();

    harness
        .wait_for_text("TURNTWO", Duration::from_secs(40))
        .expect("resubmitted turn reply");
    tokio::time::timeout(Duration::from_secs(10), resubmitted.wait_satisfied())
        .await
        .expect("resubmitted turn expectation satisfied");

<<<<<<<< HEAD:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_interrupts_with_queued.rs
    // Interrupting is not cancelling: the turn continues, so no marker.
========
    // The send-now cancel is silent: no cancelled marker between the partial turn-1 output and the promoted prompt
>>>>>>>> upstream/main:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_force_sends_top_queued.rs
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
<<<<<<<< HEAD:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_interrupts_with_queued.rs
        delivered.contains(INTERJECTION_WIRE_PREFIX),
        "the row must arrive as a mid-turn interjection: {delivered}"
    );
    assert!(
        delivered.contains("<user_query>"),
        "the interjection still wraps the user's text: {delivered}"
========
        promoted.contains(INTERJECTION_WIRE_PREFIX),
        "send-now must use the interjection preamble: {promoted}"
    );
    assert!(
        promoted.contains("<user_query>"),
        "send-now must wrap the steered text in user_query: {promoted}"
>>>>>>>> upstream/main:crates/codegen/xai-grok-pager-pty-harness/tests/pty_e2e/empty_enter_force_sends_top_queued.rs
    );

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
