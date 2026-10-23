//! PTY: a goal round shows the evaluator call and the directive it sends the model.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::{InferenceEndpoint, InferenceRequestMatcher};

const ROUND_ONE: &str = "ROUND_ONE_MARK";
const ROUND_TWO: &str = "ROUND_TWO_MARK";
const NEXT_STEP: &str = "NEXT_STEP_MARK";
const CHECK_ROW: &str = "Goal check:";
const DIRECTIVE_ROW: &str = "Sent to the model · goal";

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

fn expect_verdict(content: &ContentController, name: &str, verdict: &serde_json::Value) {
    let text = verdict.to_string();
    for (endpoint, response) in [
        (
            InferenceEndpoint::Responses,
            ScriptedResponse::sse(sse::responses_api_script_exact(&text, "test-model")),
        ),
        (
            InferenceEndpoint::ChatCompletions,
            ScriptedResponse::sse(sse::chat_completion_script_exact(&text, "test-model")),
        ),
    ] {
        let _ = content.expect_response(
            format!("{name} ({endpoint:?})"),
            InferenceRequestMatcher::auxiliary(endpoint),
            response,
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn goal_round_shows_check_and_directive() {
    let content = ContentController::start().await.expect("start content");
    let _first = content.expect_agent_turn("first round", ROUND_ONE);
    expect_verdict(
        &content,
        "continue",
        &json!({
            "decision": "continue",
            "evidence": "the round only wrote a marker",
            "next_step": NEXT_STEP,
            "blocker_key": ""
        }),
    );
    let _second = content.expect_agent_turn("second round", ROUND_TWO);
    expect_verdict(
        &content,
        "complete",
        &json!({
            "decision": "candidate_complete",
            "evidence": "both rounds ran",
            "next_step": "none",
            "blocker_key": ""
        }),
    );
    content.set_response("GOAL_FALLBACK");

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
        .inject_keys(b"/goal --lite write two markers")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let shown = wait_until(Duration::from_secs(90), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(ROUND_TWO) && screen.contains(DIRECTIVE_ROW)
    });
    let screen = harness.screen_contents();
    assert!(
        shown,
        "the second round and the directive row must show; screen:\n{screen}"
    );
    let one = row_of(&screen, ROUND_ONE);
    let check = row_of(&screen, CHECK_ROW);
    let directive = row_of(&screen, DIRECTIVE_ROW);
    let two = row_of(&screen, ROUND_TWO);
    assert!(
        one.is_some() && check.is_some(),
        "the first round and the goal check must show; screen:\n{screen}"
    );
    assert!(
        one < check && check < directive && directive < two,
        "order must be: round one, goal check, directive, round two; screen:\n{screen}"
    );
    assert!(
        screen
            .lines()
            .skip(directive.unwrap_or(0))
            .any(|line| line.contains(NEXT_STEP)),
        "the directive row must show the text the model got; screen:\n{screen}"
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}

/// A full goal starts with a planner subagent. Its run has a row, so the wait before the first round is accounted for.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn full_goal_shows_planner_row() {
    let content = ContentController::start().await.expect("start content");
    content.set_response("PLANNER_FALLBACK");

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
        .inject_keys(b"/goal write two markers")
        .expect("type goal");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit goal");

    let shown = wait_until(Duration::from_secs(60), || {
        harness.update(Duration::from_millis(100));
        harness.screen_contents().contains("Goal planning:")
    });
    assert!(
        shown,
        "the planner run must have a row; screen:\n{}",
        harness.screen_contents()
    );

    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}
