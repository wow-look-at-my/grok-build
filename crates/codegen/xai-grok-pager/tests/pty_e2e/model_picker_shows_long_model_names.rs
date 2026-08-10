// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **`/model` rows must be readable when every model id is long.**
///
/// Gateway catalogs name models `provider/vendor:family:size`, which runs past
/// 40 columns on its own, and the current row adds " (current)". The dropdown
/// sized its label column from the widest label under a 40-column cap while
/// DISCARDING the ones above it, so a catalog where every id is long left
/// nothing to take a max over: a zero-width column, and rows that draw,
/// highlight and switch models with nothing written in them.
///
/// Unit tests over the width function did not catch it, and could not have
/// caught it alone -- what was broken was what reached the screen. So this
/// asserts against the rendered terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn model_picker_shows_long_model_names() {
    const LONG_MODELS: [&str; 4] = [
        "Synthetic_Anthropic/syn:small:text",
        "Synthetic_Anthropic/hf:zai-org/GLM-4.7-Flash",
        "Synthetic_Anthropic/hf:moonshotai/Kimi-K2-Instruct",
        "Synthetic_Anthropic/hf:deepseek-ai/DeepSeek-V3.2",
    ];

    let content =
        ContentController::start_with_models(LONG_MODELS.into_iter().map(MockModel::new).collect())
            .await
            .expect("start content with gateway-length model ids");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // Open the `/model` argument dropdown without picking anything.
    harness.inject_keys(b"/model ").expect("type /model ");

    // Each id must appear on screen. A prefix long enough to be unambiguous is
    // the assertion -- the tail may be truncated at this terminal width, and
    // truncation is fine. Blank is not.
    for model in LONG_MODELS {
        let visible_prefix = &model[..30.min(model.len())];
        let found = harness
            .wait_for_text(visible_prefix, Duration::from_secs(15))
            .is_ok();
        assert!(
            found,
            "`{model}` is missing from the /model dropdown -- the row is blank.\nscreen:\n{}",
            harness.screen_contents()
        );
    }

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
