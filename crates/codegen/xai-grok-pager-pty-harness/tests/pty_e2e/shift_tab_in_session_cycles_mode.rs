// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

// ── Interactive flow e2e tests ──────────────────────────────────────────

/// 15. **In-session Shift+Tab cycles permission mode.**
/// Routes BackTab through the agent view's `resolve_action`, the path that
/// previously dropped `CycleMode`; test 2b only covers the welcome screen.
/// With the auto gate on (client default): Normal → Plan → Auto →
/// Always-Approve → Orchestrator → Explore → Plan (the ring never lands
/// back on bare Normal once cycling has started).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn shift_tab_in_session_cycles_mode() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} turn done."));

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
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("turn rendered");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Plan", Duration::from_secs(10))
        .expect("first cycle: Normal -> Plan");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Auto", Duration::from_secs(10))
        .expect("second cycle: Plan -> Auto");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Always-Approve", Duration::from_secs(10))
        .expect("third cycle: Auto -> Always-Approve");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Orchestrator", Duration::from_secs(10))
        .expect("fourth cycle: Always-Approve -> Orchestrator");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Explore", Duration::from_secs(10))
        .expect("fifth cycle: Orchestrator -> Explore");

    harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
    harness
        .wait_for_text("Switched to mode: Plan", Duration::from_secs(10))
        .expect("sixth cycle: Explore -> Plan (ring closes)");

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}

/// Whether the prompt's flag row carries `flag`.
///
/// The row renders each flag behind a middle-dot separator, and matching on
/// that separator keeps the probe off the word where it appears anywhere else
/// on screen.
fn mode_flag(harness: &PtyHarness, flag: &str) -> bool {
    let needle = format!("\u{B7} {flag}");
    harness
        .screen_output()
        .lines
        .iter()
        .any(|line| line.contains(&needle))
}

/// 15b. **Two rapid Shift+Tab presses land on the LAST stop and stay there.**
///
/// Both presses go out before the shell has confirmed the first one, so the
/// confirmation for the first stop (Plan) arrives after the ring already
/// advanced to Auto. The burst must settle on Auto and keep it: the prompt's
/// `auto` flag stays up and the `plan` flag the earlier press asked for never
/// appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn rapid_shift_tab_presses_stay_on_the_last_stop() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} turn done."));

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
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("turn rendered");

    // Normal -> Plan -> Auto in one burst, with no confirmation read in
    // between: the whole point of the case.
    harness
        .inject_keys(b"\x1b[Z")
        .expect("inject first BackTab");
    harness
        .inject_keys(b"\x1b[Z")
        .expect("inject second BackTab");
    harness
        .wait_for_text("Switched to mode: Auto", Duration::from_secs(10))
        .expect("second press must show Auto");

    // Hold long enough for the first press's confirmation to land. A step back
    // to Plan clears the `auto` flag and lights `plan` instead.
    harness
        .wait_until_stable(
            "the auto flag held, with no step back to the plan the earlier press asked for",
            Duration::from_secs(15),
            Duration::from_secs(3),
            |h| mode_flag(h, "auto") && !mode_flag(h, "plan"),
        )
        .expect("the ring must keep the last press's stop, not the previous one");

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
