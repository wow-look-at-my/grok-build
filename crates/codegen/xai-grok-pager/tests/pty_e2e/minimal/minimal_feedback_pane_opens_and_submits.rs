// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use crate::common::*;

const FEEDBACK_LABEL_SENTINEL: &str = "How can we improve Grok Build?";
const FEEDBACK_PLACEHOLDER_SENTINEL: &str = "Please provide as much detail as possible.";
const THANKS_SENTINEL: &str = "Thanks for the feedback";
const PANE_FEEDBACK: &str = "minimal-pty-feedback-report-xyz";

/// Minimal: bare `/feedback` opens the freeform pane and submits like full TUI.
///
/// The no-session guard is NOT tested here. Reaching it means racing startup:
/// the idle prompt renders before `NewSessionComplete` arrives, so whether
/// `session_id` is bound when the keys land is a coin flip, and the assertion
/// fails whenever the session wins. `enter_feedback_mode_requires_session`
/// (dispatch::tests::notes) owns that guard instead -- it sets `session_id` to
/// `None` outright and pins both the message and minimal's system-block
/// rendering of it, which is the whole of what this half was reaching for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn minimal_feedback_pane_opens_and_submits() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} minimal ready."));

    let mut harness = spawn_minimal(&content);
    wait_minimal_ready(&mut harness);

    // Establish a session, then open the pane and submit.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("response rendered");

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
