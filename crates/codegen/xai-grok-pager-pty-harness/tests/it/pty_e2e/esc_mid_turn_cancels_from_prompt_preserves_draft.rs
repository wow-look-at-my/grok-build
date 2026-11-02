// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// A bare Esc from the prompt pane cancels a running turn at once, with no Ctrl+C reminder, and keeps a non-empty draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn esc_mid_turn_cancels_from_prompt_preserves_draft() {
    let content = ContentController::start().await.expect("start content");
    // Stream a long paced response so the turn is still visibly running when Esc lands
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

    // A distinctive single token avoids any wrapping ambiguity
    let draft = "DRAFTKEEPME";
    harness.inject_keys(draft.as_bytes()).expect("type draft");
    harness
        .wait_for_text(draft, Duration::from_secs(10))
        .expect("draft renders in the composer");

    harness.inject_keys(keys::ESC).expect("press esc");
    harness
        .wait_for_text("Turn cancelled by user", Duration::from_secs(15))
        .expect("mid-turn Esc must cancel the turn");

    harness.update(Duration::from_millis(600));
    let screen = harness.screen_contents();
    assert!(
        screen.contains(draft),
        "mid-turn Esc must preserve the draft\nscreen:\n{screen}"
    );
    assert!(
        !screen.contains("to cancel the turn"),
        "mid-turn Esc must not show a Ctrl+C reminder\nscreen:\n{screen}"
    );
    assert!(
        !screen.contains("press again to clear"),
        "running-turn Esc must not arm the idle clear\nscreen:\n{screen}"
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
