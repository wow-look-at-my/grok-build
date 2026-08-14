// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use crate::common::*;

/// Minimal + Apple Terminal: Ctrl+O is the send-now chord. With an empty
/// composer and a mid-turn queued follow-up it must interrupt — the in-flight
/// stream is cancelled and the row is delivered into the SAME turn as an
/// interjection — not open the transcript pager remap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn minimal_ctrl_o_interrupts_queued_apple_terminal() {
    let content = ContentController::start().await.expect("start content");
    let mut turn_one = content.expect_agent_turn_blocked(
        "running turn before the minimal Ctrl+O interrupt",
        slow_turn_text("STEPONE"),
    );
    let _resubmitted = content.expect_agent_turn(
        "resubmitted request carrying the interjection",
        "STEPTWO interrupt via Ctrl+O acknowledged.",
    );

    let binary = pager_binary().expect("resolve pager binary");
    let mut overrides: Vec<(String, String)> =
        vec![("TERM_PROGRAM".into(), "Apple_Terminal".into())];
    // Non-interactive $PAGER so a mistaken transcript open fails fast rather
    // than hanging in `less` if the predicate regresses.
    overrides.push(("PAGER".into(), "cat".into()));
    let env_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        MINIMAL_ARGS,
        &env_refs,
    )
    .expect("spawn minimal + Apple_Terminal");
    harness.set_respond_to_queries(true);

    wait_minimal_ready(&mut harness);

    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text("STEPONE", Duration::from_secs(30))
        .expect("turn 1 streaming");
    tokio::time::timeout(Duration::from_secs(10), turn_one.wait_blocked())
        .await
        .expect("turn 1 reached completion barrier");

    harness
        .inject_keys(b"minimal interrupt payload\r")
        .expect("queue follow-up");
    harness
        .wait_for_text("1 queued", Duration::from_secs(10))
        .expect("queue indicator");

    // Empty composer + queue: Ctrl+O must yield to the interrupt, not the
    // transcript. The shell cancels the in-flight stream, drains the row as an
    // interjection, and resubmits; the row commits as a standard "❯ " block.
    harness.inject_keys(CTRL_O).expect("Ctrl+O interrupt");
    // Generous deadline: this wait is pure render latency, which under heavy
    // parallel-suite load can exceed the old 15s budget.
    harness
        .wait_for_text(
            "\u{276F} minimal interrupt payload",
            Duration::from_secs(60),
        )
        .expect("delivered-row chrome (not a silent transcript open)");

    // Let the mock's gate go; the cancelled request's completion is moot.
    turn_one.release();
    harness
        .wait_for_text("STEPTWO", Duration::from_secs(40))
        .expect("resubmitted turn reply");

    // Interrupting is not cancelling: the turn continues, so no marker
    // (scrollback-aware check: minimal commits blocks into native history).
    assert!(
        !harness.contains_full_text("Turn cancelled by user"),
        "an interrupt must not render a cancelled marker\nfull contents:\n{}",
        harness.full_text()
    );

    let users = all_user_message_blobs(&content);
    let sent = users
        .iter()
        .find(|u| u.contains("minimal interrupt payload"))
        .unwrap_or_else(|| panic!("queued follow-up never on wire: {users:#?}"));
    assert!(
        sent.contains(INTERJECTION_WIRE_PREFIX),
        "the row must arrive as a mid-turn interjection: {sent}"
    );
    assert!(
        sent.contains("<user_query>"),
        "the interjection still wraps the user's text: {sent}"
    );

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    quit_minimal(&mut harness);
}
