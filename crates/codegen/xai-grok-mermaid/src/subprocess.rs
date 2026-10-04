//! Spawn a child process, optionally feed it stdin, wait up to a wall-clock budget, and reap the whole process group on a breach.

use std::process::{Child, Command};
use std::time::Duration;

use wait_timeout::ChildExt;

/// Why a child subprocess run did not complete successfully.
#[derive(thiserror::Error, Debug)]
pub enum SubprocessError {
    /// The child could not be spawned (binary missing, fork failure, ...).
    #[error("could not spawn child process: {0}")]
    Spawn(std::io::Error),
    /// The child exceeded its wall-clock budget and was killed and reaped.
    #[error("child process timed out")]
    Timeout,
    /// The child ran to completion but exited non-zero.
    #[error("child process exited with {0}")]
    NonZeroExit(std::process::ExitStatus),
    /// Waiting on the child itself failed; the child was reaped defensively.
    #[error("waiting on child process failed: {0}")]
    Wait(std::io::Error),
}

/// Spawn `cmd`, optionally write `stdin_payload` to its stdin, wait up to `timeout`, and reap the process group on a breach.
/// The caller must have configured `cmd` (stdio, env, detach).
/// To pass `stdin_payload`, the caller must set `cmd.stdin(Stdio::piped())`.
pub fn run_with_timeout(
    mut cmd: Command,
    stdin_payload: Option<&[u8]>,
    timeout: Duration,
) -> Result<(), SubprocessError> {
    let mut child = spawn_with_etxtbsy_retry(&mut cmd).map_err(SubprocessError::Spawn)?;

    // Feed stdin from a scoped thread: a child that stops reading would otherwise wedge a `write_all` of a large payload.
    let stdin = child.stdin.take();

    // A payload with no piped stdin would be silently dropped (the caller forgot `cmd.stdin(Stdio::piped())`)
    // Both in-tree callers pipe correctly; the debug_assert makes the mistake loud in debug, and release at least logs the warn
    if stdin_payload.is_some() && stdin.is_none() {
        tracing::warn!(
            target: "mermaid",
            "run_with_timeout: stdin payload supplied but stdin is not piped; payload dropped"
        );
        debug_assert!(
            false,
            "run_with_timeout: stdin_payload supplied but cmd.stdin is not piped (payload dropped)"
        );
    }
    std::thread::scope(|scope| {
        if let (Some(mut sink), Some(payload)) = (stdin, stdin_payload) {
            scope.spawn(move || {
                use std::io::Write as _;
                // Errors are expected if the child exits/dies first; ignore them.
                let _ = sink.write_all(payload);
            });
        }
        wait_and_reap(&mut child, timeout)
    })
}

/// Spawn `cmd`, retrying briefly on `ETXTBSY` ("Text file busy").
#[allow(clippy::disallowed_methods)] // the caller owns the reap
fn spawn_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<Child> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt = 0;
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempt + 1 < MAX_ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt as u64));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Wait for `child` up to `timeout`, tearing down its detached process group on every exit path.
/// Success, non-zero exit, timeout, and wait failure all reap, so a child that spawned grandchildren can't orphan them.
fn wait_and_reap(child: &mut Child, timeout: Duration) -> Result<(), SubprocessError> {
    match child.wait_timeout(timeout) {
        // `wait_timeout` already reaped the direct child on these branches.
        Ok(Some(status)) if status.success() => {
            reap_process_group(child);
            Ok(())
        }
        Ok(Some(status)) => {
            reap_process_group(child);
            Err(SubprocessError::NonZeroExit(status))
        }
        Ok(None) => {
            reap(child);
            Err(SubprocessError::Timeout)
        }
        // waitpid failed; the child may still be running, so reap it too, the same teardown as the timeout branch (don't leak the child tree)
        Err(e) => {
            reap(child);
            Err(SubprocessError::Wait(e))
        }
    }
}

