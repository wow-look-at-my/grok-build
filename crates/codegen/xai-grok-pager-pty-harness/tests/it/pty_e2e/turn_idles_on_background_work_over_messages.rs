//! PTY, over the Messages API: a Claude-shaped turn starts a background command, ends on its message, and is woken
//! by the completion.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;
use xai_grok_test_support::{Conversation, MockToolCall, Tool};

const NARRATION: &str = "MSG_NARRATION_MARK the build starts in the background";
const WAIT_MESSAGE: &str = "MSG_WAIT_MARK the build runs in the background";
const WAKE_DONE: &str = "MSG_WAKE_MARK the build passed";
const TURN_END_ROW: &str = "Worked for";

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

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn messages_turn_ends_on_its_message_while_background_work_runs() {
    let content = ContentController::start().await.expect("start content");
    use_messages_model(&content);
    let build_flag = content.home().join("messages_wait_build_flag");
    content.server().set_keep_requests(true);
    // The model thinks, writes text, and starts the build in one response. It then thinks again and sends its message.
    content.server().set_conversations(vec![
        Conversation::nth(1)
            .calls([MockToolCall::new(
                Tool::Shell,
                json!({
                    "command": format!(
                        "while [ ! -e {} ]; do /bin/sleep 0.2; done",
                        build_flag.display()
                    ),
                    "description": "flag-gated build",
                    "is_background": true
                }),
            )
            .thinking("the build is slow, so it runs in the background")
            .narrated(NARRATION)])
            .reasoning("the user knows the build runs; the completion message wakes me")
            .reply(WAIT_MESSAGE)
            .reasoning("the completion message says the build passed")
            .reply(WAKE_DONE),
    ]);

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

    // The turn ends on the model's message while the build still runs.
    let ended = wait_until(Duration::from_secs(60), || {
        harness.update(Duration::from_millis(100));
        let screen = harness.screen_contents();
        screen.contains(WAIT_MESSAGE) && screen.contains(TURN_END_ROW)
    });
    let screen = harness.screen_contents();
    assert!(
        ended,
        "the turn must end on its message while the build runs\nscreen:\n{screen}\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        row_of(&screen, NARRATION) < row_of(&screen, WAIT_MESSAGE)
            && row_of(&screen, NARRATION).is_some(),
        "the text before the tool call must show above the message that ends the turn\nscreen:\n{screen}"
    );
    let requests_at_idle = content.server().messages_request_count();

    // The session stays idle while the build runs.
    harness.update(Duration::from_secs(3));
    let screen = harness.screen_contents();
    assert!(!build_flag.exists(), "the test releases the build");
    assert_eq!(
        content.server().messages_request_count(),
        requests_at_idle,
        "no model request may be made while the session waits\nscreen:\n{screen}"
    );
    assert!(
        !screen.contains(WAKE_DONE),
        "the wake turn must not run before the build ends\nscreen:\n{screen}"
    );

    // The completion message wakes the session.
    std::fs::write(&build_flag, b"done").expect("release the build");
    harness
        .wait_for_text(WAKE_DONE, Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "the completion must wake the session\nscreen:\n{}\n{}",
                harness.screen_contents(),
                dump_non_system_messages(&content.request_bodies())
            )
        });
    let screen = harness.screen_contents();
    assert!(
        all_user_message_blobs(&content)
            .iter()
            .any(|blob| blob.contains("Background task") && blob.contains("completed (")),
        "the wake turn must carry the completion message\n{}",
        dump_non_system_messages(&content.request_bodies())
    );
    assert!(
        row_of(&screen, WAIT_MESSAGE) < row_of(&screen, WAKE_DONE),
        "the wake reply must show below the wait message\nscreen:\n{screen}"
    );
    assert!(
        content.server().script_violations().is_empty(),
        "the script must be served in order: {:?}",
        content.server().script_violations()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{screen}"
    );
    harness.quit().expect("clean quit");
}
