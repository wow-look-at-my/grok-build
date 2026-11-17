//! PTY: `/goal` starts in a session that Shift+Tab put in explore mode.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::{InferenceEndpoint, InferenceRequestMatcher};

const ROUND_ONE: &str = "EXPLORE_GOAL_ROUND_MARK";
const ROUND_TWO: &str = "EXPLORE_GOAL_SECOND_MARK";
const TODO_TEXT: &str = "profile the slow build step";
const REFUSAL: &str = "Goal not started";
/// The last line of the goal-start reminder. Only the goal's first turn carries it.
const GOAL_START: &str = "Start now.";

/// The capture side call writes one todo.
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

/// The function tools a request offers, in either wire shape.
fn tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| {
            tool["name"]
                .as_str()
                .or_else(|| tool["function"]["name"].as_str())
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn goal_starts_in_explore_mode() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} turn done."));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &[("GROK_GOAL", "1")],
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("turn rendered");

    // Shift+Tab, one stop at a time, until the ring is on explore.
    for mode in ["Plan", "Auto", "Always-Approve", "Orchestrator", "Explore"] {
        harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
        harness
            .wait_for_text(
                &format!("Switched to mode: {mode}"),
                Duration::from_secs(10),
            )
            .unwrap_or_else(|e| panic!("the ring must reach {mode}: {e}"));
    }

    expect_todo_capture(&content);
    let _first = content.expect_agent_turn("goal round", ROUND_ONE);
    // The second round stays open, so the first round stays on the screen.
    let second = content.expect_agent_turn_blocked("second round", ROUND_TWO);
    content.set_response("EXPLORE_GOAL_FALLBACK");

    harness
        .inject_keys(b"/goal --lite find why the build is slow")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let started = wait_until(Duration::from_secs(90), || {
        harness.update(Duration::from_millis(100));
        harness.screen_contents().contains(ROUND_ONE)
    });
    let screen = harness.screen_contents();
    assert!(
        started,
        "the goal's first round must run in explore mode\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        !screen.contains(REFUSAL),
        "explore mode must not refuse a goal\nscreen:\n{screen}"
    );

    // The goal's first turn ran as explore: a todo list, no shell, no edit tool.
    let goal_turn_tools = content
        .request_bodies()
        .iter()
        .find(|body| body.to_string().contains(GOAL_START))
        .map(tool_names)
        .unwrap_or_else(|| {
            panic!(
                "a request must carry the goal-start reminder\n{}",
                dump_non_system_messages(&content.request_bodies())
            )
        });
    assert!(
        goal_turn_tools.iter().any(|name| name == "todo_write"),
        "explore keeps a todo list: {goal_turn_tools:?}"
    );
    for absent in ["run_terminal_command", "search_replace"] {
        assert!(
            !goal_turn_tools.iter().any(|name| name == absent),
            "the goal turn must run as explore, which has no `{absent}`: {goal_turn_tools:?}"
        );
    }

    // The captured item is on the session's list: the round end sends it back to the model.
    let listed = wait_until(Duration::from_secs(60), || {
        harness.update(Duration::from_millis(100));
        all_user_message_blobs(&content)
            .iter()
            .any(|blob| blob.contains(TODO_TEXT))
    });
    assert!(
        listed,
        "the model must be shown the captured todo\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        !harness.screen_contents().contains(REFUSAL),
        "explore mode must not refuse a goal\nscreen:\n{}",
        harness.screen_contents()
    );

    second.release();
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
