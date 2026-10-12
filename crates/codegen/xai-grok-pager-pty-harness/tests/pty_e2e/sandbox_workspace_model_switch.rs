// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// **`/model` under `--sandbox=workspace` switches live, on the wire, and on disk.**
/// `workspace` write-denies `$GROK_HOME/config.toml` (trust boundary) for this
/// process, so the persist leg rides the unsandboxed host worker that started
/// before confinement. The switch must apply live and must land in `config.toml`;
/// the in-process deny still stands for every tool path. See
/// docs/user-guide/18-sandbox.md, "Direct global write protection".
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

    // The in-process write-deny still holds; the host worker started before confinement is what lands the write.
    let config_path = content.sandbox().grok_home().join("config.toml");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let config = std::fs::read_to_string(&config_path).unwrap_or_default();
        if config.contains("model-b") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "[models].default must reach config.toml through the host worker\nconfig.toml:\n{config}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    harness.quit().expect("clean quit");
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
