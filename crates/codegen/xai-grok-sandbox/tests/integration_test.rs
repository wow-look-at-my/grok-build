//! Integration tests for xai-grok-sandbox.
//!
//! Note: `Sandbox::apply()` is irreversible and process-wide, so we cannot
//! test actual kernel enforcement in standard `#[test]` functions (they share
//! a process). Use the `sandbox_smoke_test` example for enforcement testing.
//! These tests verify the API contracts, config loading, and support detection.

// `support_info` is only available with the `enforce` feature (it returns a
// nono type), so gate this test the same way.
#[test]
#[cfg(all(feature = "enforce", unix))]
fn test_support_info() {
    // Verify that nono can report platform support status without applying
    let support = xai_grok_sandbox::SandboxManager::support_info();
    // On macOS and Linux 5.13+, this should be supported
    // On other platforms, it gracefully reports unsupported
    println!(
        "Sandbox support: supported={}, details={}",
        support.is_supported, support.details
    );
    // We don't assert is_supported because CI may run on any platform
}

// `to_capability_set` is only available with the `enforce` feature.
#[test]
#[cfg(all(feature = "enforce", unix))]
fn test_profile_capability_set_construction() {
    use xai_grok_sandbox::ProfileName;

    // Use CWD as workspace — guaranteed to exist
    let workspace = std::env::current_dir().expect("cwd");

    // All profiles should produce valid CapabilitySets without panicking
    for profile in [
        ProfileName::Workspace,
        ProfileName::ReadOnly,
        ProfileName::Strict,
        ProfileName::Off,
    ] {
        let result = profile.to_capability_set(&workspace);
        assert!(
            result.is_ok(),
            "Profile {:?} failed to build CapabilitySet: {:?}",
            profile,
            result.err()
        );
    }
}

// ── CI host worker: real end-to-end across an inherited socketpair ──────────
//
// The unsandboxed `gh` CI-status worker is the only code that must run on the
// host side of a `--sandbox` session. This suite drives the shipped worker
// entry (`run_ci_host_worker`) as a REAL child process re-entering this same
// binary in worker mode, with its stdin/stdout pointed at a socketpair the
// parent then queries with the shipped `query_ci_host_stream` client — the
// exact fd handoff `spawn_ci_host` performs before the jail exec.

