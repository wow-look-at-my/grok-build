// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use crate::common::*;

const FEEDBACK_LABEL_SENTINEL: &str = "How can we improve Grok Build?";
const FEEDBACK_PLACEHOLDER_SENTINEL: &str = "Please provide as much detail as possible.";
const THANKS_SENTINEL: &str = "Thanks for the feedback";
const PANE_FEEDBACK: &str = "minimal-pty-feedback-report-xyz";

/// Minimal: bare `/feedback` opens the freeform pane and submits like full TUI.
///
/// The no-session guard is deliberately NOT here, and it is not a PTY subject
/// at all: `session_id` is `None` only between the prompt rendering and
/// `session/new` returning, and how long that lasts is a property of the
/// machine. Sleeping 5s before the keys proves it -- unloaded, the session
/// binds and the guard goes unreachable; under a full parallel suite it was
/// still unbound 15s in. So the version of this test that asserted the guard
/// was reading load, not behavior, and flipped with it. The other route to
/// no-session does not reach the guard either: when creation fails,
/// `handle_session_failed` removes the sole agent and shows Welcome, so
/// `dispatch_open_feedback_pane` returns at its `ActiveView::Agent` check.
/// `enter_feedback_mode_requires_session` (dispatch::tests::notes) owns the
/// guard, setting `session_id` to `None` outright and pinning the message, the
/// pane staying shut, and minimal's system block against fullscreen's toast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn minimal_feedback_pane_opens_and_submits() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} minimal ready."));

    let mut harness = spawn_minimal(&content);
    wait_minimal_ready(&mut harness);
    // The pane is gated on a bound session, and the idle prompt does not mean
    // there is one.
    wait_session_bound(&mut harness);

    harness
        .inject_keys(b"/feedback\r")
        .expect("bare /feedback with session");
    harness
        .wait_for_text(FEEDBACK_LABEL_SENTINEL, Duration::from_secs(15))
        .expect("feedback pane label in minimal");
    assert!(
        harness.contains_text(FEEDBACK_PLACEHOLDER_SENTINEL),
        "composer placeholder must render in minimal\nscreen:\n{}",
        harness.screen_contents()
    );

    harness
        .inject_keys(format!("{PANE_FEEDBACK}\r").as_bytes())
        .expect("submit freeform feedback");
    harness
        .wait_for_full_text(THANKS_SENTINEL, Duration::from_secs(15))
        .expect("minimal pane submit should thank the user");
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    quit_minimal(&mut harness);
}
