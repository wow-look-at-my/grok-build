// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **`/model` under `--sandbox=workspace` switches live and on the wire, session-only.**
/// `workspace` write-denies `$GROK_HOME/config.toml` (trust boundary), so the persist
/// leg of the switch must report success without touching disk and must not roll the
/// session back. See docs/user-guide/18-sandbox.md, "Direct global write protection".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
#[cfg(unix)]
async fn sandbox_workspace_model_switch_applies_session_only_and_switches_on_wire() {
    let support = xai_grok_sandbox::SandboxManager::support_info();
    if !support.is_supported {
        eprintln!("skip: no kernel sandbox on this host: {}", support.details);
        return;
    }
    // The workspace profile's deny list is mounted through bubblewrap on Linux;
    // with no bwrap the pager refuses to start under the profile.
    if cfg!(target_os = "linux")
        && !std::process::Command::new("bwrap")
            .arg("--version")
            .status()
            .is_ok_and(|s| s.success())
    {
        eprintln!("skip: no usable bwrap on this host");
        return;
    }
    let content = ContentController::start_with_models(vec![
        MockModel::new("model-a"),
        MockModel::new("model-b"),
    ])
    .await
    .expect("start content");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} first turn."));

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &["--sandbox=workspace"],
    )
    .expect("spawn pager under workspace sandbox");

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

    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} second turn."));
    harness
        .inject_keys(b"/model model-b\r")
        .expect("type model switch");
    harness
        .wait_for_text("model-b", Duration::from_secs(15))
        .expect("model-b visible on screen after switch");
    assert!(
        !harness.contains_text("Could not save"),
        "workspace confinement must not fail the default_model persist\nscreen:\n{}",
        harness.screen_contents()
    );

    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit second prompt");
    harness
        .wait_for_text("second turn", Duration::from_secs(30))
        .expect("second turn rendered");

    let bodies = content.request_bodies();
    assert!(
        bodies
            .iter()
            .any(|b| b.get("model").and_then(|m| m.as_str()) == Some("model-b")),
        "the request after the switch must carry the switched model\nbodies: {bodies:#?}"
    );

    // The profile write-denies config.toml: the switch must NOT reach disk
    // (session-only by design, docs/user-guide/18-sandbox.md).
    let config = std::fs::read_to_string(content.sandbox().grok_home().join("config.toml"))
        .unwrap_or_default();
    assert!(
        !config.contains("model-b"),
        "[models].default must not be written to the write-denied config.toml\nconfig.toml:\n{config}"
    );

    harness.quit().expect("clean quit");
}