/// Best-effort teardown of a spawned child: SIGKILL its process group (to reach any grandchildren), then kill and reap the child.
fn reap(child: &mut Child) {
    reap_process_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

/// SIGKILL the child's process group so grandchildren are reaped, not the
/// direct child.
#[cfg(unix)]
fn reap_process_group(child: &Child) {
    let pid = child.id() as libc::pid_t;
    // SAFETY: killpg with a valid pid and a standard signal has no memory effects
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn reap_process_group(_child: &Child) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::Instant;

    fn detached(mut cmd: Command) -> Command {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        xai_tty_utils::detach_std_command(&mut cmd);
        cmd
    }

    #[cfg(unix)]
    #[test]
    fn zero_exit_is_ok() {
        let cmd = detached(Command::new("true"));
        assert!(run_with_timeout(cmd, None, Duration::from_secs(5)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_reported() {
        let cmd = detached(Command::new("false"));
        let r = run_with_timeout(cmd, None, Duration::from_secs(5));
        assert!(
            matches!(r, Err(SubprocessError::NonZeroExit(_))),
            "got {r:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn slow_command_times_out_quickly() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let cmd = detached(cmd);
        let start = Instant::now();
        let r = run_with_timeout(cmd, None, Duration::from_millis(150));
        assert!(matches!(r, Err(SubprocessError::Timeout)));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "should return at the deadline, not wait the full 5s",
        );
    }

    /// A large stdin payload (bigger than any OS pipe buffer) must be delivered through `run_with_timeout`'s own writer without deadlocking the wait.
    /// That is the whole reason the writer is a scoped thread.
    /// We point `cat`'s stdout at a file to prove every byte was consumed, and assert the call returns `Ok(())` promptly rather than at the timeout.
    #[cfg(unix)]
    #[test]
    fn large_stdin_payload_is_delivered_without_deadlock() {
        let payload = vec![b'x'; 256 * 1024];
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = dir.path().join("drained");
        let sink_file = std::fs::File::create(&sink).expect("create sink");

        let mut cmd = Command::new("cat");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::from(sink_file))
            .stderr(Stdio::null());
        xai_tty_utils::detach_std_command(&mut cmd);

        let start = Instant::now();
        let r = run_with_timeout(cmd, Some(&payload), Duration::from_secs(10));
        assert!(
            r.is_ok(),
            "draining a large stdin payload must succeed: {r:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must return after the drain, not after the full timeout",
        );
        // `cat` copies all of stdin to the file, proving the whole payload was both delivered and consumed via the real scoped writer
        let drained = std::fs::metadata(&sink).expect("sink metadata").len();
        assert_eq!(
            drained,
            payload.len() as u64,
            "all stdin bytes round-tripped through cat"
        );
    }

    /// `reap` terminates the spawned process group.
    #[cfg(unix)]
    #[test]
    fn reap_terminates_the_process() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let mut cmd = detached(cmd);
        #[allow(clippy::disallowed_methods)] // test fixture; the test kills it
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = child.id() as libc::pid_t;

        reap(&mut child);

        // After SIGKILL and wait, the pid no longer names a live process
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH),
            "process {pid} should be gone after reap",
        );
    }

    #[test]
    fn missing_binary_is_spawn_error() {
        let cmd = Command::new("definitely-not-a-real-binary-9f8a7b6c5d4e");
        let r = run_with_timeout(cmd, None, Duration::from_secs(5));
        assert!(matches!(r, Err(SubprocessError::Spawn(_))), "got {r:?}");
    }

    /// A caller can pass a payload but forget `cmd.stdin(Stdio::piped())`;
    /// the `debug_assert!` turns that silent drop into a hard failure.
    #[cfg(all(unix, debug_assertions))]
    #[test]
    #[should_panic(expected = "stdin_payload supplied but cmd.stdin is not piped")]
    fn stdin_payload_without_piped_stdin_is_flagged() {
        let cmd = detached(Command::new("true"));
        let _ = run_with_timeout(cmd, Some(b"payload"), Duration::from_secs(5));
    }
}
