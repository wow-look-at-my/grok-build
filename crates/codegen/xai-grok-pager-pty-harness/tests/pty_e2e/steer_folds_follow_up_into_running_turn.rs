// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

const STEER_MARKER: &str = "STEER_FOLD_MARKER_XYZ";

/// Steer mode: a follow-up typed while a tool runs reaches the running turn.
/// The request after the tool must carry it as an interjection, and it must not run as a turn of its own.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn steer_folds_follow_up_into_running_turn() {
    let content = ContentController::start().await.expect("start content");
    seed_ui_config(&content, "follow_up_behavior = \"steer\"");

    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold_turn = expect_tool_turn(
        &content,
        "call_steer_hold",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("STEER_TURN_SETTLED");

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
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));

    harness
        .inject_keys(format!("{STEER_MARKER} also check this").as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit follow-up");

    harness
        .wait_for_text("STEER_TURN_SETTLED", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the turn never settled; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_secs(2));

    let bodies = content.request_bodies();
    let carriers: Vec<usize> = bodies
        .iter()
        .enumerate()
        .filter(|(_, b)| b.to_string().contains(STEER_MARKER))
        .map(|(i, _)| i)
        .collect();
    assert!(
        !carriers.is_empty(),
        "the follow-up never reached the model\n{}",
        dump_non_system_messages(&bodies)
    );
    let first = &bodies[carriers[0]];
    assert!(
        first.to_string().contains(INTERJECTION_WIRE_PREFIX),
        "the follow-up was not folded into the running turn as an interjection\n{}",
        dump_non_system_messages(&bodies)
    );
    assert!(
        first.to_string().contains("call_steer_hold"),
        "the first request that carries the follow-up must be the one after the tool\n{}",
        dump_non_system_messages(&bodies)
    );
    assert_eq!(
        block_lines_containing(&harness, STEER_MARKER),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// Assert the marker reached the model first as an interjection and renders once.
#[cfg(unix)]
fn assert_folded_once(content: &ContentController, harness: &PtyHarness, marker: &str) {
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(marker))
        .unwrap_or_else(|| {
            panic!(
                "the follow-up never reached the model\n{}",
                dump_non_system_messages(&bodies)
            )
        });
    assert!(
        first.to_string().contains(INTERJECTION_WIRE_PREFIX),
        "the follow-up was not folded into the running turn as an interjection\n{}",
        dump_non_system_messages(&bodies)
    );
    assert_eq!(
        block_lines_containing(harness, marker),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
}

/// Steer mode: a follow-up typed while the final answer streams is folded before the turn ends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn steer_folds_follow_up_typed_during_final_answer() {
    const MARKER: &str = "STEER_FINAL_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    seed_ui_config(&content, "follow_up_behavior = \"steer\"");
    content.set_chunk_delay(Some(Duration::from_millis(150)));
    let _final = content.expect_agent_turn("final answer", slow_turn_text("FINALSTREAM"));
    content.set_response("STEER_AFTER_FOLD");

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
        .wait_for_text("FINALSTREAM", Duration::from_secs(45))
        .expect("final answer streaming");
    harness
        .inject_keys(format!("{MARKER} one more").as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit follow-up");
    harness
        .wait_for_text("STEER_AFTER_FOLD", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("never answered; screen:\n{}", harness.screen_contents()));
    harness.update(Duration::from_secs(2));
    assert_folded_once(&content, &harness, MARKER);
    harness.quit().expect("clean quit");
}

/// Steer mode: a follow-up typed during an auto-wake turn reaches that turn at its next request.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn steer_folds_follow_up_into_auto_wake_turn() {
    const MARKER: &str = "STEER_WAKE_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    seed_ui_config(&content, "follow_up_behavior = \"steer\"");
    let bg_args = json!({
        "command": "/bin/sleep 6",
        "description": "wake trigger",
        "is_background": true
    })
    .to_string();
    let _bg = expect_tool_turn(&content, "call_steer_bg", "run_terminal_command", bg_args);
    content.set_response("TURN1_SETTLED");

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
        .wait_for_text("TURN1_SETTLED", Duration::from_secs(45))
        .expect("turn 1 settled");

    let hold_args = json!({
        "command": "/bin/sleep 8",
        "description": "wake hold"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_steer_wake_hold",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("WAKE_SETTLED");

    harness
        .wait_for_text("wake hold", Duration::from_secs(30))
        .unwrap_or_else(|_| {
            panic!(
                "auto-wake hold never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    harness
        .inject_keys(format!("{MARKER} during wake").as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit follow-up");
    harness
        .wait_for_text("WAKE_SETTLED", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("wake never settled; screen:\n{}", harness.screen_contents()));
    harness.update(Duration::from_secs(3));
    assert_folded_once(&content, &harness, MARKER);
    harness.quit().expect("clean quit");
}
