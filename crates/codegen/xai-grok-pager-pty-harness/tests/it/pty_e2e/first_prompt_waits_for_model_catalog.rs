// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// A prompt sent while the first model-catalog fetch is still in flight must still reach
/// the model. A session takes its model's URL. This happens when it is created, so a
/// session that started before the catalog would send to a model with no URL ("Server
/// error").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn first_prompt_waits_for_model_catalog() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!(
        "{MOCK_RESPONSE_SENTINEL} the catalog arrived in time."
    ));
    // Only the model list is slow: settings land at once.
    content.server().set_models_delay(Duration::from_secs(6));

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
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("the first prompt is answered once the catalog lands");
    assert!(
        !harness.contains_text("Server error"),
        "a prompt sent during the catalog fetch must not fail\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
