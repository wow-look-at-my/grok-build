// context_window_limit_setting_pty.rs holds test.

#[allow(unused_imports)]
use super::common::*;

const TIMEOUT: Duration = Duration::from_secs(20);

fn config_text(content: &ContentController) -> String {
    std::fs::read_to_string(content.home().join(".grok").join("config.json")).unwrap_or_default()
}

/// The `GROK_DEBUG_LOG` file of the test child.
fn debug_log_path(content: &ContentController) -> std::path::PathBuf {
    content.home().join("debug.log")
}

/// The debug log lines about the model reload and the context window.
fn window_log(content: &ContentController) -> String {
    let path = debug_log_path(content);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| format!("(no log at {}: {e})", path.display()))
        .lines()
        .filter(|l| l.contains("context window") || l.contains("model config"))
        .take(60)
        .collect::<Vec<_>>()
        .join("\n")
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
/// `config.json` and in the chooser label when the row opens again.
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
        .wait_until("config.json holds the limit", TIMEOUT, |_| {
            config_text(&content).contains("\"context_window\": 350000")
        })
        .unwrap_or_else(|_| {
            panic!(
                "config.json after save:\n{}\nscreen:\n{}",
                config_text(&content),
                harness.screen_contents()
            )
        });

    harness
        .wait_for_text("350k", TIMEOUT)
        .unwrap_or_else(|_| panic!("row shows the limit\n{}", harness.screen_contents()));

    harness.quit().expect("clean quit");
}

/// A limit set in the row reaches the running session. The session takes the
/// new window into its sampling config, and the context bar shows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn context_window_limit_reaches_the_running_session() {
    let content = ContentController::start().await.expect("mock server");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} context limit turn."));

    let binary = pager_binary().expect("pager binary");
    let log_path = debug_log_path(&content);
    let log_path = log_path.to_str().expect("utf-8 log path");
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &[("GROK_DEBUG_LOG", log_path)],
    )
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
        .wait_until("config.json holds the limit", TIMEOUT, |_| {
            config_text(&content).contains("\"context_window\": 1000")
        })
        .unwrap_or_else(|_| panic!("config.json:\n{}", config_text(&content)));
    harness.inject_keys(keys::ESC).expect("close editor");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(keys::ESC).expect("close settings");
    // The config watcher reloads the models and sends the window to the session.
    harness
        .wait_for_text("/ 1.0K", TIMEOUT)
        .unwrap_or_else(|_| {
            panic!(
                "the context bar must show the limit\nlog:\n{}\nscreen:\n{}",
                window_log(&content),
                harness.screen_contents()
            )
        });

    // The session actor logs the window it now samples with.
    harness
        .wait_until("the session takes the limit", TIMEOUT, |_| {
            window_log(&content)
                .lines()
                .any(|l| l.contains("context window reloaded from config") && l.contains("to=1000"))
        })
        .unwrap_or_else(|_| {
            panic!(
                "the running session must take the 1k window\nlog:\n{}",
                window_log(&content)
            )
        });

    harness.quit().expect("clean quit");
}
