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
    narration_reaches_the_screen_with(call, narration, done, over_messages, &[]).await;
}

async fn narration_reaches_the_screen_with(
    call: MockToolCall,
    narration: &str,
    done: &str,
    over_messages: bool,
    extra_args: &[&str],
) {
    let conversations = vec![Conversation::nth(1).calls([call]).reply(done)];
    narration_reaches_the_screen_in(conversations, narration, done, over_messages, extra_args)
        .await;
}

async fn narration_reaches_the_screen_in(
    conversations: Vec<Conversation>,
    narration: &str,
    done: &str,
    over_messages: bool,
    extra_args: &[&str],
) {
    let content = ContentController::start().await.expect("start content");
    if over_messages {
        use_messages_model(&content);
    }
    content.server().set_keep_requests(true);
    content.server().set_conversations(conversations);
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &pager_args(extra_args),
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
    let follow_up_carries_it = content
        .server()
        .request_bodies()
        .iter()
        .skip(1)
        .any(|body| body.to_string().contains(narration));
    assert!(
        follow_up_carries_it,
        "the request after the tool call did not carry the text the model wrote before it"
    );
    if over_messages {
        assert!(
            content.server().messages_request_count() > 0,
            "the turn never used the Messages endpoint"
        );
    }
}

/// The flags every test runs the pager with, then the mode under test.
fn pager_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["--yolo", "--trust"];
    args.extend_from_slice(extra);
    args
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
        &[],
    )
    .await;
}

fn slow_shell_call(seconds: u32) -> MockToolCall {
    MockToolCall::new(
        Tool::Shell,
        json!({ "command": format!("sleep {seconds}"), "description": "wait" }),
    )
}

