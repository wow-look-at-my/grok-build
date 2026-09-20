//! Drives the shipped startup path that starts the CI host worker.
//!
//! `xai_grok_shell::config::apply_sandbox` is what the pager's `main` calls to
//! confine a session. It starts the unsandboxed `gh` worker before the
//! confinement is installed, because the macOS profile it applies denies the
//! keychain mach services and a `gh` running under it has no token to send:
//! every CI query would answer `401`. This test runs that function for real, in
//! a child process, and then asks the questions the session itself asks:
//!
//!   * does the session find the worker (`ci_host::ci_host_fd`)?
//!   * does the shipped query path (`ci_host::run_gh`) get a framed answer?
//!   * is the process actually confined (the login keychain out of reach)?
//!
//! Two profiles are run. `workspace` is the one `--sandbox=workspace` uses, and
//! it proves the whole thing on a host that can install a profile. `devbox` is
//! the profile whose apply never refuses, so the hand-off is exercised even on a
//! host that is already confined and cannot nest a second profile.
//!
//! `harness = false` (see Cargo.toml): the worker child is this binary
//! re-entered with the marker env var set and nothing else, which is how the
//! shipped spawn starts it, and only a hand-written `main` can dispatch that.

#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

/// Which side of the test a spawned child runs. Absent means "the parent".
const MODE_ENV: &str = "GROK_CI_HOST_STARTUP_MODE";
const PROFILE_ENV: &str = "GROK_CI_HOST_STARTUP_PROFILE";
const WORKSPACE_ENV: &str = "GROK_CI_HOST_STARTUP_WORKSPACE";
const BRANCH_ENV: &str = "GROK_CI_HOST_STARTUP_BRANCH";

const MODE_SESSION: &str = "session";

const REPORT: &str = "sandbox-ci-host-startup: ";

fn main() {
    // A worker child re-enters this binary with the marker set and nothing
    // else, exactly as the pager's `main` dispatches it.
    if xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        xai_grok_sandbox::ci_host::run_ci_host_worker();
        std::process::exit(0);
    }
    #[cfg(target_os = "macos")]
    match std::env::var(MODE_ENV).as_deref() {
        Ok(MODE_SESSION) => session_child(),
        _ => parent(),
    }
    #[cfg(not(target_os = "macos"))]
    println!("{REPORT}skip: the profile sandbox is a macOS Seatbelt profile");
}

/// The confined side: run the shipped startup call, then the session's own
/// questions about the worker it should have.
#[cfg(target_os = "macos")]
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

/// Whether the `gh` the worker runs exists on this host. With none, the worker
/// still answers, but every query it runs comes back as its nothing-usable
/// sentinel and no answer can be asserted.
#[cfg(target_os = "macos")]
fn gh_is_installed() -> bool {
    std::process::Command::new("gh")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A child's report.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct SessionReport {
    status: std::process::ExitStatus,
    fd: String,
    keychain: String,
    answered: String,
    gh: String,
    stderr: String,
}

#[cfg(target_os = "macos")]
impl SessionReport {
    fn summary(&self) -> String {
        format!(
            "status={} fd={} keychain={} answered={} gh={} stderr={:?}",
            self.status, self.fd, self.keychain, self.answered, self.gh, self.stderr
        )
    }

    fn refused_to_start(&self) -> bool {
        self.stderr.contains("could not apply the")
    }
}

#[cfg(target_os = "macos")]
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
        assert_eq!(
            workspace_leg.keychain, "denied",
            "a `--sandbox=workspace` session must be confined (the login \
             keychain out of reach): {}",
            workspace_leg.summary()
        );
        assert_answered(&workspace_leg);
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
    println!("{REPORT}PASS");
}

/// The session's query has to reach the worker over the fd it published. With
/// no `gh` on the host there is nothing for the worker to run, so only the fd is
/// asserted and the leg says so.
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
fn repo_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = dunce::canonicalize(root).ok()?;
    root.join(".git").exists().then_some(root)
}

#[cfg(target_os = "macos")]
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
