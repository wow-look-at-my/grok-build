// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **Settings changed inside the `--sandbox=pathbox` jail save to the real
/// `config.toml`.** The jail binds `$GROK_HOME`. A `/model` switch and a
/// permission-mode change must reach the file on the host, with no
/// "Could not save" toast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
#[cfg(target_os = "linux")]
async fn sandbox_pathbox_settings_save_to_the_real_config() {
    let content = start_content().await;
    let config_path = content.sandbox().grok_home().join("config.toml");
    switch_and_persist(&content, &[], &config_path).await;
}

/// **The same with a symlinked `$GROK_HOME` and a `config.toml` linked into a
/// dotfiles folder.** The jailed process uses the symlink path, and the save
/// renames a temp file beside the link's target, so the jail must bind both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
#[cfg(target_os = "linux")]
async fn sandbox_pathbox_settings_save_through_a_symlinked_grok_home() {
    let content = start_content().await;
    let sandbox = content.sandbox();
    let real_home = sandbox.grok_home();
    let dotfiles = sandbox.root().join("dotfiles");
    std::fs::create_dir_all(&dotfiles).expect("create dotfiles");
    let linked_config = dotfiles.join("config.toml");
    let existing = std::fs::read_to_string(real_home.join("config.toml")).unwrap_or_default();
    let _ = std::fs::remove_file(real_home.join("config.toml"));
    std::fs::write(&linked_config, existing).expect("write the dotfiles config.toml");
    std::os::unix::fs::symlink(&linked_config, real_home.join("config.toml"))
        .expect("link config.toml");
    let home_link = sandbox.root().join("grok-home-link");
    std::os::unix::fs::symlink(real_home, &home_link).expect("link grok home");
    let home_link = home_link.to_str().expect("utf-8 path").to_string();
    switch_and_persist(&content, &[("GROK_HOME", &home_link)], &linked_config).await;
}

async fn start_content() -> ContentController {
    let content = ContentController::start_with_models(vec![
        MockModel::new("model-a"),
        MockModel::new("model-b"),
    ])
    .await
    .expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} first turn."));
    content
}

async fn switch_and_persist(
    content: &ContentController,
    env: &[(&str, &str)],
    config_path: &std::path::Path,
) {
    if !bwrap_can_jail() {
        eprintln!("SKIP: bubblewrap cannot build a jail on this host");
        return;
    }
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        content,
        &["--sandbox=pathbox"],
        env,
        Some(content.sandbox().workspace()),
    )
    .expect("spawn pager in the pathbox jail");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // A session must exist so `/model` sends the session-scoped SetSessionModel.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("first turn rendered");

    harness
        .inject_keys(b"/model model-b\r")
        .expect("type model switch");
    harness
        .wait_for_text("model-b", Duration::from_secs(15))
        .expect("model-b visible on screen after switch");
    wait_for_config(config_path, "model-b");

    harness
        .inject_keys(b"/always-approve\r")
        .expect("type always-approve");
    wait_for_config(config_path, "permission_mode = \"always-approve\"");
    assert!(
        !harness.contains_text("Could not save"),
        "the jail must not fail a settings save\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}

fn bwrap_can_jail() -> bool {
    std::process::Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "true"])
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_config(config_path: &std::path::Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let config = std::fs::read_to_string(config_path).unwrap_or_default();
        if config.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "`{needle}` must reach the real config.toml\nconfig.toml:\n{config}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
