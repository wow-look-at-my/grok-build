// memory_settings_pty.rs holds open_memory_sheet.

use super::common::*;

const TIMEOUT: Duration = Duration::from_secs(20);

fn saved_config(content: &ContentController) -> serde_json::Value {
    let path = content.sandbox().grok_home().join("config.json");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
}

/// Open `/settings` and the Memory sub-sheet.
fn open_memory_sheet(harness: &mut PtyHarness) {
    harness.inject_keys(keys::F2).expect("open settings");
    harness
        .wait_for_text("Appearance", TIMEOUT)
        .expect("settings shown");
    harness.inject_keys(b"/").expect("search settings");
    harness
        .wait_for_text_absent("/ to search", TIMEOUT)
        .expect("search focused");
    harness.inject_keys(b"memory recall").expect("type search");
    harness
        .wait_for_text("Memory recall and the embedding endpoint", TIMEOUT)
        .unwrap_or_else(|_| panic!("search finds the group\n{}", harness.screen_contents()));
    harness
        .inject_keys(keys::ENTER)
        .expect("commit settings search");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(keys::ENTER).expect("open the group");
    harness
        .wait_for_text("Embedding endpoint", TIMEOUT)
        .unwrap_or_else(|_| panic!("sub-sheet lists the rows\n{}", harness.screen_contents()));
}

/// Move to the next row of the sub-sheet, open its editor, type `text` and save.
fn edit_next_row(harness: &mut PtyHarness, text: &str) {
    harness.inject_keys(b"j").expect("next row");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(keys::ENTER).expect("open the editor");
    harness.update(Duration::from_millis(300));
    harness
        .inject_keys(text.as_bytes())
        .expect("type the value");
    harness
        .wait_for_text(text, TIMEOUT)
        .unwrap_or_else(|_| panic!("editor shows `{text}`\n{}", harness.screen_contents()));
    harness.inject_keys(keys::ENTER).expect("save the value");
}

fn wait_saved(
    harness: &mut PtyHarness,
    content: &ContentController,
    what: &str,
    saved: impl Fn(&serde_json::Value) -> bool,
) {
    harness
        .wait_until(what, TIMEOUT, |_| saved(&saved_config(content)))
        .unwrap_or_else(|_| {
            panic!(
                "{what}\nconfig.json:\n{:#}\nscreen:\n{}",
                saved_config(content),
                harness.screen_contents()
            )
        });
}

/// The Memory rows of `/settings` write `config.json`, and a restarted pager shows what they wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; requires a rebuilt pager binary"]
async fn memory_settings_write_config_and_show_after_a_restart() {
    let content = ContentController::start().await.expect("mock server");
    content.set_response(format!("{MOCK_RESPONSE_SENTINEL} memory settings turn."));

    let binary = pager_binary().expect("pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, Duration::from_secs(30))
        .expect("turn rendered");
    harness.wait_for_turn_idle(TIMEOUT).expect("idle session");

    open_memory_sheet(&mut harness);

    harness.inject_keys(b" ").expect("turn recall off");
    wait_saved(&mut harness, &content, "recall is saved as off", |config| {
        config["memory"]["v2"]["recall_enabled"] == false
    });

    edit_next_row(&mut harness, "nomic-embed-text");
    wait_saved(&mut harness, &content, "the model is saved", |config| {
        config["memory"]["embedding"]["model"] == "nomic-embed-text"
    });

    edit_next_row(&mut harness, "http://localhost:11434/v1");
    wait_saved(&mut harness, &content, "the endpoint is saved", |config| {
        config["memory"]["embedding"]["base_url"] == "http://localhost:11434/v1"
    });

    edit_next_row(&mut harness, "EMBED_KEY");
    wait_saved(
        &mut harness,
        &content,
        "the key variable is saved",
        |config| config["memory"]["embedding"]["api_key_env"] == "EMBED_KEY",
    );

    harness.inject_keys(keys::ESC).expect("close the sub-sheet");
    harness.update(Duration::from_millis(200));
    harness.inject_keys(keys::F2).expect("close settings");
    harness.quit().expect("stop first pager");

    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &["-c"])
            .expect("restart pager");
    harness
        .wait_for_text(MOCK_RESPONSE_SENTINEL, RESUME_TIMEOUT)
        .expect("resumed session");
    open_memory_sheet(&mut harness);
    for shown in ["nomic-embed-text", "http://localhost:11434/v1", "EMBED_KEY"] {
        harness
            .wait_for_text(shown, TIMEOUT)
            .unwrap_or_else(|_| panic!("the sheet shows `{shown}`\n{}", harness.screen_contents()));
    }
    harness
        .wait_until("recall reads off", TIMEOUT, |harness| {
            harness.screen_contents().lines().any(|line| {
                line.contains("Memory recall") && line.split_whitespace().any(|word| word == "off")
            })
        })
        .unwrap_or_else(|_| panic!("recall reads off\n{}", harness.screen_contents()));

    harness.quit().expect("clean quit");
}
