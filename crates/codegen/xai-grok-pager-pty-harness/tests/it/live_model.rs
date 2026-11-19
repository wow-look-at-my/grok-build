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
        .unwrap_or_else(|| {
            panic!(
                "{name} must be set for a live-model run (see docs/background-waits-and-the-goal-check.md)"
            )
        })
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
        Err(_) | Ok("lite") => "/goal",
        Ok("full") => "/goal --full",
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

/// Text the pager shows when explore mode stops a goal or one of its subagents.
const EXPLORE_REFUSALS: [&str; 3] = ["Goal not started", "No plan was produced", "not allowed"];
/// A function only `src/marker.rs` of the sandbox workspace defines.
const MARKER_FN: &str = "live_marker_fn_7421";
const MARKER_FILE: &str = "marker.rs";
/// The last line of the goal-start reminder.
const GOAL_START: &str = "Start now.";

/// The main session's chat history, or nothing while no session has a history file.
fn chat_history_if_any(grok_home: &Path) -> Vec<Value> {
    let has_history = std::fs::read_dir(grok_home.join("sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|cwd_dir| {
            std::fs::read_dir(cwd_dir.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .any(|session| session.path().join("chat_history.jsonl").is_file());
    if has_history {
        chat_history(grok_home)
    } else {
        Vec::new()
    }
}

/// A live model runs a goal in a session that Shift+Tab put in explore mode.
/// The goal must start, and the model must find a file with explore's tools.
/// `GROK_LIVE_GOAL_MODE=full` adds the planner, which has no tool that writes a file there.
#[cfg(unix)]
#[test]
#[ignore = "live model; needs GROK_LIVE_BASE_URL, GROK_LIVE_MODEL and GROK_LIVE_KEY_ENV"]
fn live_goal_runs_in_explore_mode() {
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
    std::fs::create_dir_all(workspace.join("src")).expect("make src");
    std::fs::write(
        workspace.join("src").join(MARKER_FILE),
        format!("pub fn {MARKER_FN}() {{}}\n"),
    )
    .expect("write the marker file");

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

    // No `--yolo`: the ring then starts at its first stop.
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::new_in_sandbox(
        &binary,
        160,
        140,
        &["--trust"],
        &sandbox,
        &env_refs,
        Some(&workspace),
    )
    .expect("spawn pager");
    harness
        .wait_for_text(WELCOME, Duration::from_secs(30))
        .unwrap_or_else(|e| panic!("welcome: {e:#}\nscreen:\n{}", harness.screen_contents()));

    // One turn first: the ring is cycled inside a session.
    harness
        .inject_keys(b"Reply with one short sentence. Use no tool.\r")
        .expect("submit the first prompt");
    let answered = wait_until(Duration::from_secs(300), || {
        harness.update(Duration::from_millis(200));
        assistant_items(&chat_history_if_any(&grok_home))
            .iter()
            .any(|item| {
                !item["content"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            })
    });
    assert!(
        answered,
        "the live model must answer the first prompt\nscreen:\n{}",
        harness.screen_contents()
    );
    harness.update(Duration::from_secs(3));
    for mode in ["Plan", "Auto", "Always-Approve", "Orchestrator", "Explore"] {
        harness.inject_keys(b"\x1b[Z").expect("inject BackTab");
        harness
            .wait_for_text(
                &format!("Switched to mode: {mode}"),
                Duration::from_secs(30),
            )
            .unwrap_or_else(|e| {
                panic!(
                    "the ring must reach {mode}: {e:#}\nscreen:\n{}",
                    harness.screen_contents()
                )
            });
    }

    let goal = format!(
        "{} Find the file in this workspace that defines the function {MARKER_FN}. Tell me its path.",
        goal_command()
    );
    harness.inject_keys(goal.as_bytes()).expect("type goal");
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\r").expect("submit goal");

    // The goal starts, and the model names the file.
    let mut refused = None;
    let found = wait_until(Duration::from_secs(900), || {
        harness.update(Duration::from_millis(200));
        let screen = harness.screen_contents();
        refused = EXPLORE_REFUSALS
            .into_iter()
            .find(|refusal| screen.contains(refusal));
        refused.is_some()
            || assistant_items(&chat_history_if_any(&grok_home))
                .iter()
                .any(|item| {
                    item["content"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(MARKER_FILE)
                })
    });
    let screen = harness.screen_contents();
    assert!(
        refused.is_none(),
        "explore mode must not show `{}`\n{}\nscreen:\n{screen}",
        refused.unwrap_or_default(),
        run_summary(&grok_home)
    );
    assert!(
        found,
        "the live model must name the file under a goal in explore mode\n{}\nscreen:\n{screen}",
        run_summary(&grok_home)
    );
    let history = chat_history(&grok_home);
    assert!(
        history
            .iter()
            .any(|item| item["type"] == "user" && item.to_string().contains(GOAL_START)),
        "the goal must have started: no goal-start reminder is in the history\nscreen:\n{screen}"
    );

    // The main session ran as explore: it called no shell and no edit tool.
    let calls: Vec<String> = assistant_items(&history)
        .iter()
        .flat_map(|item| tool_call_names(item))
        .collect();
    println!("LIVE explore goal tool calls: {calls:?}");
    for absent in ["run_terminal_command", "search_replace", "write"] {
        assert!(
            !calls.iter().any(|call| call == absent),
            "an explore session must not call `{absent}`: {calls:?}"
        );
    }

    harness.update(Duration::from_secs(2));
    println!("LIVE final screen:\n{}", harness.screen_contents());
    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    let _ = harness.quit();
}

/// Opening tag of a memory recall reminder in the history.
const RECALL_OPEN_TAG: &str = "<memory-recall>";
/// The stored one prompt of the session asks about.
const RELEASE_NOTE: &str = "release.md";

/// The prompts of the memory session, in order. Only the third one is about a stored note.
/// The second states a preference, so capture has something to write from this conversation.
const MEMORY_PROMPTS: [&str; 5] = [
    "Create a file named notes.txt that holds the single line hello. Then tell me its size in bytes.",
    "From now on I want every text file in this project to end with a trailing newline, always. Add a second line that says goodbye to notes.txt.",
    "How do I run the zebrafish deploy pipeline? Answer from what you know and run nothing.",
    "Add a third line that says thanks to notes.txt, and keep my trailing newline rule.",
    "Read notes.txt back to me and tell me how many lines it has.",
];

/// The topic the session stores next to the release note. No prompt is about it.
const EDITOR_NOTE: &str = "files.md";

/// The observations and topics the session itself wrote under `grok_home`.
fn notes_written_by_the_session(grok_home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![grok_home.join("memory-v2")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let text = path.to_string_lossy().into_owned();
            if path.is_dir() {
                pending.push(path);
            } else if (text.contains("/observations/") || text.contains("/topics/"))
                && text.ends_with(".md")
                && !text.ends_with(RELEASE_NOTE)
                && !text.ends_with(EDITOR_NOTE)
            {
                found.push(path);
            }
        }
    }
    found
}

/// The note lines of every recall reminder, grouped by the prompt they followed.
fn recalled_notes_by_prompt(history: &[Value]) -> Vec<Vec<String>> {
    let mut by_prompt: Vec<Vec<String>> = vec![Vec::new(); MEMORY_PROMPTS.len()];
    let mut current = None;
    for item in history.iter().filter(|item| item["type"] == "user") {
        let text = item.to_string();
        if text.contains(RECALL_OPEN_TAG) {
            let slot = current.expect("a recall reminder follows a prompt");
            let notes = text
                .split("\\n")
                .filter(|line| line.trim_start().starts_with("- ") && line.contains("ref "))
                .map(|line| line.trim().to_owned());
            by_prompt[slot] = notes.collect();
        } else if let Some(index) = MEMORY_PROMPTS
            .iter()
            .position(|prompt| text.contains(prompt))
        {
            current = Some(index);
        }
    }
    by_prompt
}

/// A marker word that only the earlier session's facts hold.
const SCRATCH_PREFIX: &str = "zz_";
const DEPLOY_SCRIPT: &str = "tools/ship/quokka.sh";

/// The earlier session. Each prompt is a task that states a fact in passing. None asks the model to remember.
const EARLIER_PROMPTS: [&str; 2] = [
    "Create a file named zz_first.txt that holds the single line hello. I always prefix my scratch files with zz_ so that they sort last.",
    "For context, the deploy script of this project lives at tools/ship/quokka.sh. Do not create it or run it. Tell me how many characters that path has.",
];

/// The later session. The first prompt asks where a thing is. The second is a task that the preference applies to.
const LATER_PROMPTS: [&str; 2] = [
    "Where does the deploy script of this project live? Answer with the path. Run nothing.",
    "Create a scratch file that holds the single line bye. Choose the name yourself.",
];

/// The history of the session that holds `prompt`, or nothing before that session wrote one.
fn history_holding(grok_home: &Path, prompt: &str) -> Vec<Value> {
    let sessions = grok_home.join("sessions");
    for cwd_dir in std::fs::read_dir(&sessions).into_iter().flatten().flatten() {
        for session in std::fs::read_dir(cwd_dir.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let file = session.path().join("chat_history.jsonl");
            if !file.is_file() {
                continue;
            }
            let history = read_jsonl(&file);
            if history
                .iter()
                .any(|item| item["type"] == "user" && item.to_string().contains(prompt))
            {
                return history;
            }
        }
    }
    Vec::new()
}

/// Send `prompt` and wait for the answer that ends its turn. Returns that answer.
fn ask_and_wait(harness: &mut PtyHarness, grok_home: &Path, prompt: &str) -> String {
    harness.inject_keys(prompt.as_bytes()).expect("type prompt");
    harness.update(Duration::from_millis(500));
    harness.inject_keys(b"\r").expect("submit prompt");
    let mut stable_since: Option<(usize, std::time::Instant)> = None;
    let mut answer = String::new();
    let answered = wait_until(Duration::from_secs(600), || {
        harness.update(Duration::from_millis(500));
        let history = history_holding(grok_home, prompt);
        let Some(last) = history.last() else {
            return false;
        };
        let text = last["content"].as_str().unwrap_or_default().trim();
        if last["type"] != "assistant" || !tool_call_names(last).is_empty() || text.is_empty() {
            stable_since = None;
            return false;
        }
        match stable_since {
            Some((len, since)) if len == history.len() => {
                answer = text.to_owned();
                since.elapsed() >= Duration::from_secs(4)
            }
            _ => {
                stable_since = Some((history.len(), std::time::Instant::now()));
                false
            }
        }
    });
    assert!(
        answered,
        "the live model must finish `{prompt}`\nscreen:\n{}",
        harness.screen_contents()
    );
    answer
}

/// The recall note lines that follow `prompt` in its session.
fn notes_recalled_for(grok_home: &Path, prompt: &str) -> Vec<String> {
    let history = history_holding(grok_home, prompt);
    let mut after_prompt = false;
    let mut notes = Vec::new();
    for item in history.iter().filter(|item| item["type"] == "user") {
        let text = item.to_string();
        if text.contains(RECALL_OPEN_TAG) {
            if after_prompt {
                notes.extend(
                    text.split("\\n")
                        .filter(|line| line.trim_start().starts_with("- ") && line.contains("ref "))
                        .map(|line| line.trim().to_owned()),
                );
            }
        } else {
            after_prompt = text.contains(prompt);
        }
    }
    notes
}

/// Every memory manifest under `grok_home`, for the test output.
fn manifests(grok_home: &Path) -> String {
    let mut out = String::new();
    let mut pending = vec![grok_home.join("memory-v2")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "MEMORY.md") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                out.push_str(&format!("--- {}\n{text}\n", path.display()));
            }
        }
    }
    out
}

/// One session states a preference and a location in passing. A later session, in a new process, is asked where
/// the thing is and is given a task that the preference applies to. Nothing but memory carries both facts over.
#[cfg(unix)]
#[test]
#[ignore = "live model; needs GROK_LIVE_BASE_URL, GROK_LIVE_MODEL and GROK_LIVE_KEY_ENV"]
fn live_later_session_knows_what_an_earlier_session_was_told_in_passing() {
    let base_url = required_env("GROK_LIVE_BASE_URL");
    let model = required_env("GROK_LIVE_MODEL");
    let key_env = required_env("GROK_LIVE_KEY_ENV");
    let key = required_env(&key_env);

    let sandbox = TestSandbox::new();
    let grok_home = sandbox.grok_home().to_path_buf();
    // Memory is off for a workspace in the system temp directory.
    let workspace = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("live-memory-carry-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("create the workspace");
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

    let mut env: Vec<(String, String)> = vec![
        (key_env.clone(), key),
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
    let spawn = || {
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
        harness
    };
    let quit = |mut harness: PtyHarness| {
        harness.inject_keys(b"\x11").expect("ctrl-q once");
        harness.update(Duration::from_millis(300));
        harness.inject_keys(b"\x11").expect("ctrl-q confirm");
        let _ = harness.quit();
    };

    let mut harness = spawn();
    for prompt in EARLIER_PROMPTS {
        let notes_before = notes_written_by_the_session(&grok_home).len();
        ask_and_wait(&mut harness, &grok_home, prompt);
        // Capture runs after the turn.
        wait_until(Duration::from_secs(90), || {
            harness.update(Duration::from_millis(500));
            notes_written_by_the_session(&grok_home).len() > notes_before
        });
    }
    quit(harness);
    let captured = notes_written_by_the_session(&grok_home);
    println!("LIVE carry, notes of the earlier session: {captured:?}");
    for note in &captured {
        let text = std::fs::read_to_string(note).unwrap_or_default();
        println!("--- {}\n{text}", note.display());
    }
    println!("LIVE carry, manifests:\n{}", manifests(&grok_home));
    // The later session must not learn the prefix from the file.
    for entry in std::fs::read_dir(&workspace)
        .into_iter()
        .flatten()
        .flatten()
    {
        let _ = std::fs::remove_file(entry.path());
    }

    let mut harness = spawn();
    let where_answer = ask_and_wait(&mut harness, &grok_home, LATER_PROMPTS[0]);
    let _ = ask_and_wait(&mut harness, &grok_home, LATER_PROMPTS[1]);
    quit(harness);

    let created: Vec<String> = std::fs::read_dir(&workspace)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for prompt in LATER_PROMPTS {
        println!(
            "LIVE carry, `{prompt}` recalled {:?}",
            notes_recalled_for(&grok_home, prompt)
        );
    }
    println!("LIVE carry, answer to the location prompt: {where_answer}");
    println!("LIVE carry, files the later session created: {created:?}");
    let _ = std::fs::remove_dir_all(&workspace);

    assert!(
        !captured.is_empty(),
        "the earlier session must write a note with no request to remember"
    );
    assert!(
        where_answer.contains(DEPLOY_SCRIPT),
        "the later session must know where the deploy script is: {where_answer}"
    );
    assert!(
        created.iter().any(|name| name.starts_with(SCRATCH_PREFIX)),
        "the later session must name its scratch file with the stated prefix: {created:?}"
    );
}

/// A live model works through prompts with tools while capture writes notes from the same conversation. One prompt
/// asks about a stored note. That note is recalled for it, and nothing is recalled for any other prompt.
#[cfg(unix)]
#[test]
#[ignore = "live model; needs GROK_LIVE_BASE_URL, GROK_LIVE_MODEL and GROK_LIVE_KEY_ENV"]
fn live_session_recalls_a_note_only_for_the_prompt_that_asks_about_it() {
    let base_url = required_env("GROK_LIVE_BASE_URL");
    let model = required_env("GROK_LIVE_MODEL");
    let key_env = required_env("GROK_LIVE_KEY_ENV");
    let key = required_env(&key_env);

    let sandbox = TestSandbox::new();
    let grok_home = sandbox.grok_home().to_path_buf();
    // Memory is off for a workspace in the system temp directory.
    let workspace =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("live-memory-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("create the workspace");
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
    let topics = grok_home.join("memory-v2").join("global").join("topics");
    std::fs::create_dir_all(&topics).expect("create the topics directory");
    std::fs::write(
        topics.join(RELEASE_NOTE),
        "# Release steps\nHow a release is cut.\n\n## Zebrafish deploy pipeline\n\
         - The zebrafish deploy pipeline starts with `make quokka-release`.\n",
    )
    .expect("write the release note");
    std::fs::write(
        topics.join(EDITOR_NOTE),
        "# Editor setup\nHow the editor is set up.\n\n## Formatting\n\
         - The formatter rewrites a text file on save and keeps every line under a fixed length.\n",
    )
    .expect("write the editor note");

    let mut env: Vec<(String, String)> = vec![
        (key_env.clone(), key),
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

    for (index, prompt) in MEMORY_PROMPTS.iter().enumerate() {
        let notes_before = notes_written_by_the_session(&grok_home).len();
        harness.inject_keys(prompt.as_bytes()).expect("type prompt");
        harness.update(Duration::from_millis(500));
        harness.inject_keys(b"\r").expect("submit prompt");

        // The turn is over when its last item is an answer with no tool call, and nothing follows it.
        let mut stable_since: Option<(usize, std::time::Instant)> = None;
        let answered = wait_until(Duration::from_secs(600), || {
            harness.update(Duration::from_millis(500));
            let history = chat_history_if_any(&grok_home);
            let asked = history
                .iter()
                .rposition(|item| item["type"] == "user" && item.to_string().contains(prompt));
            let ended = asked.is_some()
                && history.last().is_some_and(|item| {
                    item["type"] == "assistant"
                        && tool_call_names(item).is_empty()
                        && !item["content"]
                            .as_str()
                            .unwrap_or_default()
                            .trim()
                            .is_empty()
                });
            if !ended {
                stable_since = None;
                return false;
            }
            match stable_since {
                Some((len, since)) if len == history.len() => {
                    since.elapsed() >= Duration::from_secs(4)
                }
                _ => {
                    stable_since = Some((history.len(), std::time::Instant::now()));
                    false
                }
            }
        });
        assert!(
            answered,
            "the live model must finish prompt {}\nscreen:\n{}",
            index + 1,
            harness.screen_contents()
        );
        // Capture runs after the turn. The next prompt must find its note on disk.
        wait_until(Duration::from_secs(90), || {
            harness.update(Duration::from_millis(500));
            notes_written_by_the_session(&grok_home).len() > notes_before
        });
    }

    let history = chat_history(&grok_home);
    let recalled = recalled_notes_by_prompt(&history);
    let captured = notes_written_by_the_session(&grok_home);
    println!("LIVE memory session, notes written from it: {captured:?}");
    for (index, notes) in recalled.iter().enumerate() {
        println!(
            "LIVE prompt {}: {} recalled {notes:?}",
            index + 1,
            notes.len()
        );
    }

    harness.inject_keys(b"\x11").expect("ctrl-q once");
    harness.update(Duration::from_millis(300));
    harness.inject_keys(b"\x11").expect("ctrl-q confirm");
    let _ = harness.quit();
    let _ = std::fs::remove_dir_all(&workspace);

    assert!(
        !captured.is_empty(),
        "capture must write a note from this conversation, or the session proves nothing about them"
    );
    for (index, notes) in recalled.iter().enumerate() {
        if index == 2 {
            assert!(
                notes.len() == 1 && notes.iter().all(|note| note.contains(RELEASE_NOTE)),
                "the prompt about the deploy pipeline must recall the release note and nothing else: {notes:?}"
            );
        } else {
            assert!(
                notes.is_empty(),
                "prompt {} is about no stored note, and recalled {notes:?}",
                index + 1
            );
        }
    }
}
