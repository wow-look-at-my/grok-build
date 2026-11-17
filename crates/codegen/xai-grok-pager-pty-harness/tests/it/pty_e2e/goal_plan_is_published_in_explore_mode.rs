//! PTY: in explore mode, a full goal's planner writes its plan and the goal goes on.
#[allow(unused_imports)]
use super::common::*;
use super::goal_starts_in_explore_mode::{pager_in_explore_mode, tool_names};
use xai_grok_pager_pty_harness::{
    InferenceEndpoint, InferenceExpectation, InferenceRequestMatcher,
};

const ROUND: &str = "EXPLORE_PLANNED_ROUND_MARK";
const PLAN_TODO: &str = "time each build step";
/// The first words of the planner's prompt. Only the planner subagent's requests carry them.
const PLANNER_PROMPT: &str = "Goal Plan Writer";
/// The words before the plan path in the planner's prompt.
const PLAN_PATH_LEAD: &str = "your only write is `";
/// The last line of the goal-start reminder. Only the goal's first turn carries it.
const GOAL_START: &str = "Start now.";
const NOT_SHOWN: [&str; 3] = ["Goal not started", "No plan was produced", "not allowed"];

/// One tool call, in the wire shape of `endpoint`.
fn tool_call(
    endpoint: InferenceEndpoint,
    call_id: &str,
    name: &str,
    args: &str,
) -> ScriptedResponse {
    match endpoint {
        InferenceEndpoint::ChatCompletions => ScriptedResponse::sse(
            chat_completions_tool_call_events_with_id(call_id, name, args),
        ),
        _ => ScriptedResponse::sse(responses_api_tool_call_events(call_id, name, args)),
    }
}

/// One text answer, in the wire shape of `endpoint`.
fn text(endpoint: InferenceEndpoint, text: &str) -> ScriptedResponse {
    match endpoint {
        InferenceEndpoint::ChatCompletions => {
            ScriptedResponse::sse(sse::chat_completion_script_exact(text, "test-model"))
        }
        _ => ScriptedResponse::sse(sse::responses_api_script_exact(text, "test-model")),
    }
}

/// Queue the planner subagent's next answer. The matcher wants the planner's
/// prompt in the request, so no other agent's request takes the answer.
fn planner_step(
    content: &ContentController,
    name: &str,
    blocked: bool,
    answer: impl Fn(InferenceEndpoint) -> ScriptedResponse,
) -> Vec<InferenceExpectation> {
    let mut steps = Vec::new();
    for endpoint in [
        InferenceEndpoint::Responses,
        InferenceEndpoint::ChatCompletions,
    ] {
        for matcher in [
            InferenceRequestMatcher::foreground(endpoint),
            InferenceRequestMatcher::auxiliary(endpoint),
        ] {
            let matcher = matcher.containing(PLANNER_PROMPT);
            let name = format!("planner {name} ({endpoint:?})");
            steps.push(if blocked {
                content.expect_response_blocked(name, matcher, answer(endpoint))
            } else {
                content.expect_response(name, matcher, answer(endpoint))
            });
        }
    }
    steps
}

/// The path the planner's prompt names as its one write.
fn plan_path(content: &ContentController) -> Option<String> {
    content.request_bodies().iter().find_map(|body| {
        let text = body.to_string();
        if !text.contains(PLANNER_PROMPT) {
            return None;
        }
        let rest = &text[text.find(PLAN_PATH_LEAD)? + PLAN_PATH_LEAD.len()..];
        Some(rest[..rest.find('`')?].to_owned())
    })
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn full_goal_plan_is_published_in_explore_mode() {
    let content = ContentController::start().await.expect("start content");
    let mut harness = pager_in_explore_mode(&content);
    content.set_response(ROUND);
    // The planner's first answer stays open until the test knows the plan path.
    let probe = planner_step(&content, "probe", true, |endpoint| {
        tool_call(
            endpoint,
            "call_plan_probe",
            "todo_write",
            &json!({ "todos": [] }).to_string(),
        )
    });

    harness
        .inject_keys(b"/goal find why the build is slow")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let mut path = None;
    let asked = wait_until(Duration::from_secs(30), || {
        harness.update(Duration::from_millis(100));
        path = plan_path(&content);
        path.is_some()
    });
    assert!(
        asked,
        "the planner subagent must run in explore mode\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );
    let path = path.expect("plan path");

    // The planner writes the plan with a tool it has, lists one step, and ends.
    let planner_tools = content
        .request_bodies()
        .iter()
        .find(|body| body.to_string().contains(PLANNER_PROMPT))
        .map(tool_names)
        .unwrap_or_default();
    let has = |name: &str| planner_tools.iter().any(|tool| tool == name);
    let (write_tool, write) = if has("run_terminal_command") {
        (
            "run_terminal_command",
            json!({
                "command": format!("printf '# Plan\\n\\n1. {PLAN_TODO}\\n' > '{path}'"),
                "description": "write the plan",
                "is_background": false
            }),
        )
    } else {
        assert!(
            has("write"),
            "the planner must have a tool that writes its plan: {planner_tools:?}"
        );
        (
            "write",
            json!({ "file_path": path, "content": format!("# Plan\n\n1. {PLAN_TODO}\n") }),
        )
    };
    let write = write.to_string();
    let _write = planner_step(&content, "write", false, |endpoint| {
        tool_call(endpoint, "call_plan_write", write_tool, &write)
    });
    let todos =
        json!({ "todos": [{"id": "p1", "content": PLAN_TODO, "status": "pending"}] }).to_string();
    let _todos = planner_step(&content, "todos", false, |endpoint| {
        tool_call(endpoint, "call_plan_todos", "todo_write", &todos)
    });
    let _done = planner_step(&content, "done", false, |endpoint| {
        text(endpoint, "The plan is written.")
    });
    for step in &probe {
        step.release();
    }

    // The plan is published and the goal's first turn reaches the model.
    let started = wait_until(Duration::from_secs(25), || {
        harness.update(Duration::from_millis(100));
        content
            .request_bodies()
            .iter()
            .any(|body| body.to_string().contains(GOAL_START))
    });
    let screen = harness.screen_contents();
    assert!(
        started,
        "the goal must go on after the planner in explore mode\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    for refusal in NOT_SHOWN {
        assert!(
            !screen.contains(refusal),
            "explore mode must not show `{refusal}`\nscreen:\n{screen}"
        );
    }

    // The planner's step is on the session's list: the model is shown it.
    let listed = wait_until(Duration::from_secs(10), || {
        harness.update(Duration::from_millis(100));
        all_user_message_blobs(&content)
            .iter()
            .any(|blob| !blob.contains(PLANNER_PROMPT) && blob.contains(PLAN_TODO))
    });
    assert!(
        listed,
        "the model must be shown the planner's step\nscreen:\n{}\n{}",
        harness.screen_contents(),
        dump_non_system_messages(&content.request_bodies())
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
