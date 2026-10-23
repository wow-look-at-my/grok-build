//! PTY: text the shell sends the model between steps has its own row, above the reply it caused.
#[allow(unused_imports)]
use super::common::*;

const FIRST: &str = "FIRST_REPLY_MARK";
const GATE: &str = "GATE_REASON_MARK";
const SECOND: &str = "SECOND_REPLY_MARK";
const ROW_HEADER: &str = "Sent to the model · stop gate";

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

/// A Stop hook that blocks the first stop and allows the next one.
fn seed_block_once_stop_hook(content: &ContentController) {
    let flag = content.home().join("stop_gate_fired");
    let command = format!(
        "if [ -e {flag} ]; then exit 0; fi; : > {flag}; echo {GATE} >&2; exit 2",
        flag = flag.display()
    );
    let spec = json!({
        "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": command, "timeout": 10 }] }]
        }
    });
    seed_hook_spec(content, "block_once.json", &spec);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn stop_gate_feedback_is_shown_above_the_reply() {
    let content = ContentController::start().await.expect("start content");
    seed_block_once_stop_hook(&content);
    let _first = content.expect_agent_turn("first reply", FIRST);
    let _second = content.expect_agent_turn("reply to the gate", SECOND);
    content.set_response("STOP_GATE_FALLBACK");

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
        .wait_for_text("Worked for", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("turn never ended; screen:\n{}", harness.screen_contents()));

    let screen = harness.screen_contents();
    let first = row_of(&screen, FIRST);
    let header = row_of(&screen, ROW_HEADER);
    // The hook annotation above the row also names the reason, so look for the body below the header.
    let reason = header.and_then(|h| {
        screen
            .lines()
            .enumerate()
            .skip(h + 1)
            .find(|(_, line)| line.contains(GATE))
            .map(|(i, _)| i)
    });
    let second = row_of(&screen, SECOND);
    assert!(
        header.is_some() && reason.is_some(),
        "the gate's message to the model must have a row with its text; screen:\n{screen}"
    );
    assert!(
        first < header && header <= reason && reason < second,
        "order must be: first reply, the gate's message, the reply to it; screen:\n{screen}"
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
