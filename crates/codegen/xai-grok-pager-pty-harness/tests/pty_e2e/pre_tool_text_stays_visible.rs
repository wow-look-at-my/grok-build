//! PTY: text the model writes before a tool call stays on screen.
#[allow(unused_imports)]
use super::common::*;

const PRE_MARK: &str = "PRE_TOOL_MARK";
const POST_MARK: &str = "POST_TOOL_MARK";

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

fn quit(harness: &mut PtyHarness) {
    // Focus is in the prompt, so `q` would only type. Ctrl+Q twice quits.
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}

/// Text, a tool call, its result, then more text: the first text shows while the turn runs, after it ends, and after a resume.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn pre_tool_text_survives_turn_and_resume() {
    let content = ContentController::start().await.expect("start content");
    let args = json!({ "command": "true", "description": "no-op" }).to_string();
    let _first = expect_text_then_tool_turn(
        &content,
        PRE_MARK,
        "call_pre_tool",
        "run_terminal_command",
        args,
    );
    let mut second = content.expect_agent_turn_blocked("text after the tool", POST_MARK);

    let project = tempfile::tempdir().expect("create project dir");
    std::fs::create_dir_all(project.path().join(".git")).expect("create .git");
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        Some(project.path()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");

    let running = wait_until(Duration::from_secs(60), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(PRE_MARK) && screen.contains(POST_MARK) && !screen.contains("Worked for")
    });
    assert!(
        running,
        "the text before the tool call must show while the turn still runs; screen:\n{}",
        harness.screen_contents()
    );

    second.release();
    harness
        .wait_for_text("Worked for", Duration::from_secs(30))
        .expect("turn ended");
    let screen = harness.screen_contents();
    assert!(
        row_of(&screen, PRE_MARK) < row_of(&screen, POST_MARK),
        "the text before the tool call must stay above the text after it; screen:\n{screen}"
    );
    assert!(
        !screen.contains(NO_MESSAGE_MARK),
        "a turn with text must not say it wrote none; screen:\n{screen}"
    );
    quit(&mut harness);

    let mut resumed = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust", "--continue"],
        Some(project.path()),
    )
    .expect("spawn resumed pager");
    resumed
        .wait_for_text(POST_MARK, WELCOME_TIMEOUT)
        .expect("history replayed after --continue");
    let screen = resumed.screen_contents();
    assert_eq!(
        screen.matches(PRE_MARK).count(),
        1,
        "the text before the tool call must replay exactly once; screen:\n{screen}"
    );
    assert!(row_of(&screen, PRE_MARK) < row_of(&screen, POST_MARK));
    quit(&mut resumed);
}

/// The same shape on a turn the harness starts itself: a background command's completion wake.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn pre_tool_text_survives_completion_wake() {
    let content = ContentController::start().await.expect("start content");
    let flag = content.home().join("pre_tool_wake_flag");
    let gated = json!({
        "command": format!("while [ ! -e {} ]; do /bin/sleep 0.2; done", flag.display()),
        "description": "flag-gated command",
        "is_background": true
    })
    .to_string();
    let _background = expect_tool_turn(&content, "call_bg_gate", "run_terminal_command", gated);
    let _settled = content.expect_agent_turn("turn settles", "WAKE_TEST_SETTLED");
    let noop = json!({ "command": "true", "description": "no-op" }).to_string();
    let _wake = expect_text_then_tool_turn(
        &content,
        PRE_MARK,
        "call_wake_tool",
        "run_terminal_command",
        noop,
    );
    let _wake_end = content.expect_agent_turn("wake ends", POST_MARK);
    content.set_response("WAKE_TEST_FALLBACK");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        50,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text("WAKE_TEST_SETTLED", Duration::from_secs(60))
        .expect("first turn settled");

    std::fs::write(&flag, b"done").expect("release flag");
    harness
        .wait_for_text(POST_MARK, Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the wake turn never finished; screen:\n{}",
                harness.screen_contents()
            )
        });
    let screen = harness.screen_contents();
    assert!(
        row_of(&screen, PRE_MARK).is_some()
            && row_of(&screen, PRE_MARK) < row_of(&screen, POST_MARK),
        "a wake turn's text before its tool call must show above its later text; screen:\n{screen}"
    );
    quit(&mut harness);
}

/// A turn that calls a tool and writes no text says so on its end marker.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn tool_only_turn_marks_no_message() {
    let content = ContentController::start().await.expect("start content");
    let noop = json!({ "command": "true", "description": "no-op" }).to_string();
    let _tool = expect_tool_turn(&content, "call_silent", "run_terminal_command", noop);
    let _end = content.expect_agent_turn("ends without text", "");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(NO_MESSAGE_MARK, Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "a turn with no text must say `{NO_MESSAGE_MARK}` on its end marker; screen:\n{}",
                harness.screen_contents()
            )
        });
    let screen = harness.screen_contents();
    let marker = screen
        .lines()
        .find(|line| line.contains(NO_MESSAGE_MARK))
        .unwrap_or_default();
    assert!(
        marker.contains("Worked for"),
        "the no-message note must sit on the end marker row; row: {marker:?}"
    );
    quit(&mut harness);
}

/// The text after the pager's `NO_MESSAGE_SUFFIX` separator.
const NO_MESSAGE_MARK: &str = "no message";
