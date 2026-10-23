//! PTY: a reminder and an output-limit continue prompt each get a row, above the reply they caused.
#[allow(unused_imports)]
use super::common::*;
use xai_grok_pager_pty_harness::{InferenceEndpoint, InferenceRequestMatcher};

const CUT: &str = "CUT_REPLY_MARK";
const REST: &str = "REST_REPLY_MARK";
const PLAN_REPLY: &str = "PLAN_REPLY_MARK";

fn row_of(screen: &str, needle: &str) -> Option<usize> {
    screen.lines().position(|line| line.contains(needle))
}

fn quit(harness: &mut PtyHarness) {
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    harness.quit().expect("reap pager");
}

/// A reply the output limit cuts off, on both wires.
fn expect_cut_reply(content: &ContentController, text: &str) {
    for (endpoint, response) in [
        (
            InferenceEndpoint::Responses,
            ScriptedResponse::sse(sse::responses_api_script_exact_length(text, "test-model")),
        ),
        (
            InferenceEndpoint::ChatCompletions,
            ScriptedResponse::sse(sse::chat_completion_script_exact_length(text, "test-model")),
        ),
    ] {
        let _ = content.expect_response(
            format!("cut reply ({endpoint:?})"),
            InferenceRequestMatcher::foreground(endpoint),
            response,
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn length_continue_prompt_is_shown() {
    let content = ContentController::start().await.expect("start content");
    expect_cut_reply(&content, CUT);
    let _rest = content.expect_agent_turn("rest of the reply", REST);
    content.set_response("LENGTH_FALLBACK");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--yolo", "--trust"],
        &[("GROK_LENGTH_SALVAGE", "1")],
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
        .wait_for_text("Worked for", Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("turn never ended; screen:\n{}", harness.screen_contents()));

    let screen = harness.screen_contents();
    let cut = row_of(&screen, CUT);
    let row = row_of(&screen, "Sent to the model · output-limit continue");
    let rest = row_of(&screen, REST);
    assert!(
        cut.is_some() && row.is_some() && rest.is_some(),
        "the cut reply, the continue prompt and the rest must all show; screen:\n{screen}"
    );
    assert!(
        cut < row && row < rest,
        "order must be: cut reply, continue prompt, rest; screen:\n{screen}"
    );
    quit(&mut harness);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn plan_mode_reminder_is_shown() {
    let content = ContentController::start().await.expect("start content");
    let _reply = content.expect_agent_turn("plan reply", PLAN_REPLY);
    content.set_response("PLAN_FALLBACK");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--trust"],
        Some(content.home()),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome");
    let in_plan = wait_until(Duration::from_secs(10), || {
        harness.inject_keys(b"\x1b[Z").expect("shift-tab");
        harness.update(Duration::from_millis(300));
        harness.screen_contents().to_lowercase().contains("plan")
    });
    assert!(
        in_plan,
        "Shift+Tab must reach plan mode; screen:\n{}",
        harness.screen_contents()
    );
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(PLAN_REPLY, Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no reply; screen:\n{}", harness.screen_contents()));
    harness.update(Duration::from_millis(500));

    let screen = harness.screen_contents();
    let row = row_of(&screen, "Sent to the model · system reminder");
    let reply = row_of(&screen, PLAN_REPLY);
    assert!(
        row.is_some() && row < reply,
        "the plan-mode reminder must show above the reply it caused; screen:\n{screen}"
    );
    quit(&mut harness);
}
