// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// The input mode follows the first character. `!` typed at the start of a draft that already
/// holds text turns the draft into a bash command. It runs, and it never reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
#[cfg(unix)]
async fn bang_at_start_of_draft_runs_bash() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} short reply."));

    let project = tempfile::tempdir().expect("create project dir");
    std::fs::create_dir_all(project.path().join(".git")).expect("create .git");
    let cwd = dunce::canonicalize(project.path()).expect("canonicalize project");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        Some(cwd.as_path()),
    )
    .expect("spawn pager");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("first turn done");
    harness.update(Duration::from_secs(2));

    harness
        .inject_keys(b"printf 'FIRSTCHAR_%s_OK\\n' RAN")
        .expect("type the draft");
    harness
        .wait_for_text("FIRSTCHAR", Duration::from_secs(10))
        .expect("draft echoes");
    // Home moves the cursor to the start of the draft.
    harness.inject_keys(b"\x1b[H").expect("press Home");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"!").expect("type !");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\r").expect("submit");

    harness
        .wait_for_text("FIRSTCHAR_RAN_OK", Duration::from_secs(30))
        .unwrap_or_else(|_| {
            panic!(
                "the draft did not run as a bash command\nscreen:\n{}",
                harness.screen_contents()
            )
        });
    harness
        .wait_for_text("Run (user)", Duration::from_secs(15))
        .expect("Run (user) chrome for the bash turn");

    let users = all_user_message_blobs(&content);
    assert!(
        !users.iter().any(|u| u.contains("FIRSTCHAR")),
        "the draft reached the model as a prompt instead of running as bash: {users:#?}"
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.quit().expect("clean quit");
}
