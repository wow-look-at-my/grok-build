// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Spawn the pager on `content` and submit the first prompt.
#[cfg(unix)]
fn spawn_and_submit(content: &ContentController) -> PtyHarness {
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
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
}

/// Type `text` and press Enter once.
#[cfg(unix)]
fn type_follow_up(harness: &mut PtyHarness, text: &str) {
    harness
        .inject_keys(text.as_bytes())
        .expect("type follow-up");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit follow-up");
}

/// Script a parent that spawns one subagent, then answers `done`. The child holds until `flag` exists.
#[cfg(unix)]
fn script_subagent_turn(
    content: &ContentController,
    background: bool,
    flag: &std::path::Path,
    done: &str,
    replies: usize,
) {
    use xai_grok_test_support::{Conversation, MockToolCall, Tool};
    let mut parent = Conversation::nth(1).calls([MockToolCall::new(
        Tool::Task,
        json!({
            "description": "Build the project",
            "prompt": "HOLDING_CHILD_PROMPT wait for the flag",
            "subagent_type": "general-purpose",
            "background": background,
        }),
    )]);
    for _ in 0..replies {
        parent = parent.reply(done);
    }
    content.server().set_conversations(vec![
        parent,
        Conversation::nth(2)
            .calls([MockToolCall::new(
                Tool::Shell,
                json!({
                    "command": format!(
                        "while [ ! -e {} ]; do /bin/sleep 0.2; done",
                        flag.display()
                    ),
                    "description": "hold until released",
                }),
            )])
            .reply("CHILD_DONE"),
    ]);
}

/// Poll for `marker` in a request body, release the child, then assert the marker arrived in time.
#[cfg(unix)]
fn assert_reaches_model_before_release(
    content: &ContentController,
    harness: &PtyHarness,
    marker: &str,
    flag: &std::path::Path,
) {
    // Only a parent request counts. The parent's history holds the spawn call `call_mock_1_1`, and the child's does not.
    let reached = poll_for(Duration::from_secs(20), || {
        content
            .request_bodies()
            .iter()
            .any(|b| {
                let body = b.to_string();
                body.contains(marker) && body.contains("call_mock_1_1")
            })
            .then_some(())
    });
    let screen = harness.screen_contents();
    let _ = std::fs::write(flag, "");
    assert!(
        reached.is_some(),
        "the follow-up sat behind the subagent\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
}

/// Queue mode: a message typed while a foreground subagent runs reaches the model before the subagent ends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_a_foreground_subagent() {
    const MARKER: &str = "QUEUE_FG_TASK_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let flag = content.home().join("fg_hold_flag");
    script_subagent_turn(&content, false, &flag, "FG_PARENT_DONE", 2);

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold until released", Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the child never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{MARKER} tests are a waste of time"));
    assert_reaches_model_before_release(&content, &harness, MARKER, &flag);
    harness.quit().expect("clean quit");
}

