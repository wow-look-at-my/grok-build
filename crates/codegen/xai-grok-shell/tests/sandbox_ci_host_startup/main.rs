//! Drives the shipped startup path that starts the CI host worker.
//!
//! `apply_sandbox` starts the unsandboxed `gh` worker before it confines the
//! session. This runs it for real, in a child, and asks whether the session
//! finds the worker, gets a framed answer, is confined, and — the hand-off's
//! contract — keeps the worker away from its OWN children.
//!
//! A session confined IN PLACE keeps it. Only a Linux bwrap re-exec is handed
//! the fd's number, and there the jail is the confinement. Both unix platforms
//! run this: macOS installs a Seatbelt profile, Linux applies Landlock and
//! re-execs only for a profile carrying denials.
//!
//! `harness = false` (see Cargo.toml): the worker child is this binary
//! re-entered with the marker env var set and nothing else, and only a
//! hand-written `main` can dispatch that.

#[cfg(unix)]
use std::path::{Path, PathBuf};

/// Which side of the test a spawned child runs. Absent means "the parent".
#[cfg(unix)]
const MODE_ENV: &str = "GROK_CI_HOST_STARTUP_MODE";
#[cfg(unix)]
const PROFILE_ENV: &str = "GROK_CI_HOST_STARTUP_PROFILE";
#[cfg(unix)]
const WORKSPACE_ENV: &str = "GROK_CI_HOST_STARTUP_WORKSPACE";
#[cfg(unix)]
const BRANCH_ENV: &str = "GROK_CI_HOST_STARTUP_BRANCH";

#[cfg(unix)]
const MODE_SESSION: &str = "session";

const REPORT: &str = "sandbox-ci-host-startup: ";

/// The one case this binary runs, under the name a test runner lists it by.
const TEST_NAME: &str = "the_shipped_startup_path_hands_a_confined_session_its_worker";

fn main() {
    // A worker child re-enters this binary with the marker set and nothing
    // else, exactly as the pager's `main` dispatches it.
    if xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        xai_grok_sandbox::ci_host::run_ci_host_worker();
        std::process::exit(0);
    }
    if serve_list_protocol(TEST_NAME) {
        return;
    }
    #[cfg(unix)]
    match std::env::var(MODE_ENV).as_deref() {
        Ok(MODE_SESSION) => session_child(),
        _ => parent(),
    }
    #[cfg(not(unix))]
    println!("{REPORT}skip: the profile sandbox is a unix confinement");
}

/// Answer the listing a test runner asks for before it runs anything, and say
/// whether that is all this run was.
///
/// `harness = false` leaves the protocol to this binary. nextest lists with
/// `--list --format terse` and refuses a binary that answers with anything but
/// `<name>: test` lines. `cargo test` never lists, which is why a binary that
/// ignores the argument passes there and fails under nextest.
fn serve_list_protocol(name: &str) -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|arg| arg == "--list") {
        return false;
    }
    // A listing of the ignored tests alone. This binary has none to name.
    if !args.iter().any(|arg| arg == "--ignored") {
        println!("{name}: test");
    }
    true
}

/// The confined side: run the shipped startup call, then the session's own
/// questions about the worker it should have.
#[cfg(unix)]
fn session_child() {
    let workspace = PathBuf::from(std::env::var(WORKSPACE_ENV).expect(WORKSPACE_ENV));
    let profile = std::env::var(PROFILE_ENV).unwrap_or_else(|_| "workspace".to_string());
    let branch = std::env::var(BRANCH_ENV).unwrap_or_else(|_| "master".to_string());

    // The shipped startup path, called the way the pager's `main` calls it.
    xai_grok_shell::config::apply_sandbox(None, Some(&profile), Some(&workspace));

    let fd = xai_grok_sandbox::ci_host::ci_host_fd();
    println!(
        "{REPORT}fd={}",
        fd.map_or_else(|| "none".to_string(), |fd| fd.to_string())
    );
    println!("{REPORT}keychain={}", keychain_state());
    println!(
        "{REPORT}gh={}",
        if gh_is_installed() { "present" } else { "absent" }
    );

    // The query the `ci` tool's `status` action issues, over the shipped path.
    let answer = xai_grok_sandbox::ci_host::run_gh(
        &workspace,
        &[
            "run",
            "list",
            "--branch",
            &branch,
            "--limit",
            "10",
            "--json",
            "status,conclusion,headBranch,workflowName",
        ],
    );
    println!("{REPORT}answered={}", u8::from(answer.is_some()));
    if let Some(response) = &answer {
        println!("{REPORT}answer_code={}", response.code);
    }

    // What an ordinary child of this session can see of the worker. A session
    // confined in place keeps the worker to itself: the fd stays close-on-exec
    // and its number never reaches the environment. Only a re-exec into bwrap
    // is handed the number, and there the jail is the confinement.
    println!(
        "{REPORT}reexeced={}",
        u8::from(xai_grok_sandbox::is_inside_bwrap())
    );
    let probe = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("printf '%s' \"${GROK_CI_HOST_FD:-unset}\"")
        .output();
    println!(
        "{REPORT}grandchild_env={}",
        match &probe {
            Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
            Err(e) => format!("probe-failed-{e}"),
        }
    );
    println!("{REPORT}child_done");
}

