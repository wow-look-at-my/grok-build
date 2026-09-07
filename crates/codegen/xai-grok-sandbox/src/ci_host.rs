//! Unsandboxed `gh` CI-status host worker for `--sandbox` sessions.
//!
//! A bare `--sandbox` re-execs the whole binary inside bwrap / Seatbelt, so
//! an in-process `gh` call (or any `Command::new("gh")` reached from the
//! jailed renderer) runs *inside* the jail, where it cannot reach the host
//! credentials / git remote / network that power the real CI dot. Yet the dot
//! must keep polling all session long, and it must stay read-only and bounded.
//!
//! The fix is a long-lived host worker started on the *host* side, moments
//! before the re-exec, and handed into the jail as an already-open Unix
//! socketpair FD that survives `exec`. The worker's one and only privilege is
//! to run `gh run list --json` for the current branch in the session repo; it
//! cannot run arbitrary commands, only that fixed query shape. The jailed
//! pager asks it for a fresh result on every poll and never spawns `gh`
//! itself while sandboxed.
//!
//! Protocol (one `UnixStream`, newline-delimited, request/response):
//!   request  : `gh-status <HEAD_BRANCH>\n`
//!   response : one line of JSON (the raw `gh run list --json` array), or a
//!              single `.` when the run produced nothing usable.
//!
//! The worker loops until its stream write end closes (the jailed process
//! exited), then exits. Nothing is ever trusted from a request beyond the
//! tracked branch token; the query shape is fixed in this module.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Env var set on the re-executed jailed binary naming the inherited fd of
/// the host worker's stream. Its presence (>= 0) means "talk to the host
/// worker rather than spawning `gh`". Survives `exec` because the fd itself
/// is open across it.
pub const CI_HOST_FD_ENV: &str = "GROK_CI_HOST_FD";
/// Env var set on the host worker itself so a re-entry of `main` knows it is
/// the worker and must not build another jail / worker. Never reaches the
/// jailed process.
pub const CI_HOST_MARKER_ENV: &str = "GROK_CI_HOST_SUBPROCESS";

/// The max size of a single `gh` run-list JSON document we accept. `gh run
/// list --limit 10` is tiny (a few KB); this is a generous cap that still
/// bounds how much a hostile/misbehaving worker may dump into the jail.
const MAX_RESPONSE_BYTES: usize = 1 << 20; // 1 MiB

/// Whether this process is the host CI worker (its `main` should run the
/// worker loop and exit rather than start a session).
pub fn is_ci_host_subprocess() -> bool {
    std::env::var_os(CI_HOST_MARKER_ENV).is_some()
}

/// Host-side spawn, called from `main` immediately before the jail re-exec.
///
/// Returns the fd the jailed process should inherit to reach the worker, or
/// `None` when no worker could be started (the jailed pager then falls back
/// to its normal in-jail `gh`, which degrades to "off" under the jail — the
/// dot simply does not show, exactly as if `gh` were missing).
///
/// `repo_root` is where the worker runs `gh` (so `gh` discovers the remote
/// from the git repo there), and what the worker uses as its cwd. Callers
/// must only call this when a jail is about to be built; it is a no-op for
/// the (already) jailed worker process re-entry (guarded by
/// [`is_ci_host_subprocess`]).
pub fn spawn_ci_host(repo_root: &Path) -> Option<i32> {
    if is_ci_host_subprocess() {
        return None;
    }
    use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    let _ = repo_root;
    let exe = std::env::current_exe().ok()?;
    let (ours, theirs) = UnixStream::pair().ok()?;
    let our_fd: RawFd = ours.as_raw_fd();
    let theirs_fd: RawFd = theirs.into_raw_fd();

    let mut cmd = std::process::Command::new(exe);
    cmd.env(CI_HOST_MARKER_ENV, "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `theirs_fd` is a valid open fd and the closure only calls
        // the async-signal-safe `dup2`.
        unsafe {
            cmd.pre_exec(move || {
                dup2_to_stdin_stdout(theirs_fd);
                Ok(())
            });
        }
    }

    match cmd.spawn() {
        Ok(_) => {
            // Leak `ours` so the fd stays open across the jail `exec`; the
            // jailed process rebuilt from `GROK_CI_HOST_FD` owns it onward.
            std::mem::forget(ours);
            Some(our_fd)
        }
        Err(_) => {
            // Close the child peer we never handed to a running worker; let
            // `ours` drop normally.
            unsafe {
                OwnedFd::from_raw_fd(theirs_fd);
            }
            None
        }
    }
}

