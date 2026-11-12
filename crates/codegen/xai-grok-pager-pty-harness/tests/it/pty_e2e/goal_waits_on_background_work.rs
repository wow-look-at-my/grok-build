//! PTY: a goal turn that ends while a background command runs shows its
//! message and idles.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::{InferenceEndpoint, InferenceRequestMatcher};

const WAIT_MESSAGE: &str = "GOAL_WAIT_MARK the build runs in the background";
const WAKE_DONE: &str = "GOAL_WAKE_MARK the build passed";
const WAITING_ROW: &str = "Goal is waiting on 1 background task";
const CHECK_ROW: &str = "Goal check:";
const DIRECTIVE_ROW: &str = "Sent to the model · goal";
const GOAL_COMPLETE: &str = "Goal complete.";
/// The first words of the evaluator's system prompt. Only a goal check request carries them.
const EVALUATOR_PROMPT: &str = "hidden completion evaluator for an autonomous coding goal";
const TODO_TEXT: &str = "build the project";

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

fn goal_check_requests(content: &ContentController) -> usize {
    content
        .request_bodies()
        .iter()
        .filter(|body| body.to_string().contains(EVALUATOR_PROMPT))
        .count()
}

/// The distinct user messages that are a goal continuation directive or a todo bounce.
fn goal_directives(content: &ContentController) -> std::collections::BTreeSet<String> {
    all_user_message_blobs(content)
        .into_iter()
        .filter(|blob| {
            blob.contains("Goal NOT complete") || blob.contains("You have incomplete todos")
        })
        .collect()
}

