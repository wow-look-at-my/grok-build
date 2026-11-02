// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// `[models].force_reasoning_effort_models` turns `/effort` on for a model the
/// server never flagged. The mock server reports `supports_reasoning_effort =
/// false`, which is the exact state that makes `/effort` refuse, so this drives
/// the whole chain: config -> catalog -> `meta.supportsReasoningEffort` -> gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn force_reasoning_effort_models_enables_effort() {
    let content = ContentController::start_with_models(vec![
        MockModel::new("grok-4.5").with_supports_reasoning_effort(false),
    ])
    .await
    .expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} turn."));

    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create .grok");
    std::fs::write(
        grok_home.join("config.toml"),
        "[models]\nforce_reasoning_effort_models = [\"grok-4.5\"]\n",
    )
    .expect("write config.toml");

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

    // A forced model carries no menu of its own, so it gets the built-in rows.
    inject_keys_paced(&mut harness, b"/effort ");
    harness
        .wait_for_text("Extended reasoning", Duration::from_secs(10))
        .expect("forced model offers the built-in /effort menu");

    harness.quit().expect("clean quit");
}
