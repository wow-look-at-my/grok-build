// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// `$HOME` (and its default `~/.grok`) can never be recorded by the trust store, so `decide`
/// resolves Trusted rather than prompting on a key that could never persist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn folder_trust_cwd_is_home_git_repo_no_prompt() {
    let content = ContentController::start().await.expect("start content");

    // $HOME is a git repo with a home-level repo-local marker, and cwd IS $HOME There is genuinely "something to gate".
    git2::Repository::init(content.home()).expect("git init $HOME");
    std::fs::write(content.home().join(".mcp.json"), "{}").expect("write $HOME/.mcp.json");

    let env_refs = trust_env(true);
    let cwd = content.home().to_str().expect("utf8 home path");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--cwd", cwd],
        &env_refs,
    )
    .expect("spawn pager");

    // Normal welcome boots; the trust question never appears (an unrecordable
    // key resolves Trusted).
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("normal welcome renders");
    assert!(
        !harness.contains_text(TRUST_QUESTION_SENTINEL),
        "cwd == $HOME must not render the trust question (unrecordable key => Trusted)\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}
