// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Submitting a prompt produces the mock server's response text on screen.
/// This is the full loop: the pager sends to the shell agent, which hits mock inference, streams chunks back, and the pager renders them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn agent_response() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!(
        "{MOCK_RESPONSE_SENTINEL} hello from the mock inference server."
    ));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager with content");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // Type the prompt and submit with Enter.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");

    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("mock response on screen");
    assert!(
        content.has_chat_completion(),
        "mock inference server never received a chat completion request\nrequests: {:?}",
        content.requests()
    );

    harness.quit().expect("clean quit");
}

/// The status row carries a `tok/s` reading while the response is still
/// arriving. The stream is paced so the reading has to appear mid-response.
/// Waiting for the stream to finish would pass even if the counter only showed
/// up at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn agent_response_shows_a_rate_while_streaming() {
    let content = ContentController::start().await.expect("start content");
    let tail_word = "RATEDONE";
    let mut body = format!("{MOCK_RESPONSE_SENTINEL}");
    for word in 0..120 {
        body.push_str(&format!(" w{word}"));
    }
    body.push_str(&format!(" {tail_word}"));
    content.set_response(body);
    content.set_chunk_delay(Some(Duration::from_millis(100)));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager with content");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");

    harness
        .wait_until(
            "a tok/s reading appears before the last chunk lands",
            Duration::from_secs(30),
            |h| h.contains_text("tok/s") && !h.contains_text(tail_word),
        )
        .expect("the row reports its rate while the stream is still running");
    harness
        .wait_for_text(tail_word, Duration::from_secs(30))
        .expect("the paced stream finishes");

    harness.quit().expect("clean quit");
}