async fn spawn_on_narrated_slow_call(
    content: &ContentController,
    narration: &str,
    replies: &[&str],
    seconds: u32,
) -> PtyHarness {
    let mut conversation =
        Conversation::nth(1).calls([slow_shell_call(seconds).narrated(narration)]);
    for reply in replies {
        conversation = conversation.reply(*reply);
    }
    content.server().set_conversations(vec![conversation]);
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_running_tool_stays_when_a_second_prompt_is_sent() {
    let content = ContentController::start().await.expect("start content");
    let narration = "NARRATION_BEFORE_SLOW_CALL looking into it";
    let mut harness = spawn_on_narrated_slow_call(&content, narration, &["TURN_DONE"], 4).await;
    harness
        .wait_for_text(narration, TURN_TIMEOUT)
        .expect("the text must show while the tool runs");
    harness
        .inject_keys(b"the same question again\r")
        .expect("send a second prompt mid-turn");
    harness
        .wait_for_text("TURN_DONE", TURN_TIMEOUT)
        .unwrap_or_else(|error| panic!("{error}\n{}", harness.screen_contents()));
    let screen = harness.screen_contents();
    assert!(
        screen.contains(narration),
        "the text written before the tool call vanished once a second prompt was sent:\n{screen}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_running_tool_stays_after_ctrl_c_and_a_new_prompt() {
    let content = ContentController::start().await.expect("start content");
    let narration = "NARRATION_BEFORE_SLOW_CALL looking into it";
    let mut harness =
        spawn_on_narrated_slow_call(&content, narration, &["AFTER_CANCEL_DONE"], 30).await;
    harness
        .wait_for_text(narration, TURN_TIMEOUT)
        .expect("the text must show while the tool runs");
    harness.inject_keys(keys::CTRL_C).expect("cancel the turn");
    harness.update(Duration::from_secs(2));
    let after_cancel = harness.screen_contents();
    assert!(
        after_cancel.contains(narration),
        "the text written before the tool call vanished when the turn was cancelled:\n{after_cancel}"
    );
    harness
        .inject_keys(b"answer me now\r")
        .expect("send a new prompt");
    harness
        .wait_for_text("AFTER_CANCEL_DONE", TURN_TIMEOUT)
        .unwrap_or_else(|error| panic!("{error}\n{}", harness.screen_contents()));
    // The new turn is anchored at the top, so the earlier turn sits above the viewport: scroll up to it.
    harness.inject_keys(b"\t").expect("focus the scrollback");
    for _ in 0..4 {
        harness.inject_keys(keys::PGUP).expect("page up");
        harness.update(Duration::from_millis(300));
    }
    let screen = harness.screen_contents();
    assert!(
        screen.contains(narration),
        "the text written before the tool call is gone after the new prompt, even scrolled up:\n{screen}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_stays_when_a_prompt_interrupts_the_stream_before_the_tool_call() {
    let content = ContentController::start().await.expect("start content");
    content.set_chunk_delay(Some(Duration::from_secs(3)));
    let narration = "NARRATION_BEFORE_INTERRUPT looking into it";
    let conversation = Conversation::nth(1)
        .calls([shell_call().narrated(narration)])
        .reply("FINAL_DONE");
    content.server().set_conversations(vec![conversation]);
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
        .wait_for_text(narration, TURN_TIMEOUT)
        .expect("the text must stream in");
    harness
        .inject_keys(b"the same question again\r")
        .expect("interrupt the stream with a second prompt");
    content.set_chunk_delay(None);
    harness
        .wait_for_text("FINAL_DONE", TURN_TIMEOUT)
        .unwrap_or_else(|error| panic!("{error}\n{}", harness.screen_contents()));
    harness.inject_keys(b"\t").expect("focus the scrollback");
    for _ in 0..4 {
        harness.inject_keys(keys::PGUP).expect("page up");
        harness.update(Duration::from_millis(300));
    }
    let screen = harness.screen_contents();
    assert!(
        screen.contains(narration),
        "the text vanished when a prompt interrupted the stream:\n{screen}"
    );
}

/// Each narration is before its own call and after the call before it, so those read top to bottom in the order written.
async fn successive_narrations_read_in_order(over_messages: bool, extra_args: &[&str]) {
    let content = ContentController::start().await.expect("start content");
    if over_messages {
        use_messages_model(&content);
    }
    let call = |narration: &str| {
        let call = shell_call().narrated(narration);
        if over_messages {
            call.thinking("deciding the next step")
        } else {
            call
        }
    };
    let narrations = [
        "NARRATION_ONE first look",
        "NARRATION_TWO second look",
        "NARRATION_THREE third look",
    ];
    content.server().set_conversations(vec![
        Conversation::nth(1)
            .calls(narrations.map(call))
            .reply("ALL_CALLS_DONE"),
    ]);
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &pager_args(extra_args),
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
        .wait_for_text("ALL_CALLS_DONE", TURN_TIMEOUT)
        .unwrap_or_else(|error| panic!("{error}\n{}", harness.screen_contents()));
    let screen = harness.screen_contents();
    let positions: Vec<Option<usize>> = narrations
        .iter()
        .map(|narration| screen.find(&narration[..13]))
        .collect();
    assert!(
        positions.iter().all(Option::is_some) && positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the narrations must all show, top to bottom in the order written ({positions:?}):\n{screen}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn successive_narrations_read_in_order_over_chat_completions() {
    successive_narrations_read_in_order(false, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn successive_narrations_read_in_order_over_messages_with_thinking() {
    successive_narrations_read_in_order(true, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn successive_narrations_read_in_order_inline() {
    successive_narrations_read_in_order(true, &["--no-alt-screen"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn markdown_text_before_a_shell_call_reaches_the_screen() {
    let narration = "\n\nForecast: **sunny**, high of $72 and low of $58.\n\n| day | high |\n|-----|------|\n| tue | 72 |\n\n```\nraw block\n```\n\n- first\n- NARRATION_MARKDOWN_END";
    narration_reaches_the_screen(
        shell_call().narrated(narration),
        "NARRATION_MARKDOWN_END",
        "MARKDOWN_TURN_DONE",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_shell_call_reaches_the_screen_inline() {
    narration_reaches_the_screen_with(
        shell_call().narrated("NARRATION_INLINE checking now"),
        "NARRATION_INLINE",
        "INLINE_TURN_DONE",
        false,
        &["--no-alt-screen"],
    )
    .await;
}

fn narration_before(call: MockToolCall, tag: &str) -> (MockToolCall, String) {
    let narration = format!("NARRATION_{tag}");
    (
        call.narrated(&format!("{narration} looking into it")),
        narration,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_before_a_read_or_search_call_reaches_the_screen() {
    let cases = [
        (
            "READ",
            MockToolCall::new(Tool::Read, json!({ "target_file": "/etc/hostname" })),
        ),
        (
            "GREP",
            MockToolCall::new(
                Tool::Grep,
                json!({ "pattern": "root", "path": "/etc/passwd" }),
            ),
        ),
    ];
    for (tag, call) in cases {
        let (call, narration) = narration_before(call, tag);
        for over_messages in [false, true] {
            narration_reaches_the_screen(
                call.clone(),
                &narration,
                &format!("{tag}_DONE"),
                over_messages,
            )
            .await;
        }
    }
}