/// Whether the login keychain is reachable from inside this process. `security`
/// answers a bare "valid parameters" error rather than a search list when the
/// keychain mach services are denied, which is what the profile does.
#[cfg(target_os = "macos")]
fn keychain_state() -> &'static str {
    match std::process::Command::new("/usr/bin/security")
        .arg("list-keychains")
        .output()
    {
        Ok(out) if out.status.success() => "reachable",
        _ => "denied",
    }
}

/// There is no keychain off this platform, so nothing here says whether the
/// confinement took. Linux proves that a different way: see the grandchild
/// probe, which a Landlock-confined session still has to keep the worker from.
#[cfg(all(unix, not(target_os = "macos")))]
fn keychain_state() -> &'static str {
    "n/a"
}

/// Whether the `gh` the worker runs exists on this host. With none, the worker
/// still answers, but every query it runs comes back as its nothing-usable
/// sentinel and no answer can be asserted.
#[cfg(unix)]
fn gh_is_installed() -> bool {
    std::process::Command::new("gh")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A child's report.
#[cfg(unix)]
#[derive(Debug)]
struct SessionReport {
    status: std::process::ExitStatus,
    fd: String,
    keychain: String,
    answered: String,
    gh: String,
    /// What an ordinary child of the session read out of `GROK_CI_HOST_FD`.
    grandchild_env: String,
    /// Whether the session was the image a bwrap re-exec produced.
    reexeced: String,
    stderr: String,
}

#[cfg(unix)]
impl SessionReport {
    fn summary(&self) -> String {
        format!(
            "status={} fd={} keychain={} answered={} gh={} reexeced={} \
             grandchild_env={} stderr={:?}",
            self.status,
            self.fd,
            self.keychain,
            self.answered,
            self.gh,
            self.reexeced,
            self.grandchild_env,
            self.stderr
        )
    }

    fn refused_to_start(&self) -> bool {
        self.stderr.contains("could not apply the")
    }
}

#[cfg(unix)]
fn parent() {
    let workspace = match repo_root() {
        Some(path) => path,
        None => {
            println!("{REPORT}skip: no session workspace");
            return;
        }
    };
    if !xai_grok_sandbox::SandboxManager::support_info().is_supported {
        println!("{REPORT}skip: no kernel sandbox on this host");
        return;
    }
    println!("{REPORT}workspace={}", workspace.display());

    // The profile `--sandbox=workspace` runs, on a host that can install it.
    let workspace_leg = run_session_child("workspace", &workspace);
    println!("{REPORT}workspace_leg={}", workspace_leg.summary());
    if workspace_leg.refused_to_start() {
        // A host that is already confined cannot nest a second profile, and the
        // shipped startup path refuses to run without its protections. That is
        // the host, not the hand-off: the devbox leg below still drives it.
        println!(
            "{REPORT}workspace_leg=unverified here: this host is already confined \
             and cannot nest the profile"
        );
    } else {
        assert!(
            workspace_leg.status.success(),
            "the confined session must run: {}",
            workspace_leg.summary()
        );
        assert_ne!(
            workspace_leg.fd, "none",
            "the shipped startup path must leave the session a worker: {}",
            workspace_leg.summary()
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            workspace_leg.keychain, "denied",
            "a `--sandbox=workspace` session must be confined (the login \
             keychain out of reach): {}",
            workspace_leg.summary()
        );
        assert_answered(&workspace_leg);
        assert_worker_is_the_sessions_alone(&workspace_leg);
        println!("{REPORT}workspace_leg=applied and answering");
    }

    // The profile whose apply never refuses, so the hand-off is driven on hosts
    // that cannot install a second profile at all.
    let devbox_leg = run_session_child("devbox", &workspace);
    println!("{REPORT}devbox_leg={}", devbox_leg.summary());
    assert!(
        devbox_leg.status.success(),
        "the session must run: {}",
        devbox_leg.summary()
    );
    assert_ne!(
        devbox_leg.fd, "none",
        "the shipped startup path must start the worker and publish its fd: {}",
        devbox_leg.summary()
    );
    assert_answered(&devbox_leg);
    assert_worker_is_the_sessions_alone(&devbox_leg);
    println!("{REPORT}PASS");
}

/// A session confined IN PLACE keeps the worker to itself.
///
/// The hand-off makes the worker's fd exec-surviving, and names it in the
/// environment, only where an exec follows. Where none does, doing either
/// leaves every child of the session holding a live socket to an UNCONFINED
/// `gh`, with the number to read it on. That is the whole point of the fd
/// riding a `OnceLock` instead of the environment.
///
/// A session that DID re-exec is the image the jail produced, and there the
/// number is supposed to be in its environment: the jail is the confinement.
#[cfg(unix)]
fn assert_worker_is_the_sessions_alone(report: &SessionReport) {
    if report.reexeced == "1" {
        println!(
            "{REPORT}grandchild=re-exec: the jail owns this session, so the fd's \
             name belongs in its environment ({})",
            report.summary()
        );
        return;
    }
    assert_eq!(
        report.grandchild_env, "unset",
        "a session confined in place must not hand its children the worker: {}",
        report.summary()
    );
}

/// The session's query has to reach the worker over the fd it published. With
/// no `gh` on the host there is nothing for the worker to run, so only the fd is
/// asserted and the leg says so.
#[cfg(unix)]
fn assert_answered(report: &SessionReport) {
    if report.gh == "absent" {
        println!(
            "{REPORT}answer=unverified: no `gh` on this host for the worker to \
             run ({}), so the fd is the whole assertion",
            report.summary()
        );
        return;
    }
    assert_eq!(
        report.answered, "1",
        "the session's CI query must be answered by the worker: {}",
        report.summary()
    );
}

/// Re-enter this binary as a session confined by `profile`, started the way the
/// pager starts one.
#[cfg(unix)]
fn run_session_child(profile: &str, workspace: &Path) -> SessionReport {
    let mut cmd = std::process::Command::new(std::env::current_exe().expect("current exe"));
    cmd.env(MODE_ENV, MODE_SESSION)
        .env(PROFILE_ENV, profile)
        .env(WORKSPACE_ENV, workspace)
        .env(BRANCH_ENV, "master")
        // The startup path materializes hook directories under `$GROK_HOME`;
        // a fixture keeps the session's own home out of a test run.
        .env("GROK_HOME", fixture_dir("grok-home"));
    let output = cmd.output().expect("spawn the session child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut report = SessionReport {
        status: output.status,
        fd: "?".to_string(),
        keychain: "?".to_string(),
        answered: "?".to_string(),
        gh: "?".to_string(),
        grandchild_env: "?".to_string(),
        reexeced: "?".to_string(),
        stderr: clip(&stderr),
    };
    let mut saw_done = false;
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(REPORT) else {
            continue;
        };
        if rest == "child_done" {
            saw_done = true;
        }
        for (key, slot) in [
            ("fd=", &mut report.fd),
            ("keychain=", &mut report.keychain),
            ("answered=", &mut report.answered),
            ("gh=", &mut report.gh),
            ("grandchild_env=", &mut report.grandchild_env),
            ("reexeced=", &mut report.reexeced),
        ] {
            if let Some(value) = rest.strip_prefix(key) {
                *slot = value.split_whitespace().next().unwrap_or("").to_string();
            }
        }
    }
    if output.status.success() {
        assert!(saw_done, "the session child must report back: {stdout}");
    }
    report
}

#[cfg(unix)]
fn clip(text: &str) -> String {
    const MAX: usize = 600;
    if text.len() <= MAX {
        return text.to_string();
    }
    let end = (0..=MAX).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    format!("{}...", &text[..end])
}

/// The repository root, which is the workspace a `--sandbox=workspace` session
/// in this repo confines to.
#[cfg(unix)]
fn repo_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = dunce::canonicalize(root).ok()?;
    root.join(".git").exists().then_some(root)
}

#[cfg(unix)]
fn fixture_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "grok-ci-host-startup-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create fixture dir");
    path
}
