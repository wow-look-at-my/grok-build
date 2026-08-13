// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// With two mid-turn queued rows, empty Enter delivers **both** into the
/// running turn, in queue order — the interrupt is "take everything I have",
/// not "take the top one". The resubmitted request carries the original prompt
/// followed by alpha then bravo, each with the mid-turn preamble.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn empty_enter_delivers_both_queued_rows() {
    let content = ContentController::start().await.expect("start content");
    let mut turn_one = content.expect_agent_turn_blocked(
        "running turn before the interrupt",
        slow_turn_text("TURNONE"),
    );
    let mut resubmitted = content.expect_agent_turn(
        "resubmitted request carrying both rows",
        "TURNTWO both queued rows acknowledged.",
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
        .wait_for_text("TURNONE", Duration::from_secs(45))
        .expect("turn 1 streaming");
    tokio::time::timeout(Duration::from_secs(10), turn_one.wait_blocked())
        .await
        .expect("turn 1 reached completion barrier");

    harness
        .inject_keys(b"queue-alpha-top\r")
        .expect("queue alpha");
    harness
        .wait_for_text("queue-alpha-top", Duration::from_secs(20))
        .expect("alpha visible");
    harness
        .inject_keys(b"queue-bravo-later\r")
        .expect("queue bravo");
    harness
        .wait_for_text("queue-bravo-later", Duration::from_secs(20))
        .expect("bravo visible");

    harness.inject_keys(b"\r").expect("empty Enter interrupt");
    turn_one.release();
    // Both rows land on the resubmitted request. Blocks can scroll above the
    // viewport before a 100ms poll observes them, so gate on the WIRE — the
    // authoritative record — rather than on-screen markers. Pump the event loop
    // while waiting so the delivery actually happens.
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while !all_user_messages(&content)
        .iter()
        .any(|u| u.contains("queue-bravo-later"))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "queued rows never reached the model\nscreen:\n{}",
            harness.screen_contents()
        );
        harness.update(Duration::from_millis(100));
    }
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
    let alpha = users
        .iter()
        .find(|u| u.contains("queue-alpha-top"))
        .unwrap_or_else(|| panic!("top row never on wire: {users:#?}"));
    assert!(
        alpha.contains(INTERJECTION_WIRE_PREFIX),
        "delivered rows arrive as mid-turn interjections: {alpha}"
    );

    // The final request's user sequence proves the order: prompt, then alpha,
    // then bravo — never bravo before alpha.
    let bodies = content.request_bodies();
    let last = bodies.last().expect("final request recorded");
    let finals: Vec<String> = last["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter(|m| {
            m["role"] == "user"
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("<user_query>"))
        })
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(3, finals.len(), "expected 3 user messages: {finals:#?}");
    assert!(finals[0].contains(PROMPT), "first: {finals:#?}");
    assert!(
        finals[1].contains("queue-alpha-top"),
        "second must be the TOP row: {finals:#?}"
    );
    assert!(
        finals[2].contains("queue-bravo-later"),
        "third must be bravo: {finals:#?}"
    );

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