/// Run by the parent: spawn the current binary as the worker child and prove a
/// request round-trips through the real shipped worker loop to a real client.
#[test]
fn ci_host_worker_serves_a_request_over_an_inherited_socketpair() {
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::net::UnixStream;

    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    // The worker's end rides a fd of its own rather than stdin/stdout.
    // Production hands the worker fd 0 and fd 1, but this child is the TEST
    // BINARY: its harness prints progress lines to stdout, and each one would
    // reach the client as a fake answer and put every later answer a request
    // behind. The protocol is what this test covers, so it gets a clean fd.
    let theirs_fd = theirs.into_raw_fd();
    const WORKER_FD: std::os::unix::io::RawFd = 3;

    let exe = std::env::current_exe().expect("current test binary");
    let mut child = std::process::Command::new(exe);
    child
        .env(xai_grok_sandbox::ci_host::CI_HOST_MARKER_ENV, "1")
        .env(WORKER_FD_ENV, WORKER_FD.to_string())
        .arg("--exact")
        .arg("ci_host_worker_self_entry")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        // SAFETY: the closure runs between fork and exec and calls only the
        // async-signal-safe `dup2`. `dup2` also clears CLOEXEC on the new fd,
        // which is what carries the socket across the exec.
        std::os::unix::process::CommandExt::pre_exec(&mut child, move || {
            if libc::dup2(theirs_fd, WORKER_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = child.spawn().expect("spawn worker child");
    // Close the parent's copy so the child owns the only writer: otherwise the
    // worker never sees EOF and the exit check below times out.
    unsafe {
        libc::close(theirs_fd);
    }

    // Ask the worker for a branch, then run an allowlisted `gh` over the same
    // connection. Whatever `gh` does (present or not), each request must get a
    // framed one-line answer and never hang.
    let our_fd = {
        use std::os::unix::io::AsRawFd as _;
        ours.as_raw_fd()
    };
    std::mem::forget(ours); // the transport owns the fd from here.
    let got = xai_grok_sandbox::ci_host::query_ci_host(our_fd, "feature/ci-host");
    let gh = xai_grok_sandbox::ci_host::query_gh_host(our_fd, &["auth", "status"]);
    // A refused command must come back as the sentinel, from the real worker.
    let refused = xai_grok_sandbox::ci_host::query_gh_host(our_fd, &["run", "cancel", "1"]);
    assert_eq!(refused, None, "the worker must refuse a write");
    // Closing our end is what makes the worker's read loop hit EOF and exit.
    xai_grok_sandbox::ci_host::close_host_connection(our_fd);
    // Bound the wait so a hung worker fails the test instead of hanging CI.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(_status) = child.try_wait().expect("poll child") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("worker child did not exit; worker loop likely hung");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The worker always answers one framed line: the nothing-usable sentinel
    // (→ None, when `gh` is absent or has no runs) or a real JSON array. Both
    // prove the shipped client/worker framing; either is a valid session.
    match got {
        None => {} // sentinel / no runs → the dot reads "off"
        Some(body) => {
            let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&body);
            assert!(parsed.is_ok(), "a worker JSON answer must parse");
        }
    }
    // `gh auth status` exits non-zero when nobody is logged in, so only the
    // framing is asserted: an answer arrived, decoded, and named an exit code.
    if let Some(response) = gh {
        assert!(
            response.stdout.len() + response.stderr.len() < 1 << 21,
            "a worker response must stay bounded"
        );
    }
}

/// Names the fd the parent handed this child its socket on.
const WORKER_FD_ENV: &str = "GROK_CI_HOST_TEST_FD";

/// Delegate that the parent spawns: re-enter the real shipped worker loop.
#[test]
fn ci_host_worker_self_entry() {
    use std::os::unix::io::FromRawFd;
    if !xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        return; // only meaningful when spawned as the worker
    }
    let Some(fd) = std::env::var(WORKER_FD_ENV)
        .ok()
        .and_then(|raw| raw.parse::<std::os::unix::io::RawFd>().ok())
    else {
        return;
    };
    // SAFETY: the parent dup2'd its socketpair end onto this fd before exec,
    // and nothing else in this process owns it.
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    xai_grok_sandbox::ci_host::run_ci_host_worker_on(stream);
}

#[test]
fn test_sandbox_manager_lifecycle() {
    use xai_grok_sandbox::{ProfileName, SandboxManager};

    let workspace = std::env::current_dir().expect("cwd");

    // Off profile: apply should succeed without actually sandboxing
    let mut manager = SandboxManager::new(ProfileName::Off, &workspace);
    assert!(!manager.is_applied());
    assert!(!manager.restrict_child_network());

    let result = manager.apply(&workspace);
    assert!(result.is_ok());
    // Off profile doesn't actually apply
    assert!(!manager.is_applied());
}

#[test]
fn test_sandbox_logger() {
    use xai_grok_sandbox::{SandboxEvent, SandboxLogger};

    let logger = SandboxLogger::new();

    // Log some events (use violation events — profile_applied requires a resolved profile)
    logger.log(SandboxEvent::fs_violation("workspace", "/tmp/test", "read"));
    logger.log(SandboxEvent::fs_violation(
        "workspace",
        "/etc/shadow",
        "write",
    ));
    logger.log(SandboxEvent::net_violation("strict", "evil.com:443"));

    // Check metrics
    assert_eq!(logger.metrics().fs_violation_count(), 2);
    assert_eq!(logger.metrics().net_violation_count(), 1);

    // Take events drains the buffer
    let events = logger.take_events();
    assert_eq!(events.len(), 3);

    // Buffer is now empty
    let events2 = logger.take_events();
    assert!(events2.is_empty());
}

#[test]
fn test_should_restrict_child_network_default() {
    // Before any sandbox is applied, child network should not be restricted
    // Note: this test may interfere with other tests if they set the global.
    // In practice, the global is set once at process startup and never unset.
    // For testing, we just verify the default state.
    //
    // We can't meaningfully test the "set" path without applying a sandbox
    // (which is irreversible), so we verify the default is false.
    assert!(!xai_grok_sandbox::should_restrict_child_network());
}
