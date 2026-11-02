// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// A bare Esc from the SCROLLBACK pane cancels a running turn in one press.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn esc_mid_turn_cancels_from_scrollback() {
    let content = ContentController::start().await.expect("start content");
    let long_response = format!(
        "{MOCK_RESPONSE_SENTINEL} {}",
        "streaming filler words for the cancellation window. ".repeat(120)
    );
    content.set_response(long_response);
    content.set_chunk_delay(Some(Duration::from_millis(50)));

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
        .expect("stream started");

    // Tab toggles focus, so press it once and wait for the footer to prove the scrollback owns keys
    harness.inject_keys(b"\t").expect("tab to scrollback");
    harness
        .wait_for_text("Space:prompt", Duration::from_secs(10))
        .expect("scrollback must own keys before the Esc");

    harness.inject_keys(keys::ESC).expect("press esc");
    harness
        .wait_for_text("Turn cancelled by user", Duration::from_secs(15))
        .expect("mid-turn Esc from scrollback must cancel the turn");

    harness.update(Duration::from_millis(600));
    let screen = harness.screen_contents();
    assert_eq!(
        screen.matches("Turn cancelled by user").count(),
        1,
        "'Turn cancelled' must appear exactly once\nscreen:\n{screen}"
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