/// Queue mode: a message typed while a background subagent runs after the turn ended starts a turn at once.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_while_a_background_subagent_runs() {
    const MARKER: &str = "QUEUE_BG_TASK_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let flag = content.home().join("bg_hold_flag");
    script_subagent_turn(&content, true, &flag, "BG_PARENT_DONE", 3);

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("BG_PARENT_DONE", Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the parent turn never ended; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness
        .wait_for_text("hold until released", Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the child never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{MARKER} tests are a waste of time"));
    assert_reaches_model_before_release(&content, &harness, MARKER, &flag);
    harness.quit().expect("clean quit");
}

/// Wait until a request body carries `marker`, with no further key press.
#[cfg(unix)]
fn assert_reaches_model(
    content: &ContentController,
    harness: &PtyHarness,
    marker: &str,
    within: Duration,
) {
    let reached = poll_for(within, || {
        content
            .request_bodies()
            .iter()
            .any(|b| b.to_string().contains(marker))
            .then_some(())
    });
    assert!(
        reached.is_some(),
        "the follow-up never reached the model on its own\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );
}

/// Queue mode: a follow-up typed while a foreground tool runs goes into the request after that tool.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_folds_follow_up_typed_during_a_tool() {
    const MARKER: &str = "QUEUE_TOOL_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_hold",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_TOOL_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{MARKER} also check this"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(30));
    harness
        .wait_for_text("QUEUE_TOOL_SETTLED", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the turn never settled; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_secs(2));
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(MARKER))
        .expect("a carrier");
    assert!(
        first.to_string().contains("call_queue_hold")
            && !first.to_string().contains("QUEUE_TOOL_SETTLED"),
        "the first request that carries the follow-up must be the one after the tool\n{}",
        dump_non_system_messages(&bodies)
    );
    assert_eq!(
        block_lines_containing(&harness, MARKER),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: a follow-up typed during a long foreground command reaches the model before the command exits.
/// The command moves to the background and keeps running.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_a_long_command() {
    const MARKER: &str = "QUEUE_LONG_COMMAND_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 60",
        "description": "long build"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_long",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_LONG_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("long build", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the command never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    let typed = Instant::now();
    type_follow_up(&mut harness, &format!("{MARKER} look at this now"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(20));
    assert!(
        typed.elapsed() < Duration::from_secs(30),
        "the follow-up waited for the 60s command"
    );
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(MARKER))
        .expect("a carrier")
        .to_string();
    assert!(
        first.contains("call_queue_long") && first.contains("background"),
        "the carrier must answer the command as moved to the background\n{}",
        dump_non_system_messages(&bodies)
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: a follow-up typed while the final answer streams runs as the next turn when that one ends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_final_answer() {
    const MARKER: &str = "QUEUE_FINAL_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    content.set_chunk_delay(Some(Duration::from_millis(150)));
    let _final = content.expect_agent_turn("final answer", slow_turn_text("FINALSTREAM"));
    content.set_response("QUEUE_AFTER_FINAL");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("FINALSTREAM", Duration::from_secs(45))
        .expect("final answer streaming");
    type_follow_up(&mut harness, &format!("{MARKER} one more"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(60));
    harness
        .wait_for_text("QUEUE_AFTER_FINAL", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("never answered; screen:\n{}", harness.screen_contents()));
    harness.update(Duration::from_secs(2));
    assert_eq!(
        block_lines_containing(&harness, MARKER),
        1,
        "the follow-up must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: follow-ups typed during one tool both reach the model with one Enter each.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_delivers_two_follow_ups_typed_during_a_tool() {
    const FIRST: &str = "QUEUE_TWO_FIRST_XYZ";
    const SECOND: &str = "QUEUE_TWO_SECOND_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_two",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_TWO_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{FIRST} one"));
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{SECOND} two"));

    assert_reaches_model(&content, &harness, FIRST, Duration::from_secs(30));
    assert_reaches_model(&content, &harness, SECOND, Duration::from_secs(30));
    harness.quit().expect("clean quit");
}

/// Returns once the wait turn has been submitted.
#[cfg(unix)]
fn start_wait_on_background_command(
    content: &ContentController,
    harness: &mut PtyHarness,
    wait_tool: &str,
    wait_args: impl Fn(&str) -> serde_json::Value,
) {
    let bg_args = json!({
        "command": "/bin/sleep 40",
        "description": "long task",
        "background": true
    })
    .to_string();
    let _bg = expect_tool_turn(content, "call_queue_bg", "run_terminal_command", bg_args);
    content.set_response("QUEUE_TURN1_SETTLED");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text("QUEUE_TURN1_SETTLED", Duration::from_secs(45))
        .expect("turn 1 settled");
    let task_id = poll_for(Duration::from_secs(10), || {
        content
            .request_bodies()
            .iter()
            .find_map(|b| extract_task_id(&b.to_string()))
    })
    .expect("task id in a request body");
    let _wait = expect_tool_turn(
        content,
        "call_queue_wait",
        wait_tool,
        wait_args(&task_id).to_string(),
    );
    content.set_response("QUEUE_WAIT_SETTLED");
    harness
        .inject_keys(b"now wait\r")
        .expect("submit wait prompt");
    harness.update(Duration::from_secs(4));
}

/// Spawn the pager on `content` without submitting a prompt.
#[cfg(unix)]
fn spawn_idle(content: &ContentController) -> PtyHarness {
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
        &["--yolo", "--trust"],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
}

/// Queue mode: a message typed while the turn blocks on `wait_commands_or_subagents` does not wait for the task.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_a_task_wait() {
    const MARKER: &str = "QUEUE_WAIT_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let mut harness = spawn_idle(&content);
    start_wait_on_background_command(
        &content,
        &mut harness,
        "wait_commands_or_subagents",
        |id| json!({ "task_ids": [id] }),
    );
    type_follow_up(&mut harness, &format!("{MARKER} during wait"));
    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(20));
    harness.quit().expect("clean quit");
}

/// Queue mode: a message typed while the turn blocks on a task-output read with a timeout does not wait for the task.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_sends_follow_up_typed_during_a_task_output_wait() {
    const MARKER: &str = "QUEUE_OUTPUT_WAIT_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let mut harness = spawn_idle(&content);
    start_wait_on_background_command(
        &content,
        &mut harness,
        "get_command_or_subagent_output",
        |id| json!({ "task_ids": [id], "timeout_ms": 60000 }),
    );
    type_follow_up(&mut harness, &format!("{MARKER} during wait"));
    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(20));
    harness.quit().expect("clean quit");
}

/// Queue mode: a skill invocation typed while a tool runs goes into the request after that tool, expanded.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_folds_skill_typed_during_a_tool() {
    const SKILL_BODY: &str = "QUEUE_SKILL_BODY_XYZ";
    let content = ContentController::start().await.expect("start content");
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    git2::Repository::init(workspace.path()).expect("git init workspace");
    let skill_dir = workspace
        .path()
        .join(".grok")
        .join("skills")
        .join("queue-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: queue-skill\ndescription: Queue skill.\n---\n\n{SKILL_BODY} do the thing.\n"
        ),
    )
    .expect("write SKILL.md");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_skill",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_SKILL_SETTLED");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        Some(workspace.path()),
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
    type_follow_up(&mut harness, "/queue-skill go");

    assert_reaches_model(&content, &harness, SKILL_BODY, Duration::from_secs(30));
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(SKILL_BODY))
        .expect("a carrier");
    assert!(
        first.to_string().contains("call_queue_skill")
            && !first.to_string().contains("QUEUE_SKILL_SETTLED"),
        "the skill must be folded into the request after the tool\n{}",
        dump_non_system_messages(&bodies)
    );
    harness.quit().expect("clean quit");
}

