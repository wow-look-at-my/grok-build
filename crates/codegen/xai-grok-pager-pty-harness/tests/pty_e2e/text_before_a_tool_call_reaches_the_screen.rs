//! PTY: text the model writes in the same response as a tool call reaches the
//! screen. The mock streams the text and then the call.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;
use xai_grok_test_support::{Conversation, MockToolCall, Tool};

const TURN_TIMEOUT: Duration = Duration::from_secs(60);

/// A Claude model behind a Messages provider that points at the mock.
fn use_messages_model(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.claude]\nbase_url = \"{}\"\napi_backend = \"messages\"\napi_key = \"sk-mock\"\nmodels_autodetect = false\n\n[model.sonnet]\nmodel = \"claude-sonnet-5-5\"\nmodel_provider = \"claude\"\n\n[models]\ndefault = \"sonnet\"\n",
            content.url()
        ),
    )
    .expect("write config.toml");
}

async fn narration_reaches_the_screen(
    call: MockToolCall,
    narration: &str,
    done: &str,
    over_messages: bool,
) {
    let conversations = vec![Conversation::nth(1).calls([call]).reply(done)];
    narration_reaches_the_screen_in(conversations, narration, done, over_messages).await;
}

async fn narration_reaches_the_screen_in(
    conversations: Vec<Conversation>,
    narration: &str,
    done: &str,
    over_messages: bool,
) {
    let content = ContentController::start().await.expect("start content");
    if over_messages {
        use_messages_model(&content);
    }
    content.server().set_conversations(conversations);
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
    if over_messages {
        assert!(
            content.server().messages_request_count() > 0,
            "the turn never used the Messages endpoint"
        );
    }
}

fn shell_call() -> MockToolCall {
    MockToolCall::new(
        Tool::Shell,
        json!({ "command": "true", "description": "no-op" }),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_shell_call_reaches_the_screen() {
    narration_reaches_the_screen(
        shell_call().narrated("NARRATION_BEFORE_SHELL checking now"),
        "NARRATION_BEFORE_SHELL",
        "SHELL_TURN_DONE",
        false,
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
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_after_thinking_before_a_shell_call_reaches_the_screen_over_messages() {
    narration_reaches_the_screen(
        shell_call()
            .thinking("weighing the forecast")
            .narrated("NARRATION_AFTER_THINKING checking now"),
        "NARRATION_AFTER_THINKING",
        "MESSAGES_SHELL_DONE",
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_after_thinking_before_an_enter_plan_mode_call_reaches_the_screen_over_messages() {
    narration_reaches_the_screen(
        MockToolCall::new(Tool::EnterPlanMode, json!({}))
            .thinking("weighing the forecast")
            .narrated("NARRATION_AFTER_THINKING_PLAN let me plan this"),
        "NARRATION_AFTER_THINKING_PLAN",
        "MESSAGES_PLAN_DONE",
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_todo_write_reaches_the_screen() {
    narration_reaches_the_screen(
        MockToolCall::new(
            Tool::Todo,
            json!({ "todos": [{ "id": "1", "content": "answer the user" }] }),
        )
        .narrated("NARRATION_BEFORE_TODO tracking this"),
        "NARRATION_BEFORE_TODO",
        "TODO_TURN_DONE",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_blocking_subagent_call_reaches_the_screen() {
    let spawn = MockToolCall::new(
        Tool::Task,
        json!({
            "description": "helper",
            "prompt": "CHILD_PROMPT answer briefly",
            "subagent_type": "general-purpose",
        }),
    )
    .narrated("NARRATION_BEFORE_SUBAGENT delegating this");
    narration_reaches_the_screen_in(
        vec![
            Conversation::nth(1)
                .calls([spawn])
                .reply("SUBAGENT_TURN_DONE"),
            Conversation::nth(2).reply("CHILD_DONE"),
        ],
        "NARRATION_BEFORE_SUBAGENT",
        "SUBAGENT_TURN_DONE",
        false,
    )
    .await;
}
