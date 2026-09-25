//! `grok update` is a recovery command: a config failure must not block it.
//!
//! A local server serves the channel pointer and records each request path.
//! The config test serves the binary's own version, so a healthy run exits 0 ("already up to date").
//! A run with a corrupt config must exit 0 too; reintroducing a config `?` fails exactly that run.
//! The pointer must equal the current version: the installer converges in both directions, so an older pointer triggers a downgrade attempt.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use xai_grok_pager_pty_harness::pager_binary;

// A child forked while the copy's write fd is open, even pager_binary's cargo build, fails the copy's exec with "Text file busy".
static EXEC_LOCK: Mutex<()> = Mutex::new(());

/// Spawn a local server that answers every request with the channel pointer body and records each request path.
fn spawn_pointer_server(
    body: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<String>>>,
) -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = listener.try_clone().unwrap();
    std::thread::spawn(move || {
        for stream in serving.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            if let Some(path) = request_line.split_whitespace().nth(1) {
                requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(path.to_owned());
            }
            // Drain headers: unread input at close resets the connection and can drop the reply.
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 0) && header != "\r\n" {
                header.clear();
            }
            let version = body.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let _ = (&stream).write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    version.len(),
                    version
                )
                .as_bytes(),
            );
        }
    });
    (listener, base)
}

/// `exe` with an isolated `home`, pointed at the local pointer base.
fn grok_command(exe: &Path, home: &Path, base: &str) -> Command {
    let mut command = Command::new(exe);
    command
        .env_clear()
        .env("HOME", home)
        .env("GROK_HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GROK_CLI_BASE_URL", base);
    xai_tty_utils::detach_std_command(&mut command);
    command
}

/// Run `command` to completion, holding [`EXEC_LOCK`] for the spawn only.
fn output(mut command: Command) -> Output {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[allow(clippy::disallowed_methods)] // waited on right below
    let child = {
        let _exec = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        command.spawn().expect("spawn grok")
    };
    child.wait_with_output().expect("wait for grok")
}

/// Run `grok update` in a fresh isolated home against the local pointer base.
fn run_update(base: &str, config_toml: &str, extra_args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), config_toml).unwrap();
    let exe = {
        let _exec = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        pager_binary().expect("resolve pager binary")
    };
    let mut command = grok_command(&exe, home.path(), base);
    command.arg("update").args(extra_args);
    output(command)
}

/// The valid run proves the environment resolves to success, so a nonzero corrupt run can only mean a config failure aborted the update.
#[test]
fn corrupt_config_never_changes_update_outcome() {
    let body = Arc::new(Mutex::new("0.0.1".to_owned()));
    let (_listener, base) = spawn_pointer_server(body.clone(), Arc::default());

    // Probe the binary's own version so the pointer matches it exactly.
    let check = run_update(&base, "[cli]\n", &["--check", "--json"]);
    let status: Value = serde_json::from_slice(&check.stdout)
        .unwrap_or_else(|e| panic!("update --check --json must emit JSON: {e}"));
    let current = status["currentVersion"]
        .as_str()
        .expect("currentVersion in update --check --json")
        .to_owned();
    *body.lock().unwrap_or_else(|e| e.into_inner()) = current;

    let valid = run_update(&base, "[cli]\n", &[]);
    assert!(
        valid.status.success(),
        "healthy grok update against the local base must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&valid.stdout),
        String::from_utf8_lossy(&valid.stderr)
    );

    let corrupt = run_update(&base, "this is not toml {{{[[[", &[]);
    assert!(
        corrupt.status.success(),
        "a corrupt config.toml must not block grok update\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&corrupt.stdout),
        String::from_utf8_lossy(&corrupt.stderr)
    );
}