/// Queue mode: messages typed all through a long turn each reach that turn with one Enter.
/// Every request is stalled, so some messages land while the model is still answering.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_delivers_every_message_typed_across_a_long_turn() {
    use xai_grok_test_support::{Conversation, MockToolCall, Tool};
    const DONE: &str = "SOAK_TURN_DONE";
    let content = ContentController::start().await.expect("start content");
    let sleep = |n: u32| {
        MockToolCall::new(
            Tool::Shell,
            json!({ "command": "/bin/sleep 4", "description": format!("soak step {n}") }),
        )
    };
    content.server().set_conversations(vec![
        Conversation::nth(1)
            .calls([sleep(1), sleep(2), sleep(3)])
            .stall(Duration::from_secs(2))
            .reply(DONE),
    ]);

    let mut harness = spawn_and_submit(&content);
    let markers: Vec<String> = (1..=5).map(|n| format!("SOAK_MARKER_{n}_XYZ")).collect();
    harness.update(Duration::from_secs(1));
    for marker in &markers {
        type_follow_up(&mut harness, &format!("{marker} note"));
        harness.update(Duration::from_millis(2700));
    }
    harness
        .wait_for_text(DONE, Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the turn never ended; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_secs(2));

    let bodies = content.request_bodies();
    for marker in &markers {
        let first = bodies
            .iter()
            .find(|b| b.to_string().contains(marker.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "{marker} never reached the model\n{}",
                    dump_non_system_messages(&bodies)
                )
            });
        assert!(
            !first.to_string().contains(DONE),
            "{marker} waited for the turn to end\n{}",
            dump_non_system_messages(&bodies)
        );
    }
    harness.quit().expect("clean quit");
}

/// Queue mode: a slash command typed mid-turn does not hold back the plain message typed after it.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queue_mode_slash_command_does_not_hold_back_later_text() {
    const MARKER: &str = "QUEUE_AFTER_SLASH_MARKER_XYZ";
    let content = ContentController::start().await.expect("start content");
    let hold_args = json!({
        "command": "/bin/sleep 6",
        "description": "hold turn"
    })
    .to_string();
    let _hold = expect_tool_turn(
        &content,
        "call_queue_slash",
        "run_terminal_command",
        hold_args,
    );
    content.set_response("QUEUE_SLASH_SETTLED");

    let mut harness = spawn_and_submit(&content);
    harness
        .wait_for_text("hold turn", Duration::from_secs(45))
        .unwrap_or_else(|_| {
            panic!(
                "the hold tool never started; screen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, "/frobnicate later");
    harness.update(Duration::from_millis(500));
    type_follow_up(&mut harness, &format!("{MARKER} meanwhile"));

    assert_reaches_model(&content, &harness, MARKER, Duration::from_secs(30));
    let bodies = content.request_bodies();
    let first = bodies
        .iter()
        .find(|b| b.to_string().contains(MARKER))
        .expect("a carrier");
    assert!(
        first.to_string().contains("call_queue_slash")
            && !first.to_string().contains("QUEUE_SLASH_SETTLED"),
        "the message must reach the turn it was typed in, not wait for it to end\n{}",
        dump_non_system_messages(&bodies)
    );
    harness.quit().expect("clean quit");
}