/// `pre_exec` helper: point stdin (0) and stdout (1) at the given fd.
///
/// Runs in the child just after fork, before exec, so it is async-signal-safe
/// (only `dup2` here).
#[cfg(unix)]
fn dup2_to_stdin_stdout(fd: std::os::unix::io::RawFd) {
    // SAFETY: `fd` is a valid open fd; dup2 of it to 0 and 1 is safe.
    unsafe {
        libc::dup2(fd, 0);
        libc::dup2(fd, 1);
    }
}

/// Run the host CI worker: serve the fixed `gh` status query on stdin/stdout
/// (both the inherited socket fd) until EOF, then return.
///
/// This is the *only* code the unsandboxed worker runs. It has no access to
/// the jailed process's session, tools, or file system beyond `repo_root`
/// (its cwd); the sole external action is the fixed `gh` invocation.
pub fn run_ci_host_worker() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF: the jailed process (and its fd) is gone.
            Ok(_) => {}
            Err(_) => break,
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut out = stdout.lock();
        handle_request(line, &mut out);
        let _ = out.flush();
    }
}

/// Serve one request line, writing the single-line answer to `out`. Pure and
/// testable: drives the fixed `gh` query for `gh-status <branch>` and answers
/// the `.` nothing-usable sentinel for anything else (confinement).
pub fn handle_request<W: Write>(line: &str, out: &mut W) {
    if let Some(branch) = line.strip_prefix("gh-status ") {
        let payload = query_branch(branch).unwrap_or_else(|| vec![b'.']);
        // A lone `.` is the "nothing usable" sentinel (see module docs).
        let _ = out.write_all(&payload);
        let _ = out.write_all(b"\n");
    } else {
        // Unknown request: answer nothing usable so the jailed side
        // degrades to "off" rather than hanging or trusting us.
        let _ = out.write_all(b".\n");
    }
}

