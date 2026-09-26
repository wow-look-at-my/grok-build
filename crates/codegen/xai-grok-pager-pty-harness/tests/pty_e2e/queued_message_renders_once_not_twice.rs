//! PTY, flag-file driven like `endline_park_is_markerless`: the "queued
//! message appears 2x" regression. A message queued mid-turn is delivered
//! into the running turn at its next model request — asserting it renders
//! exactly once as a "❯ " block with no queue row left behind, and reaches
//! the model exactly once, carrying the interjection preamble.
#[allow(unused_imports)]
use super::common::*;

/// User messages in the most recent request body that contain `needle`.
/// Counted per-request, not across all of them: every later request replays
/// the same history, so a cross-request tally cannot tell a duplicate from a
/// resend.
#[cfg(unix)]
fn user_hits_in_last_request(content: &ContentController, needle: &str) -> usize {
    content
        .request_bodies()
        .last()
        .map(|body| {
            body["messages"]
                .as_array()
                .or_else(|| body["input"].as_array())
                .into_iter()
                .flatten()
                .filter(|m| m["role"] == "user" && m["content"].to_string().contains(needle))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run the owning pty_e2e_* Cargo test with --ignored (see Cargo.toml)"]
async fn queued_message_renders_once_not_twice() {
    const QUEUED_TEXT: &str = "queued exactly once probe";

    let content = ContentController::start().await.expect("start content");
    let park_flag = content.home().join("qonce_park_flag");
    let id_ready_flag = content.home().join("qonce_id_ready_flag");

    let gated_loop = |flag: &std::path::Path| {
        format!("while [ ! -e {} ]; do /bin/sleep 0.2; done", flag.display())
    };

    // Tool call 1: the flag-gated background command the wait blocks on.
    let bg_args = json!({
        "command": gated_loop(&park_flag),
        "description": "flag-gated command",
        "is_background": true
    })
    .to_string();
    let _background_turn =
        expect_tool_turn(&content, "call_qonce_bg", "run_terminal_command", bg_args);

    // Tool call 2: the flag-gated foreground hold — the mid-turn window
    // where the follow-up is queued.
    let id_hold_args = json!({
        "command": gated_loop(&id_ready_flag),
        "description": "hold for id extraction"
    })
    .to_string();
    let _id_hold_turn = expect_tool_turn(
        &content,
        "call_qonce_id_hold",
        "run_terminal_command",
        id_hold_args,
    );

    // Fallback for the turn's wrap-up once the wait returns.
    content.set_response("QONCE_WRAPUP done.");

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

    let task_id = poll_for(Duration::from_secs(30), || {
        content
            .request_bodies()
            .iter()
            .find_map(|b| extract_task_id(&b.to_string()))
    })
    .unwrap_or_else(|| {
        panic!(
            "no <task-id> in any request body\n--- non-system messages ---\n{}\n--- screen ---\n{}",
            dump_non_system_messages(&content.request_bodies()),
            harness.screen_contents()
        )
    });

    // Queue the follow-up while the id-hold tool is still running: the turn
    // is inside a tool, so nothing has been sent to the model since it was
    // typed.
    harness
        .inject_keys(format!("{QUEUED_TEXT}\r").as_bytes())
        .expect("queue follow-up mid-turn");
    harness
        .wait_for_text(QUEUED_TEXT, Duration::from_secs(10))
        .expect("queued row visible");
    assert_eq!(
        harness
            .screen_contents()
            .lines()
            .filter(|l| l.contains(QUEUED_TEXT))
            .count(),
        1,
        "queued message must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !all_user_message_blobs(&content)
            .iter()
            .any(|u| u.contains(QUEUED_TEXT)),
        "no request has been made since the follow-up was typed"
    );

    // Tool call 3: block on the REAL task. The follow-up must already have
    // been delivered on the request that produced this call — it is not held
    // for the wait, and nothing was cancelled to deliver it.
    let wait_args = json!({
        "task_ids": [task_id],
        "timeout_ms": 600_000
    })
    .to_string();
    let _wait_turn = expect_tool_turn(
        &content,
        "call_qonce_wait",
        "get_command_or_subagent_output",
        wait_args,
    );
    std::fs::write(&id_ready_flag, b"ready").expect("release id-extraction hold");

    harness
        .wait_for_text(&format!("\u{276F} {QUEUED_TEXT}"), Duration::from_secs(60))
        .unwrap_or_else(|_| {
            panic!(
                "follow-up never delivered into the running turn; screen:\n{}\n--- non-system messages ---\n{}",
                harness.screen_contents(),
                dump_non_system_messages(&content.request_bodies())
            )
        });
    assert!(
        !harness.contains_text("Worked for"),
        "delivering a follow-up ends no turn\nscreen:\n{}",
        harness.screen_contents()
    );
    let delivered = poll_for(Duration::from_secs(30), || {
        all_user_message_blobs(&content)
            .into_iter()
            .find(|u| u.contains(QUEUED_TEXT))
    })
    .expect("delivered follow-up must reach the model");
    assert!(
        delivered.contains(INTERJECTION_WIRE_PREFIX),
        "a follow-up delivered mid-turn carries the interjection preamble: {delivered}"
    );

    // Let the wait return so the turn wraps up.
    std::fs::write(&park_flag, b"done").expect("release flag");
    harness
        .wait_for_text("QONCE_WRAPUP", Duration::from_secs(60))
        .expect("turn wraps up after the wait returns");

    // Exactly once end-to-end: one "❯ " block, no leftover queue row, and one
    // user message — never a second turn replaying the same text.
    let settled = wait_until(Duration::from_secs(10), || {
        harness.update(Duration::from_millis(100));
        harness
            .screen_contents()
            .lines()
            .filter(|l| l.contains(QUEUED_TEXT))
            .count()
            == 1
    });
    assert!(
        settled,
        "delivered message must render exactly once\nscreen:\n{}",
        harness.screen_contents()
    );
    assert_eq!(
        user_hits_in_last_request(&content, QUEUED_TEXT),
        1,
        "delivered message must reach the model exactly once\n--- non-system messages ---\n{}",
        dump_non_system_messages(&content.request_bodies())
    );

    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
