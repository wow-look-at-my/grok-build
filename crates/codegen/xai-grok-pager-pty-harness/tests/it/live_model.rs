//! Opt-in runs of the real pager against a live model. No CI family selects them: each run needs a provider key and costs money.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use xai_grok_pager_pty_harness::{PtyHarness, pager_binary};
use xai_grok_test_support::TestSandbox;

use crate::common::wait_until;

const WELCOME: &str = "Quit";
const WAITING_ROW: &str = "Goal is waiting on";
const CHECK_ROW: &str = "Goal check:";
/// How long the scripted build runs. The model must end its turn inside this window.
const BUILD_SECS: u64 = 45;
const BUILD_RESULT: &str = "BUILD OK 7421";

/// Variables of the parent that the child needs to reach the provider through this machine's proxy.
const NETWORK_ENV: [&str; 8] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
];

fn required_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be set for a live-model run (see the file header)"))
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The entries of the child's unified log whose event name is `event`.
fn log_events(grok_home: &Path, event: &str) -> Vec<Value> {
    read_jsonl(&grok_home.join("logs").join("unified.jsonl"))
        .into_iter()
        .filter(|entry| entry.to_string().contains(&format!("\"{event}\"")))
        .collect()
}

/// The chat history of the session under `grok_home`.
fn chat_history(grok_home: &Path) -> Vec<Value> {
    let sessions = grok_home.join("sessions");
    let mut files: Vec<PathBuf> = Vec::new();
    for cwd_dir in std::fs::read_dir(&sessions).into_iter().flatten().flatten() {
        for session in std::fs::read_dir(cwd_dir.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let file = session.path().join("chat_history.jsonl");
            if file.is_file() {
                files.push(file);
            }
        }
    }
    // A session id sorts by its start time. A full goal's subagents start after the main session.
    files.sort();
    let main = files
        .first()
        .unwrap_or_else(|| panic!("no session under {}", sessions.display()));
    read_jsonl(main)
}

/// What the model did so far, for a failure message.
fn run_summary(grok_home: &Path) -> String {
    let history = chat_history(grok_home);
    let calls: Vec<String> = assistant_items(&history)
        .iter()
        .flat_map(|item| tool_call_names(item))
        .collect();
    let directives = history
        .iter()
        .filter(|item| item["type"] == "user" && item.to_string().contains("Goal NOT complete"))
        .count();
    format!(
        "tool calls: {calls:?}\ngoal continuation directives sent to the model: {directives}\ngoal check holds: {}",
        log_events(grok_home, "shell.goal.check_held").len()
    )
}

fn goal_command() -> &'static str {
    match std::env::var("GROK_LIVE_GOAL_MODE").as_deref() {
        Err(_) | Ok("lite") => "/goal --lite",
        Ok("full") => "/goal",
        Ok(other) => panic!("GROK_LIVE_GOAL_MODE must be `lite` or `full`, not {other:?}"),
    }
}

fn tool_call_names(item: &Value) -> Vec<String> {
    item["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|call| {
            call["name"]
                .as_str()
                .or_else(|| call["function"]["name"].as_str())
        })
        .map(str::to_owned)
        .collect()
}

fn assistant_items(history: &[Value]) -> Vec<&Value> {
    history
        .iter()
        .filter(|item| item["type"] == "assistant")
        .collect()
}

