// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

const TIMEOUT: Duration = Duration::from_secs(60);

/// `/compact` sends its summary request to the dense summarization model and
/// names that model on screen. The session model and the dense model are
/// different mock models, so a request that falls back to the session model
/// fails both checks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn compact_names_the_dense_summarization_model() {
    let content = ContentController::start_with_models(vec![
        MockModel::new("test-model"),
        MockModel::new("dense-model"),
    ])
    .await
    .expect("start content");
    std::fs::write(
        content.sandbox().grok_home().join("config.json"),
        r#"{"models": {"default": "test-model", "groups": {"dense_summarization": {"model": "dense-model"}}}}"#,
    )
    .expect("write config.json");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &[("GROK_COMPACTION_MODE", "summary")],
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} compact probe."));
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, TIMEOUT)
        .expect("turn rendered");
    harness
        .wait_for_turn_idle(Duration::from_secs(15))
        .expect("turn idle");

    // The shell rejects a short summary as degenerate, so the mock answers the compaction request with a long one.
    content.set_response(long_response("SUMMARY", 10));
    let before = content.requests().len();
    harness.inject_keys(b"/compact\r").expect("submit /compact");
    harness
        .wait_for_text("Compacting with dense-model", TIMEOUT)
        .expect("the compaction marker names the dense model");
    harness
        .wait_for_text("Compaction completed in", TIMEOUT)
        .expect("compaction completes");

    // Other side calls can run in the same window. Only the requests that
    // carry the compaction prompt are compaction requests.
    let models: Vec<String> = content
        .requests()
        .into_iter()
        .skip(before)
        .filter_map(|r| {
            let body = r.body?;
            if !body
                .to_string()
                .contains("successor assistant can continue")
            {
                return None;
            }
            body.get("model")?.as_str().map(str::to_owned)
        })
        .collect();
    assert!(
        !models.is_empty() && models.iter().all(|m| m == "dense-model"),
        "every /compact request must go to the dense model, got {models:?}\nscreen:\n{}",
        harness.screen_contents()
    );
}
