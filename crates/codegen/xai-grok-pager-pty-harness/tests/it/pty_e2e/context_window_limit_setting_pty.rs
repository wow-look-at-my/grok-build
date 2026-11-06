// context_window_limit_setting_pty.rs holds test.

#[allow(unused_imports)]
use super::common::*;

const TIMEOUT: Duration = Duration::from_secs(20);

fn config_text(content: &ContentController) -> String {
    std::fs::read_to_string(content.home().join(".grok").join("config.toml")).unwrap_or_default()
}

fn open_limit_row(harness: &mut PtyHarness) {
    harness.inject_keys(keys::F2).expect("open settings");
    harness
        .wait_for_text("Appearance", TIMEOUT)
        .expect("settings shown");
    harness.inject_keys(b"/").expect("search settings");
    harness
        .wait_for_text_absent("/ to search", TIMEOUT)
        .expect("search focused");
    harness
        .inject_keys(b"context window limit")
        .expect("type search");
    harness
        .wait_for_text("Context window limit", TIMEOUT)
        .expect("row found by search");
    harness
        .inject_keys(keys::ENTER)
        .expect("commit settings search");
    harness.update(Duration::from_millis(300));
    harness
        .inject_keys(keys::ENTER)
        .expect("open model chooser");
}

/// Pick the session model in the chooser, type `350k`, and expect the limit in
/// `config.toml` and in the chooser label when the row opens again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn context_window_limit_setting_writes_config_and_shows_the_limit() {
    let content = ContentController::start().await.expect("mock server");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} context limit turn."));

    let binary = pager_binary().expect("pager binary");
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
    harness.update(Duration::from_millis(400));

    open_limit_row(&mut harness);
    harness
        .wait_for_text("\u{00B7}", TIMEOUT)
        .unwrap_or_else(|_| {
            panic!(
                "chooser lists models with windows\n{}",
                harness.screen_contents()
            )
        });
    harness
        .inject_keys(keys::ENTER)
        .expect("pick the first model");
    harness
        .wait_for_text("Type a limit", TIMEOUT)
        .unwrap_or_else(|_| panic!("limit editor opens\n{}", harness.screen_contents()));
    harness.inject_keys(b"350k").expect("type limit");
    harness.inject_keys(keys::ENTER).expect("save limit");

    harness
        .wait_until("config.toml holds the limit", TIMEOUT, |_| {
            config_text(&content).contains("context_window = 350000")
        })
        .unwrap_or_else(|_| {
            panic!(
                "config.toml after save:\n{}\nscreen:\n{}",
                config_text(&content),
                harness.screen_contents()
            )
        });

    harness
        .wait_for_text("350k", TIMEOUT)
        .unwrap_or_else(|_| panic!("row shows the limit\n{}", harness.screen_contents()));

    harness.quit().expect("clean quit");
}

/// A limit set in the row reaches the running session: with a 1k window, a
/// prompt of about 1.5k tokens makes the session compact. The model's own
/// window is above 100k, so nothing else can start that compaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn context_window_limit_reaches_the_running_session() {
    let content = ContentController::start().await.expect("mock server");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} context limit turn."));

    let binary = pager_binary().expect("pager binary");
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
    harness.update(Duration::from_millis(400));
    assert!(
        !harness.screen_contents().contains("ompact"),
        "no compaction before the limit\n{}",
        harness.screen_contents()
    );

    open_limit_row(&mut harness);
    harness
        .wait_for_text("\u{00B7}", TIMEOUT)
        .unwrap_or_else(|_| panic!("chooser opens\n{}", harness.screen_contents()));
    harness
        .inject_keys(keys::ENTER)
        .expect("pick the first model");
    harness
        .wait_for_text("Type a limit", TIMEOUT)
        .unwrap_or_else(|_| panic!("limit editor opens\n{}", harness.screen_contents()));
    harness.inject_keys(b"1k").expect("type limit");
    harness.inject_keys(keys::ENTER).expect("save limit");
    harness
        .wait_until("config.toml holds the limit", TIMEOUT, |_| {
            config_text(&content).contains("context_window = 1000")
        })
        .unwrap_or_else(|_| panic!("config.toml:\n{}", config_text(&content)));
    harness.inject_keys(keys::ESC).expect("close editor");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(keys::ESC).expect("close settings");
    // The config watcher reloads the models and sends the window to the session.
    harness.update(Duration::from_secs(3));

    let long_prompt = "lorem ipsum dolor sit amet ".repeat(220);
    harness
        .inject_keys(format!("{long_prompt}\r").as_bytes())
        .expect("submit long prompt");
    harness
        .wait_until("the session compacts", Duration::from_secs(60), |h| {
            let screen = h.screen_contents();
            screen.contains("Compacting") || screen.contains("compacted")
        })
        .unwrap_or_else(|_| {
            panic!(
                "a 1k limit must make the session compact\n{}",
                harness.screen_contents()
            )
        });

    harness.quit().expect("clean quit");
}