/// A live model starts a slow command in the background under a goal. The turn must end with a message while the
/// command still runs, with no goal check. The completion must wake the session, and the goal check runs after that.
#[cfg(unix)]
#[test]
#[ignore = "live model; needs GROK_LIVE_BASE_URL, GROK_LIVE_MODEL and GROK_LIVE_KEY_ENV"]
fn live_goal_turn_idles_on_background_work_and_is_checked_after_the_wake() {
    let base_url = required_env("GROK_LIVE_BASE_URL");
    let model = required_env("GROK_LIVE_MODEL");
    let key_env = required_env("GROK_LIVE_KEY_ENV");
    let key = required_env(&key_env);

    let sandbox = TestSandbox::new();
    let grok_home = sandbox.grok_home().to_path_buf();
    let workspace = sandbox.workspace().to_path_buf();
    let config = json!({
        "models": {
            "default": "live",
            "providers": {
                "live": { "base_url": base_url, "env_key": key_env, "models_autodetect": false }
            },
            "definitions": {
                "live": { "model_provider": "live", "model": model, "context_window": 200_000 }
            }
        }
    });
    std::fs::write(grok_home.join("config.json"), config.to_string()).expect("write config");
    let build_log = workspace.join("build.log");
    let script = workspace.join("build.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\n/bin/sleep {BUILD_SECS}\necho '{BUILD_RESULT}' > build.log\n"),
    )
    .expect("write build script");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the build script executable");
    }

    let mut env: Vec<(String, String)> = vec![
        (key_env.clone(), key),
        ("GROK_GOAL".into(), "1".into()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
    ];
    for name in NETWORK_ENV {
        if let Ok(value) = std::env::var(name) {
            env.push((name.to_owned(), value));
        }
    }
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::new_in_sandbox(
        &binary,
        160,
        140,
        &["--yolo", "--trust"],
        &sandbox,
        &env_refs,
        Some(&workspace),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME, Duration::from_secs(30))
        .unwrap_or_else(|e| panic!("welcome: {e:#}\nscreen:\n{}", harness.screen_contents()));
    let goal = format!(
        "{} Run ./build.sh as a background command. It takes about {BUILD_SECS} seconds. \
         When it has finished, tell me the last line of build.log.",
        goal_command()
    );
    harness.inject_keys(goal.as_bytes()).expect("type goal");
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\r").expect("submit goal");

    // The goal reports its wait while the build still runs.
    let mut check_before_build_end = false;
    let waiting = wait_until(Duration::from_secs(BUILD_SECS + 600), || {
        harness.update(Duration::from_millis(200));
        let screen = harness.screen_contents();
        check_before_build_end |= screen.contains(CHECK_ROW) && !build_log.exists();
        screen.contains(WAITING_ROW) || build_log.exists()
    });
    let screen = harness.screen_contents();
    assert!(
        waiting && screen.contains(WAITING_ROW) && !build_log.exists(),
        "the live model must end its turn while the build runs, and the goal must report the wait\n{}\nscreen:\n{screen}",
        run_summary(&grok_home)
    );
    assert!(
        !check_before_build_end && !screen.contains(CHECK_ROW),
        "no goal check may run while the build runs\nscreen:\n{screen}"
    );
    assert!(
        !log_events(&grok_home, "shell.goal.check_held").is_empty(),
        "the turn must end on the goal check hold"
    );

    // The turn that ended sent a message, and it called no tool after it.
    let history = chat_history(&grok_home);
    let assistants = assistant_items(&history);
    let last = assistants
        .last()
        .unwrap_or_else(|| panic!("the model must have answered\nscreen:\n{screen}"));
    let wait_message = last["content"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    println!("LIVE wait message: {wait_message}");
    assert!(
        !wait_message.is_empty(),
        "the turn that waits must end with a message, not with thinking alone: {last}"
    );
    assert!(
        tool_call_names(last).is_empty(),
        "the message that ends the waiting turn carries no tool call: {last}"
    );
    let first_line = wait_message.lines().next().unwrap_or_default();
    let shown: String = first_line.chars().take(24).collect();
    assert!(
        screen.contains(shown.trim()),
        "the wait message must be on screen: {shown:?}\nscreen:\n{screen}"
    );
    let calls_before_wait: Vec<String> =
        assistants.iter().flat_map(|a| tool_call_names(a)).collect();
    println!("LIVE tool calls before the wait: {calls_before_wait:?}");

    // The build ends. The completion wakes the session, and the goal check runs after the wake turn.
    let mut check_before_build_end = false;
    let checked = wait_until(Duration::from_secs(BUILD_SECS + 300), || {
        harness.update(Duration::from_millis(200));
        let screen = harness.screen_contents();
        check_before_build_end |= screen.contains(CHECK_ROW) && !build_log.exists();
        screen.contains(CHECK_ROW)
    });
    let screen = harness.screen_contents();
    assert!(
        !check_before_build_end,
        "the goal check ran before the build ended\nscreen:\n{screen}"
    );
    assert!(
        checked && build_log.exists(),
        "the goal check must run after the build ends and the wake turn finishes\nscreen:\n{screen}"
    );
    let wake_turns: Vec<Value> = log_events(&grok_home, "shell.handle_prompt.start")
        .into_iter()
        .filter(|entry| entry.to_string().contains("task-completed-"))
        .collect();
    assert!(
        !wake_turns.is_empty(),
        "the completion message must start the wake turn\nscreen:\n{screen}"
    );
    let history = chat_history(&grok_home);
    let assistants = assistant_items(&history);
    let wake_message = assistants
        .last()
        .and_then(|item| item["content"].as_str())
        .unwrap_or_default()
        .trim()
        .to_owned();
    println!("LIVE wake message: {wake_message}");
    let all_calls: Vec<String> = assistants.iter().flat_map(|a| tool_call_names(a)).collect();
    println!("LIVE tool calls of the whole run: {all_calls:?}");
    assert!(
        wake_message.contains(BUILD_RESULT),
        "the wake turn must report the build result: {wake_message:?}\nscreen:\n{screen}"
    );

    harness.update(Duration::from_secs(2));
    println!("LIVE final screen:\n{}", harness.screen_contents());
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    let _ = harness.quit();
}
