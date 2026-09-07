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
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    use std::os::unix::net::UnixStream;

    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    // Two handles to the worker's end of the socket: one for the child's
    // stdin, one for its stdout — both the same connection, exactly as the
    // host-side `spawn_ci_host` wires them before the jail exec.
    let child_stdin = theirs.try_clone().expect("clone stdin");
    let child_stdout = theirs.try_clone().expect("clone stdout");
    let stdin_fd: std::os::unix::io::RawFd = {
        use std::os::unix::io::AsRawFd as _;
        child_stdin.as_raw_fd()
    };
    let stdout_fd: std::os::unix::io::RawFd = {
        use std::os::unix::io::AsRawFd as _;
        child_stdout.as_raw_fd()
    };

    let exe = std::env::current_exe().expect("current test binary");
    let mut child = std::process::Command::new(exe);
    child
        .env(xai_grok_sandbox::ci_host::CI_HOST_MARKER_ENV, "1")
        .arg("--exact")
        .arg("ci_host_worker_self_entry")
        .stdin(unsafe { std::process::Stdio::from_raw_fd(stdin_fd) })
        .stdout(unsafe { std::process::Stdio::from_raw_fd(stdout_fd) })
        .stderr(std::process::Stdio::null());
    // `into_raw_fd` prevents the parent's copies from closing the peer.
    let _ = (child_stdin.into_raw_fd(), child_stdout.into_raw_fd(), theirs.into_raw_fd());
    let mut child = child.spawn().expect("spawn worker child");

    // Ask the worker for a branch. Whatever `gh` does (present or not), the
    // client must get a framed one-line answer and never hang.
    let got = xai_grok_sandbox::ci_host::query_ci_host_stream(ours, "feature/ci-host");
    // `query_ci_host_stream` owns `ours`; dropping it on return closes our end
    // of the socket, so the worker's persistent read loop hits EOF and exits.
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
}

/// Delegate that the parent spawns: re-enter the real shipped worker loop.
#[test]
fn ci_host_worker_self_entry() {
    if !xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        return; // only meaningful when spawned as the worker
    }
    xai_grok_sandbox::ci_host::run_ci_host_worker();
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
