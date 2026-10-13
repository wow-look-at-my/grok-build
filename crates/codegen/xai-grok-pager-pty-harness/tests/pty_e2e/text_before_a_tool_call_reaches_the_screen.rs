//! PTY: text the model writes in the same response as a tool call reaches the
//! screen. The mock streams the text and then the call.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;
use xai_grok_test_support::{Conversation, MockToolCall, Tool};

const TURN_TIMEOUT: Duration = Duration::from_secs(60);

async fn narration_reaches_the_screen(call: MockToolCall, narration: &str, done: &str) {
    let content = ContentController::start().await.expect("start content");
    content
        .server()
        .set_conversations(vec![Conversation::nth(1).calls([call]).reply(done)]);
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &[],
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
        .wait_for_text(done, TURN_TIMEOUT)
        .expect("the turn must finish");
    let screen = harness.screen_contents();
    assert!(
        screen.contains(narration),
        "the text written before the tool call never reached the screen:\n{screen}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_shell_call_reaches_the_screen() {
    narration_reaches_the_screen(
        MockToolCall::new(
            Tool::Shell,
            json!({ "command": "true", "description": "no-op" }),
        )
        .narrated("NARRATION_BEFORE_SHELL checking now"),
        "NARRATION_BEFORE_SHELL",
        "SHELL_TURN_DONE",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_an_enter_plan_mode_call_reaches_the_screen() {
    narration_reaches_the_screen(
        MockToolCall::new(Tool::EnterPlanMode, json!({}))
            .narrated("NARRATION_BEFORE_PLAN_MODE let me plan this"),
        "NARRATION_BEFORE_PLAN_MODE",
        "PLAN_TURN_DONE",
    )
    .await;
}
