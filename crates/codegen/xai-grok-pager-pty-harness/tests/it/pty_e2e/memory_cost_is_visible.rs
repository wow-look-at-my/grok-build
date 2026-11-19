//! PTY: what memory spends shows in the status bar and in `/memory`, with no command run.
#![cfg(unix)]
#[allow(unused_imports)]
use super::common::*;

/// The price the mock reports for each Messages call: `$0.5`.
const CALL_COST_USD_TICKS: i64 = 5_000_000_000;
const REPLY_MARK: &str = "MEMCOSTMARK";

/// One Messages model that points at the mock. Only the Messages route of the mock reports a price.
fn use_a_priced_model(content: &ContentController) {
    let grok_home = content.home().join(".grok");
    std::fs::create_dir_all(&grok_home).expect("create GROK_HOME");
    std::fs::write(
        grok_home.join("config.toml"),
        format!(
            "[model_providers.claude]\nbase_url = \"{}\"\napi_backend = \"messages\"\napi_key = \"sk-mock\"\nmodels_autodetect = false\n\n[model.opus]\nmodel = \"claude-opus-5-5\"\nmodel_provider = \"claude\"\n\n[models]\ndefault = \"opus\"\n",
            content.url()
        ),
    )
    .expect("write config.toml");
    content
        .server()
        .set_messages_cost_usd_ticks(Some(CALL_COST_USD_TICKS));
}

/// A workspace outside the system temp directory. Memory is off for a temp workspace.
struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("memory-cost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create workspace");
        Self(dir)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn memory_cost_shows_in_the_status_bar_and_in_the_memory_browser() {
    let content = ContentController::start().await.expect("start content");
    use_a_priced_model(&content);
    // Every request gets this reply, so the memory extractor reads it as its own output.
    content.set_response(
        serde_json::json!({
            "outcome": "observations",
            "observations": [{
                "type": "user",
                "scope": "global",
                "topic_hint": "code style",
                "statement": format!("{REPLY_MARK} the user wants every indent to be a tab."),
                "keywords": ["indent", "tabs"],
                "aliases": ["tab"],
                "body": null
            }]
        })
        .to_string(),
    );
    let workspace = Workspace::new();

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_in_dir(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        Some(&workspace.0),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(REPLY_MARK, Duration::from_secs(30))
        .expect("the reply is on screen");

    // The turn makes one capture call, and nothing else spends on memory.
    harness
        .wait_for_text("mem $0.5", Duration::from_secs(60))
        .unwrap_or_else(|error| {
            panic!(
                "the status bar must show what memory spent: {error}\nscreen:\n{}",
                harness.screen_contents()
            )
        });

    harness.inject_keys(b"/memory\r").expect("submit /memory");
    harness
        .wait_for_text("Memory cost: $0.5 (capture $0.5)", Duration::from_secs(30))
        .unwrap_or_else(|error| {
            panic!(
                "the memory browser must show the cost by kind: {error}\nscreen:\n{}",
                harness.screen_contents()
            )
        });
    // No embedding model is set, so the browser says what recall matches with and where to change it.
    assert!(
        harness.contains_text("Recall: words only. /settings"),
        "the memory browser must say what recall matches with\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager panicked\nscreen:\n{}",
        harness.screen_contents()
    );
}