/// Run `gh run list --json` for `branch` in the worker's cwd and return the
/// raw stdout bytes, or `None` when `gh` failed / was unavailable. The branch
/// token is validated so only a plausible branch name is ever interpolated
/// into argv.
fn query_branch(branch: &str) -> Option<Vec<u8>> {
    if !valid_branch_token(branch) {
        return None;
    }
    let mut cmd = std::process::Command::new("gh");
    cmd.args([
        "run",
        "list",
        "--branch",
        branch,
        "--limit",
        "10",
        "--json",
        "status,conclusion,headBranch,workflowName",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = output.stdout;
    (stdout.len() <= MAX_RESPONSE_BYTES && !stdout.is_empty()).then_some(stdout)
}

/// A request may only carry a bounded, git-safe branch token — never a bare
/// line a caller could turn into an argv injection. `gh` validates the branch
/// server-side too, but the shape guard belongs here.
fn valid_branch_token(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 256
        && branch
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'\n' && b != b'\r')
}

/// Jailed-side read: query the host worker over the inherited stream for
/// `branch`. Returns the raw single-line JSON array, or `None` so the caller
/// degrades to the "off" state (worker missing, failed, or unusable output).
///
/// Takes the stream by borrow: the session reuses one open connection for
/// every poll, and the owner keeps it alive for the life of the process.
#[cfg(unix)]
pub fn query_ci_host_stream(stream: std::os::unix::net::UnixStream, branch: &str) -> Option<Vec<u8>> {
    let mut stream = stream;
    let mut line = String::from("gh-status ");
    line.push_str(branch);
    line.push('\n');
    stream.write_all(line.as_bytes()).ok()?;
    stream.flush().ok()?;

    // Read one line, capped. A single worker response carries the whole JSON
    // array and a terminating newline.
    let mut reader = BufReader::new(&stream);
    let mut response = Vec::new();
    // Read up to the cap in a bounded loop so a misbehaving worker cannot
    // grow the jail's memory without limit.
    loop {
        let n = match reader.read_until(b'\n', &mut response) {
            Ok(n) => n,
            Err(_) => return None,
        };
        if n > 0 {
            break;
        }
        // read_until returns 0 only on a true EOF (worker exited without
        // answering) — treat as failure.
        return None;
    }
    if response.last() == Some(&b'\n') {
        response.pop();
    }
    if !response.is_empty() && response != vec![b'.'] {
        Some(response)
    } else {
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A tiny `Write` that just captures bytes, so the worker handler is
    /// tested without touching a real socket or `gh`.
    struct Sink(Vec<u8>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn answer(line: &str) -> Vec<u8> {
        let mut sink = Sink(Vec::new());
        handle_request(line, &mut sink);
        sink.0
    }

    #[test]
    fn worker_answers_dot_for_any_request_that_is_not_gh_status() {
        // The confined worker must never run an arbitrary command. A non
        // `gh-status` request is answered with the nothing-usable sentinel —
        // the jailed side degrades to "off", and nothing is executed.
        for line in [
            "run list --json",
            "--branch master",
            "gh run list",
            "gh-status",
            "rm -rf /",
            "mission run --json",
            "status ",
        ] {
            assert_eq!(answer(line), b".\n", "line {line:?} must be refused");
        }
    }

    #[test]
    fn worker_refuses_invalid_branch_tokens() {
        // Ever with the `gh-status` prefix, a token that could not be a real
        // branch is rejected before `gh` is ever invoked (returns `.`).
        for line in [
            "gh-status ",
            "gh-status aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "gh-status evil\nbranch",
            "gh-status has space",
        ] {
            assert_eq!(answer(line), b".\n", "token {line:?} must be refused");
        }
    }

    #[test]
    fn valid_tokens_are_accepted_and_junk_rejected() {
        assert!(valid_branch_token("feature/gh-ci-monitor"));
        assert!(valid_branch_token("master"));
        assert!(valid_branch_token("release/2026-09"));
        assert!(!valid_branch_token(""));
        let long = "a".repeat(300);
        assert!(!valid_branch_token(&long));
        assert!(!valid_branch_token("evil\nbranch"));
        assert!(!valid_branch_token("evil\rbranch"));
        assert!(!valid_branch_token("has space"));
    }

    #[test]
    fn query_ci_host_stream_roundtrips_a_result() {
        // A real host worker does the same: read a request line, answer one
        // JSON line. Drive the jailed-side reader against that contract.
        let (ours, theirs) = UnixStream::pair().expect("pair");
        std::thread::spawn(move || {
            let mut stream = theirs;
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"[{\"status\":\"completed\",\"conclusion\":\"success\"}]\n");
            let _ = stream.flush();
        });
        let got = query_ci_host_stream(ours, "master").expect("read");
        let text = String::from_utf8(got).expect("utf8");
        assert!(text.contains("\"success\""));
    }

    #[test]
    fn query_ci_host_stream_dot_sentinel_is_off() {
        let (ours, theirs) = UnixStream::pair().expect("pair");
        std::thread::spawn(move || {
            let mut stream = theirs;
            let mut buf = [0u8; 64];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b".\n");
            let _ = stream.flush();
        });
        assert_eq!(query_ci_host_stream(ours, "master"), None);
    }

    #[test]
    fn query_ci_host_stream_eof_is_off() {
        let (ours, _theirs) = UnixStream::pair().expect("pair");
        // Drop `theirs` first so the reader sees EOF.
        drop(_theirs);
        assert_eq!(query_ci_host_stream(ours, "master"), None);
    }
}