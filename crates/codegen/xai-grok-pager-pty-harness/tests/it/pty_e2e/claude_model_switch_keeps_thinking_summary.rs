//! PTY, over the Messages API: `/model` switches a session between Claude
//! models.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;
use xai_grok_test_support::Conversation;

const OPUS: &str = "claude-opus-5-5";
const FABLE: &str = "claude-fable-5-2";
const OPUS_THOUGHT: &str = "OPUS_THOUGHT_MARK the parser needs a second pass";
const OPUS_REPLY: &str = "OPUS_REPLY_MARK first answer";
const FABLE_THOUGHT: &str = "FABLE_THOUGHT_MARK the cache key is wrong";
const FABLE_REPLY: &str = "FABLE_REPLY_MARK second answer";
const OPUS_AGAIN_REPLY: &str = "OPUS_AGAIN_MARK third answer";

/// Claude models behind one Messages provider that points at the mock.
fn use_two_claude_models(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.claude]\nbase_url = \"{}\"\napi_backend = \"messages\"\napi_key = \"sk-mock\"\nmodels_autodetect = false\n\n[model.opus]\nmodel = \"{OPUS}\"\nmodel_provider = \"claude\"\n\n[model.fable]\nmodel = \"{FABLE}\"\nmodel_provider = \"claude\"\n\n[models]\ndefault = \"opus\"\n",
            content.url()
        ),
    )
    .expect("write config.toml");
}

/// The text of every block of `kind` in the messages of `body` that carry `role`.
fn block_text(body: &serde_json::Value, role: &str, kind: &str, key: &str) -> String {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == role)
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|b| b["type"] == kind)
        .filter_map(|b| b[key].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The main-loop request to `model` whose history already holds `prior_reply`.
fn request_after<'a>(
    bodies: &'a [serde_json::Value],
    model: &str,
    prior_reply: &str,
) -> &'a serde_json::Value {
    bodies
        .iter()
        .find(|b| {
            b["model"] == model && block_text(b, "assistant", "text", "text").contains(prior_reply)
        })
        .unwrap_or_else(|| {
            panic!(
                "no request to {model} carries `{prior_reply}`\n{}",
                dump_non_system_messages(bodies)
            )
        })
}

/// `thought` reaches the model in a user-role client names `origin`. No
/// assistant text shows it.
fn assert_thought_rides_as_a_note(body: &serde_json::Value, thought: &str, origin: &str) {
    let user = block_text(body, "user", "text", "text");
    assert!(
        user.contains("Client note") && user.contains(thought) && user.contains(origin),
        "`{thought}` must reach the model in a user-role note that names {origin}: {body:#}"
    );
    assert!(
        !block_text(body, "assistant", "text", "text").contains(thought),
        "assistant text must not show `{thought}`: {body:#}"
    );
    assert!(
        !body.to_string().contains("<thinking>"),
        "no text may carry a thinking tag: {body:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn claude_model_switch_keeps_the_thinking_summary_off_assistant_text() {
    let content = ContentController::start().await.expect("start content");
    use_two_claude_models(&content);
    content.server().set_keep_requests(true);
    content.server().set_conversations(vec![
        Conversation::nth(1)
            .reasoning(OPUS_THOUGHT)
            .reply(OPUS_REPLY)
            .reasoning(FABLE_THOUGHT)
            .reply(FABLE_REPLY)
            .reply(OPUS_AGAIN_REPLY),
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

    let mut turn = |harness: &mut PtyHarness, switch_to: Option<&str>, reply: &str| {
        if let Some(model) = switch_to {
            harness
                .inject_keys(format!("/model {model}\r").as_bytes())
                .expect("type model switch");
            harness
                .wait_for_text(model, Duration::from_secs(15))
                .unwrap_or_else(|_| {
                    panic!(
                        "{model} must show after the switch\nscreen:\n{}",
                        harness.screen_contents()
                    )
                });
        }
        harness
            .inject_keys(format!("{PROMPT}\r").as_bytes())
            .expect("submit prompt");
        harness
            .wait_for_text(reply, Duration::from_secs(60))
            .unwrap_or_else(|_| {
                panic!(
                    "`{reply}` must render\nscreen:\n{}\n{}",
                    harness.screen_contents(),
                    dump_non_system_messages(&content.request_bodies())
                )
            });
    };
    turn(&mut harness, None, OPUS_REPLY);
    turn(&mut harness, Some("fable"), FABLE_REPLY);
    turn(&mut harness, Some("opus"), OPUS_AGAIN_REPLY);

    let bodies = content.request_bodies();

    // Opus thought first. Fable did not think it, so Fable gets the summary as a note.
    let to_fable = request_after(&bodies, FABLE, OPUS_REPLY);
    assert_thought_rides_as_a_note(to_fable, OPUS_THOUGHT, OPUS);
    assert!(
        !block_text(to_fable, "assistant", "thinking", "thinking").contains(OPUS_THOUGHT),
        "Fable must not get the Opus thinking block: {to_fable:#}"
    );

    // Back on Opus: its own thought is a thinking block again, and the Fable thought is a note.
    let to_opus = request_after(&bodies, OPUS, FABLE_REPLY);
    assert_thought_rides_as_a_note(to_opus, FABLE_THOUGHT, FABLE);
    assert!(
        block_text(to_opus, "assistant", "thinking", "thinking").contains(OPUS_THOUGHT),
        "Opus must get its own thinking block back: {to_opus:#}"
    );

    assert!(
        content.server().script_violations().is_empty(),
        "the script must be served in order: {:?}",
        content.server().script_violations()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