/// The id the capture gave its todo, read from a `todo_write` result the model got.
fn captured_todo_id(content: &ContentController) -> Option<String> {
    const PREFIX: &str = "capture-";
    content.request_bodies().iter().find_map(|body| {
        let text = body.to_string();
        text.match_indices(PREFIX).find_map(|(at, _)| {
            let hex: String = text[at + PREFIX.len()..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .collect();
            (hex.len() == 6).then(|| format!("{PREFIX}{hex}"))
        })
    })
}

/// A goal starts only with a todo, so the capture side call writes one.
fn expect_todo_capture(content: &ContentController) {
    let capture =
        json!({"todos": [{"id": "c1", "content": TODO_TEXT, "status": "pending"}]}).to_string();
    for (endpoint, events) in [
        (
            InferenceEndpoint::Responses,
            sse::responses_api_reasoning_then_tool_call_events(
                "one step",
                "call_capture_1",
                "todo_write",
                &capture,
                "test-model",
            ),
        ),
        (
            InferenceEndpoint::ChatCompletions,
            sse::chat_completions_reasoning_then_tool_call_events(
                "one step",
                "call_capture_1",
                "todo_write",
                &capture,
                "test-model",
            ),
        ),
    ] {
        let _ = content.expect_response(
            format!("todo capture ({endpoint:?})"),
            InferenceRequestMatcher::auxiliary(endpoint),
            ScriptedResponse::sse(events),
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn goal_turn_idles_on_background_work_and_is_checked_after_the_wake() {
    let content = ContentController::start().await.expect("start content");
    let build_flag = content.home().join("goal_wait_build_flag");
    expect_todo_capture(&content);

    // The first turn: read the todo list, start the build in the background, say what it waits for.
    let _list = expect_tool_turn(
        &content,
        "call_goal_wait_list",
        "todo_write",
        json!({ "todos": [] }).to_string(),
    );
    let _build = expect_tool_turn(
        &content,
        "call_goal_wait_build",
        "run_terminal_command",
        json!({
            "command": format!(
                "while [ ! -e {} ]; do /bin/sleep 0.2; done",
                build_flag.display()
            ),
            "description": "flag-gated build",
            "is_background": true
        })
        .to_string(),
    );
    let _wait = content.expect_agent_turn("wait message", WAIT_MESSAGE);
    // Every other request gets a "complete" verdict. A goal check that runs too early then ends the goal on screen.
    content.set_response(
        json!({
            "decision": "candidate_complete",
            "evidence": "the build passed",
            "next_step": "none",
            "blocker_key": ""
        })
        .to_string(),
    );

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        160,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &[("GROK_GOAL", "1")],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(b"/goal --lite build the project")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    // The turn ends with the model's message on screen, and the goal says what it waits for.
    let idle = wait_until(Duration::from_secs(90), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(WAIT_MESSAGE) && screen.contains(WAITING_ROW)
    });
    assert!(
        idle,
        "the waiting turn must end with its message and the goal's wait line\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );
    let directives_at_idle = goal_directives(&content);

    // The session stays idle while the build runs: no goal check and no directive.
    harness.update(Duration::from_secs(4));
    let screen = harness.screen_contents();
    assert!(
        !build_flag.exists(),
        "the test releases the build, nothing else does"
    );
    assert_eq!(
        goal_check_requests(&content),
        0,
        "no goal check may run while the background command runs\nscreen:\n{screen}"
    );
    assert_eq!(
        goal_directives(&content),
        directives_at_idle,
        "no goal directive may reach the model while the background command runs\nscreen:\n{screen}"
    );
    assert!(
        row_of(&screen, CHECK_ROW).is_none() && row_of(&screen, DIRECTIVE_ROW).is_none(),
        "the idle goal shows no check row and no directive row\nscreen:\n{screen}"
    );
    assert!(
        !screen.contains(GOAL_COMPLETE),
        "the goal must stay active while the background command runs\nscreen:\n{screen}"
    );

    // The wake turn: finish the todo, then report.
    let todo_id = captured_todo_id(&content).unwrap_or_else(|| {
        panic!(
            "the todo list read must show the captured id\n{}",
            dump_non_system_messages(&content.request_bodies())
        )
    });
    let _finish = expect_tool_turn(
        &content,
        "call_goal_wait_finish",
        "todo_write",
        json!({ "todos": [{ "id": todo_id, "status": "completed" }] }).to_string(),
    );
    let _report = content.expect_agent_turn("wake report", WAKE_DONE);
    std::fs::write(&build_flag, b"done").expect("release the build");

    let completed = wait_until(Duration::from_secs(90), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(WAKE_DONE) && screen.contains(GOAL_COMPLETE)
    });
    let screen = harness.screen_contents();
    assert!(
        completed,
        "the completion must wake the session, and the goal check must end the goal\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        all_user_message_blobs(&content)
            .iter()
            .any(|blob| blob.contains("Background task") && blob.contains("completed (")),
        "the wake turn must carry the completion message\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert_eq!(
        goal_check_requests(&content),
        1,
        "the goal check runs one time, at the end of the wake turn\nscreen:\n{screen}"
    );
    let wait = row_of(&screen, WAIT_MESSAGE);
    let wake = row_of(&screen, WAKE_DONE);
    let check = row_of(&screen, CHECK_ROW);
    assert!(
        wait.is_some() && wake.is_some() && check.is_some(),
        "the wait message, the wake report and the goal check must show\nscreen:\n{screen}"
    );
    assert!(
        wait < wake && wake < check,
        "order must be: wait message, wake report, goal check\nscreen:\n{screen}"
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{screen}"
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}

/// The model starts a background command and then only thinks: its reply has no message and no tool call.
/// That reply does not end the turn. The model is asked again, and the turn ends with the message it then sends.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn thinking_only_reply_after_a_background_start_is_asked_again() {
    const THOUGHT: &str = "GOAL_THOUGHT_MARK the user already knows that the build runs";
    let content = ContentController::start().await.expect("start content");
    let build_flag = content.home().join("goal_think_build_flag");
    expect_todo_capture(&content);

    let _build = expect_tool_turn(
        &content,
        "call_goal_think_build",
        "run_terminal_command",
        json!({
            "command": format!(
                "while [ ! -e {} ]; do /bin/sleep 0.2; done",
                build_flag.display()
            ),
            "description": "flag-gated build",
            "is_background": true
        })
        .to_string(),
    );
    let thinking_only = content.expect_agent_turn_with_responses(
        "thinking only",
        ScriptedResponse::sse(sse::responses_api_reasoning_only_events(
            THOUGHT,
            "test-model",
        )),
        ScriptedResponse::sse(sse::chat_completion_script_with_reasoning(
            THOUGHT,
            "",
            "test-model",
        )),
    );
    let _wait = content.expect_agent_turn("wait message", WAIT_MESSAGE);
    content.set_response("GOAL_THINK_FALLBACK");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        160,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &[("GROK_GOAL", "1")],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(b"/goal --lite build the project")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let idle = wait_until(Duration::from_secs(120), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(WAIT_MESSAGE) && screen.contains(WAITING_ROW)
    });
    let screen = harness.screen_contents();
    assert!(
        idle,
        "the turn must end with a message after the thinking-only reply\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        thinking_only.is_satisfied(),
        "the thinking-only reply must have been served before the message\nscreen:\n{screen}"
    );
    assert!(
        !build_flag.exists() && goal_check_requests(&content) == 0,
        "the goal waits on the build with no goal check\nscreen:\n{screen}"
    );
    assert!(
        !screen.contains("GOAL_THINK_FALLBACK"),
        "the turn ends on the wait message: no further model reply follows it\nscreen:\n{screen}"
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
