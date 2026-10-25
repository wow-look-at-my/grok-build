// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **Settings changed under `--sandbox=workspace` save to `config.toml`.**
/// `workspace` write-denies `$GROK_HOME/config.toml` for this process, so grok
/// writes it through the unsandboxed host worker that started before
/// confinement. `/model` must switch live, on the wire and on disk, and a
/// permission-mode change must save too. See docs/user-guide/18-sandbox.md,
/// "Direct global write protection".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
#[cfg(unix)]
async fn sandbox_workspace_model_switch_switches_on_wire_and_persists() {
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

    // The confined pager records kernel enforcement in its own event log
    // during startup.
    let events_path = content
        .sandbox()
        .grok_home()
        .join("sessions")
        .join("sandbox-events.jsonl");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !workspace_profile_applied(&events_path) {
        assert!(
            Instant::now() < deadline,
            "the pager never recorded kernel enforcement of the workspace profile; \
             events log:\n{}",
            std::fs::read_to_string(&events_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

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

    let config_path = content.sandbox().grok_home().join("config.toml");
    wait_for_config(&config_path, "model-b");

    // A setting that grants capability saves the same way: grok writes its own config.
    harness
        .inject_keys(b"/always-approve\r")
        .expect("type always-approve");
    wait_for_config(&config_path, "permission_mode = \"always-approve\"");
    assert!(
        !harness.contains_text("Could not save"),
        "workspace confinement must not fail the permission-mode persist\nscreen:\n{}",
        harness.screen_contents()
    );

    harness.quit().expect("clean quit");
}

/// Wait until `config.toml` holds `needle`, which only the host worker can write there.
fn wait_for_config(config_path: &std::path::Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let config = std::fs::read_to_string(config_path).unwrap_or_default();
        if config.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "`{needle}` must reach config.toml through the host worker\nconfig.toml:\n{config}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether the sandbox event log holds a kernel-enforced application of the
/// workspace profile: the `ProfileApplied` event the pager writes only after
/// `Sandbox::apply` succeeds.
fn workspace_profile_applied(events_path: &std::path::Path) -> bool {
    let Ok(text) = std::fs::read_to_string(events_path) else {
        return false;
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|event| {
            event.get("event_type").and_then(serde_json::Value::as_str) == Some("ProfileApplied")
                && event.get("profile").and_then(serde_json::Value::as_str) == Some("workspace")
                && event.get("enforced").and_then(serde_json::Value::as_bool) == Some(true)
        })
}
