//! Unsandboxed `gh` host worker for `--sandbox` (pathbox) sessions.
//!
//! A jail re-execs the whole binary, so a `gh` spawned from the jailed process
//! reaches neither the host credentials nor the network. The worker is started
//! on the host moments before the re-exec and handed in as an open socketpair
//! FD that survives `exec`. It runs `gh` and nothing else, and only the
//! read-only commands in [`ALLOWED_COMMANDS`].
//!
//! Protocol (one `UnixStream`, newline-delimited, request/response):
//!   request  : `gh-status <HEAD_BRANCH>\n`   — the CI dot's fixed query
//!   request  : `gh <JSON array of argv>\n`   — an allowlisted `gh` run
//!   response : one line, or `.` when the request produced nothing usable.

use std::io::{BufRead, BufReader, Write};
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

/// The max size of a single worker response we accept. A run list is a few KB,
/// but `gh run view --log-failed` is a whole job log, so this is what bounds
/// how much one request may push into the jail.
const MAX_RESPONSE_BYTES: usize = 1 << 20; // 1 MiB

/// The most arguments one `gh` request may carry.
const MAX_REQUEST_ARGS: usize = 32;
/// The longest single argument one `gh` request may carry.
const MAX_ARG_BYTES: usize = 512;

/// One allowlisted `gh` run, as the worker executed it.
///
/// Serialized as a single JSON line, so a log with embedded newlines rides the
/// newline-delimited protocol without escaping games of our own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GhHostResponse {
    /// The process exit code, or -1 when `gh` was killed by a signal.
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Whether [`MAX_RESPONSE_BYTES`] clipped the head off `stdout`.
    pub truncated: bool,
}

impl GhHostResponse {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// The read-only `gh` command pairs the worker will run, as `(command,
/// subcommand)`. Everything else is refused before `gh` is spawned, so no
/// request can rerun a job, cancel a run, merge a pull request, or post.
const ALLOWED_COMMANDS: &[(&str, &str)] = &[
    ("run", "list"),
    ("run", "view"),
    ("pr", "checks"),
    ("pr", "view"),
    ("pr", "list"),
    ("pr", "diff"),
    ("workflow", "list"),
    ("workflow", "view"),
    ("repo", "view"),
    ("auth", "status"),
];

/// `gh api` takes no subcommand, so it is admitted on its own. What keeps it a
/// read is [`ALLOWED_FLAGS`], which refuses every flag that carries a method
/// or a body.
const ALLOWED_BARE_COMMANDS: &[&str] = &["api"];

/// Every flag a request may carry. An allowlist rather than a deny-list
/// because the flags that matter are the ones that turn a read into a write:
/// `-X POST`, `--method`, `--field`, `--input`.
const ALLOWED_FLAGS: &[&str] = &[
    "--json",
    "--jq",
    "-q",
    "--template",
    "-t",
    "--limit",
    "-L",
    "--branch",
    "-b",
    "--workflow",
    "-w",
    "--status",
    "-s",
    "--user",
    "-u",
    "--event",
    "-e",
    "--commit",
    "-c",
    "--created",
    "--log",
    "--log-failed",
    "--job",
    "-j",
    "--attempt",
    "-a",
    "--verbose",
    "-v",
    "--exit-status",
    "--repo",
    "-R",
    "--state",
    "--base",
    "-B",
    "--head",
    "-H",
    "--author",
    "-A",
    "--label",
    "-l",
    "--search",
    "--required",
    "--paginate",
    "--slurp",
    "--cache",
    "--hostname",
    "--yaml",
    "--ref",
    "-r",
];

/// Whether this process is the host CI worker (its `main` should run the
/// worker loop and exit rather than start a session).
pub fn is_ci_host_subprocess() -> bool {
    std::env::var_os(CI_HOST_MARKER_ENV).is_some()
}

/// The inherited host-worker fd, when this process is a sandboxed session that
/// was handed one at jail entry.
pub fn ci_host_fd() -> Option<i32> {
    std::env::var(CI_HOST_FD_ENV).ok()?.parse().ok()
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
    let exe = std::env::current_exe().ok()?;
    let (ours, theirs) = UnixStream::pair().ok()?;
    let our_fd: RawFd = ours.as_raw_fd();
    // The pair is created close-on-exec, and the jail is entered by exec. Without this the fd is gone before the jailed pager reads the env var that names it.
    inherit_across_exec(our_fd)?;
    let theirs_fd: RawFd = theirs.into_raw_fd();

    let mut cmd = std::process::Command::new(exe);
    cmd.env(CI_HOST_MARKER_ENV, "1");
    // `gh` discovers the owning repo from the git remote at its cwd.
    cmd.current_dir(repo_root);
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

/// Clear `FD_CLOEXEC` on `fd` so it stays open in the process this one execs into.
#[cfg(unix)]
fn inherit_across_exec(fd: std::os::unix::io::RawFd) -> Option<()> {
    // SAFETY: fcntl on an fd this process owns; F_GETFD and F_SETFD only touch its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return None;
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    (rc >= 0).then_some(())
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
    serve(std::io::stdin(), std::io::stdout());
}

/// Serve the worker protocol on one socket instead of stdin/stdout.
///
/// Production hands the worker its socket as fd 0 and fd 1, so it reads and
/// writes the standard streams. A caller whose stdout carries something else
/// (a test harness prints its own progress there, and every such line reaches
/// the client as a fake answer) passes the socket here instead.
#[cfg(unix)]
pub fn run_ci_host_worker_on(stream: UnixStream) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    serve(stream, write_half);
}

fn serve<R: std::io::Read, W: Write>(input: R, output: W) {
    let mut reader = BufReader::new(input);
    let mut out = output;
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
        handle_request(line, &mut out);
        let _ = out.flush();
    }
}

/// Serve one request line, writing the single-line answer to `out`. Pure and
/// testable: drives the fixed `gh` query for `gh-status <branch>` and answers
/// the `.` nothing-usable sentinel for anything else (confinement).
pub fn handle_request<W: Write>(line: &str, out: &mut W) {
    if let Some(branch) = line.strip_prefix("gh-status ") {
        // A lone `.` is the "nothing usable" sentinel (see module docs).
        write_one_line(out, query_branch(branch).unwrap_or_else(|| vec![b'.']));
        return;
    }
    if let Some(json) = line.strip_prefix("gh ") {
        let payload = run_allowlisted(json)
            .and_then(|response| serde_json::to_vec(&response).ok())
            .unwrap_or_else(|| vec![b'.']);
        write_one_line(out, payload);
        return;
    }
    // Unknown request: answer nothing usable so the jailed side
    // degrades to "off" rather than hanging or trusting us.
    write_one_line(out, vec![b'.']);
}

/// Write one response as exactly one line.
///
/// The trim is what holds the framing. `gh run list --json` ends its stdout
/// with a newline of its own, so appending one wrote a blank line after every
/// answer. The caller then read that blank line as the NEXT answer, and every
/// response after the first arrived one request behind.
fn write_one_line<W: Write>(out: &mut W, mut payload: Vec<u8>) {
    while matches!(payload.last(), Some(b'\n' | b'\r')) {
        payload.pop();
    }
    if payload.is_empty() {
        payload.push(b'.');
    }
    let _ = out.write_all(&payload);
    let _ = out.write_all(b"\n");
}

/// Parse a `gh <json argv>` request, check it against the allowlist, and run
/// it. `None` for anything the allowlist refuses, and for a `gh` that could
/// not start at all.
fn run_allowlisted(json: &str) -> Option<GhHostResponse> {
    let args: Vec<String> = serde_json::from_str(json).ok()?;
    if !gh_args_allowed(&args) {
        return None;
    }
    let output = spawn_gh(args.iter().map(String::as_str))?;
    Some(response_from_output(output))
}

/// Reduce a finished `gh` run to the wire response, capping both streams.
fn response_from_output(output: std::process::Output) -> GhHostResponse {
    GhHostResponse {
        code: output.status.code().unwrap_or(-1),
        truncated: output.stdout.len() > MAX_RESPONSE_BYTES,
        stdout: tail_lossy(&output.stdout, MAX_RESPONSE_BYTES),
        stderr: tail_lossy(&output.stderr, 8192),
    }
}

/// The last `max` bytes of `bytes`, decoded lossily. An oversized body keeps
/// its tail: a failing job log ends where the error is, and a head-clipped log
/// reports the setup steps instead.
fn tail_lossy(bytes: &[u8], max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Whether an argv is a read-only `gh` invocation this worker will run.
///
/// Three gates, all of which must hold: the shape is bounded, every flag is in
/// [`ALLOWED_FLAGS`], and the leading command is in [`ALLOWED_COMMANDS`] or
/// [`ALLOWED_BARE_COMMANDS`].
pub fn gh_args_allowed(args: &[String]) -> bool {
    if args.is_empty() || args.len() > MAX_REQUEST_ARGS {
        return false;
    }
    if !args.iter().all(|arg| valid_arg(arg)) {
        return false;
    }
    if !args
        .iter()
        .filter(|arg| arg.starts_with('-'))
        .all(|flag| flag_allowed(flag))
    {
        return false;
    }
    let positional: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let Some(&command) = positional.first() else {
        return false;
    };
    if ALLOWED_BARE_COMMANDS.contains(&command) {
        return true;
    }
    let Some(&subcommand) = positional.get(1) else {
        return false;
    };
    ALLOWED_COMMANDS
        .iter()
        .any(|(cmd, sub)| *cmd == command && *sub == subcommand)
}

/// A flag is judged on its name, so `--json=a,b` is judged on `--json`.
fn flag_allowed(flag: &str) -> bool {
    let name = flag.split_once('=').map_or(flag, |(name, _)| name);
    ALLOWED_FLAGS.contains(&name)
}

/// A request argument must be bounded and printable: a newline would break the
/// protocol's own framing.
fn valid_arg(arg: &str) -> bool {
    !arg.is_empty()
        && arg.len() <= MAX_ARG_BYTES
        && arg.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
}

/// Run `gh run list --json` for `branch` in the worker's cwd and return the
/// raw stdout bytes, or `None` when `gh` failed / was unavailable. The branch
/// token is validated so only a plausible branch name is ever interpolated
/// into argv.
fn query_branch(branch: &str) -> Option<Vec<u8>> {
    if !valid_branch_token(branch) {
        return None;
    }
    let output = spawn_gh([
        "run",
        "list",
        "--branch",
        branch,
        "--limit",
        "10",
        "--json",
        "status,conclusion,headBranch,workflowName",
    ])?;
    if !output.status.success() {
        return None;
    }
    let stdout = output.stdout;
    (stdout.len() <= MAX_RESPONSE_BYTES && !stdout.is_empty()).then_some(stdout)
}

/// Spawn `gh` with colour forced off, capturing both streams. `None` only when
/// the process could not be started at all (no `gh` on the host).
fn spawn_gh<'a, I>(args: I) -> Option<std::process::Output>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut cmd = std::process::Command::new("gh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // `gh` colourises even piped `--json` output under CLICOLOR_FORCE or
    // GH_FORCE_TTY, and forcing beats NO_COLOR in gh's precedence.
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    cmd.output().ok()
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

/// The inherited host-worker stream, opened once per fd and shared by every
/// caller on it.
///
/// One connection, one mutex: the CI dot polls off a blocking thread while the
/// agent's `ci` tool runs its own queries, and this protocol is one request
/// then one response. Two callers writing at once interleave two requests into
/// one socket and read each other's answers. Keyed by fd so a distinct
/// connection (a session restart, or a test peer) gets its own lock.
/// A reader is held with the connection rather than built per call. A
/// `BufReader` reads ahead, so one built per call takes whatever followed the
/// newline into a buffer it then drops, and the next call reads a truncated
/// answer.
#[cfg(unix)]
type HostStream = std::sync::Arc<std::sync::Mutex<BufReader<UnixStream>>>;

#[cfg(unix)]
static HOST_STREAMS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<i32, HostStream>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(unix)]
fn host_stream(fd: i32) -> Option<HostStream> {
    use std::os::unix::io::FromRawFd as _;
    use std::sync::{Arc, Mutex};
    let mut map = HOST_STREAMS.lock().ok()?;
    // SAFETY: `fd` names a real socket opened by `spawn_ci_host` on the host
    // and inherited into this (jailed) process; we take ownership of that fd
    // exactly once, here, and keep the stream alive for the whole session.
    let stream = map.entry(fd).or_insert_with(|| {
        Arc::new(Mutex::new(BufReader::new(unsafe {
            UnixStream::from_raw_fd(fd)
        })))
    });
    Some(Arc::clone(stream))
}

/// Send one request line over the host connection and read its single-line
/// answer. `None` on any transport failure and on the `.` sentinel, so every
/// caller degrades rather than trusting a half-read answer.
#[cfg(unix)]
fn exchange(fd: i32, request: &str) -> Option<Vec<u8>> {
    let stream = host_stream(fd)?;
    let mut guard = stream.lock().ok()?;
    let mut line = String::with_capacity(request.len() + 1);
    line.push_str(request);
    line.push('\n');
    guard.get_mut().write_all(line.as_bytes()).ok()?;
    guard.get_mut().flush().ok()?;

    // One worker response is one line. A blank line is skipped rather than
    // read as an answer: reading one would put every later answer a request
    // behind, which is worse than the stray line it came from.
    let mut response = Vec::new();
    loop {
        response.clear();
        match guard.read_until(b'\n', &mut response) {
            // 0 is a true EOF: the worker exited without answering.
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        while matches!(response.last(), Some(b'\n' | b'\r')) {
            response.pop();
        }
        if !response.is_empty() {
            break;
        }
    }
    (response != b".").then_some(response)
}

/// Drop this process's handle on a host connection, closing our end of the
/// socket. The worker's read loop then hits EOF and exits. A session holds its
/// one connection for its whole life, so this is for a caller that owns the
/// worker's lifetime, such as a test.
#[cfg(unix)]
pub fn close_host_connection(fd: i32) {
    if let Ok(mut map) = HOST_STREAMS.lock() {
        map.remove(&fd);
    }
}

/// Jailed-side read: query the host worker over the inherited connection for
/// `branch`. Returns the raw single-line JSON array, or `None` so the caller
/// degrades to the "off" state (worker missing, failed, or unusable output).
#[cfg(unix)]
pub fn query_ci_host(fd: i32, branch: &str) -> Option<Vec<u8>> {
    exchange(fd, &format!("gh-status {branch}"))
}

/// Jailed-side read: run an allowlisted `gh` command on the host worker.
#[cfg(unix)]
pub fn query_gh_host(fd: i32, args: &[&str]) -> Option<GhHostResponse> {
    let json = serde_json::to_string(args).ok()?;
    let response = exchange(fd, &format!("gh {json}"))?;
    serde_json::from_slice(&response).ok()
}

/// Run a read-only `gh` command wherever this process can actually reach `gh`:
/// through the host worker when sandboxed, by spawning it directly otherwise.
///
/// The host worker is authoritative once it exists. A sandboxed session never
/// falls back to an in-jail spawn, where `gh` reaches neither the host
/// credentials nor the network and would answer with a misleading failure.
pub fn run_gh(cwd: &Path, args: &[&str]) -> Option<GhHostResponse> {
    #[cfg(unix)]
    if let Some(fd) = ci_host_fd() {
        return query_gh_host(fd, args);
    }
    let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    if !gh_args_allowed(&owned) {
        return None;
    }
    let mut cmd = std::process::Command::new("gh");
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    Some(response_from_output(cmd.output().ok()?))
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

    /// The jailed pager is a process this one execs into, so the worker fd must survive an exec.
    #[test]
    fn worker_fd_survives_an_exec() {
        use std::io::Read as _;
        use std::os::unix::io::AsRawFd as _;
        let (mut ours, theirs) = UnixStream::pair().expect("socketpair");
        let fd = theirs.as_raw_fd();
        // `sh` execs into a fresh image and writes to the numbered fd it inherited.
        let write_to_fd = |fd: i32| {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf alive >&{fd}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("spawn sh")
        };
        assert!(!write_to_fd(fd).success(), "the fd must be closed on exec before the fix");

        inherit_across_exec(fd).expect("fcntl");
        assert!(write_to_fd(fd).success());
        drop(theirs);
        let mut got = String::new();
        ours.read_to_string(&mut got).expect("read");
        assert_eq!(got, "alive");
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
            "gh-status ".to_string(),
            format!("gh-status {}", "a".repeat(300)),
            "gh-status evil\nbranch".to_string(),
            "gh-status has space".to_string(),
        ] {
            assert_eq!(answer(&line), b".\n", "token {line:?} must be refused");
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

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn allowlist_admits_the_read_only_pipeline_commands() {
        // Exactly the shapes the commit, push, watch, read-logs loop needs.
        for command in [
            vec!["run", "list", "--branch", "feat/x", "--limit", "10"],
            vec!["run", "list", "--json", "status,conclusion,databaseId"],
            vec!["run", "view", "12345", "--log-failed"],
            vec!["run", "view", "12345", "--json", "jobs"],
            vec!["pr", "checks", "--required"],
            vec!["pr", "view", "42", "--json", "state"],
            vec!["workflow", "list"],
            vec!["api", "repos/o/r/actions/runs", "--jq", ".workflow_runs"],
            vec!["auth", "status"],
        ] {
            assert!(
                gh_args_allowed(&args(&command)),
                "{command:?} must be admitted"
            );
        }
    }

    #[test]
    fn allowlist_refuses_every_write() {
        // The worker is the one thing between a jailed session and a host `gh`
        // that can cancel a run or merge a pull request.
        for command in [
            vec!["run", "rerun", "12345"],
            vec!["run", "cancel", "12345"],
            vec!["run", "delete", "12345"],
            vec!["pr", "merge", "42"],
            vec!["pr", "create", "--title", "x"],
            vec!["pr", "comment", "42", "--body", "x"],
            vec!["pr", "close", "42"],
            vec!["issue", "create"],
            vec!["release", "upload"],
            vec!["repo", "delete"],
            vec!["workflow", "run", "ci.yml"],
            vec!["auth", "token"],
            vec!["secret", "set", "X"],
        ] {
            assert!(
                !gh_args_allowed(&args(&command)),
                "{command:?} must be refused"
            );
        }
    }

    #[test]
    fn allowlist_keeps_gh_api_a_read() {
        // `gh api` defaults to GET. The flags that change that are the reason
        // the flag check is an allowlist and not a deny-list.
        assert!(gh_args_allowed(&args(&["api", "repos/o/r"])));
        for command in [
            vec!["api", "repos/o/r", "-X", "DELETE"],
            vec!["api", "repos/o/r", "--method", "POST"],
            vec!["api", "repos/o/r", "-f", "name=x"],
            vec!["api", "repos/o/r", "--field", "name=x"],
            vec!["api", "repos/o/r", "--raw-field", "name=x"],
            vec!["api", "repos/o/r", "--input", "-"],
            vec!["run", "list", "--web"],
        ] {
            assert!(
                !gh_args_allowed(&args(&command)),
                "{command:?} must be refused"
            );
        }
        // A flag written as `--flag=value` is judged on its name.
        assert!(gh_args_allowed(&args(&["run", "list", "--limit=5"])));
        assert!(!gh_args_allowed(&args(&["api", "x", "--method=POST"])));
    }

    #[test]
    fn allowlist_bounds_the_request_shape() {
        assert!(!gh_args_allowed(&[]));
        let mut many = args(&["run", "list"]);
        many.extend(std::iter::repeat_n("x".to_string(), MAX_REQUEST_ARGS));
        assert!(!gh_args_allowed(&many), "an unbounded argv must be refused");
        assert!(!gh_args_allowed(&args(&["run", "list", "--branch", ""])));
        let long = "a".repeat(MAX_ARG_BYTES + 1);
        assert!(!gh_args_allowed(&args(&["run", "list", "--branch", &long])));
        // A newline would break the protocol's own framing.
        assert!(!gh_args_allowed(&args(&[
            "run", "list", "--branch", "a\nb"
        ])));
    }

    #[test]
    fn worker_refuses_a_gh_request_the_allowlist_rejects() {
        // The refusal happens on the worker, so a jailed session that wrote
        // the request line by hand gets the same answer.
        assert_eq!(answer(r#"gh ["run","cancel","1"]"#), b".\n");
        assert_eq!(answer(r#"gh ["pr","merge","42"]"#), b".\n");
        // Malformed JSON is a refusal too, never a panic.
        assert_eq!(answer("gh not-json"), b".\n");
        assert_eq!(answer(r#"gh {"run":"list"}"#), b".\n");
    }

    #[test]
    fn a_payload_that_already_ends_in_a_newline_still_writes_one_line() {
        // `gh run list --json` ends its stdout with a newline. Appending a
        // second one wrote a blank line after the answer, and the caller read
        // that blank line as the NEXT answer — so every response after the
        // first arrived one request behind.
        let mut sink = Sink(Vec::new());
        write_one_line(&mut sink, b"[{\"status\":\"completed\"}]\n".to_vec());
        assert_eq!(sink.0, b"[{\"status\":\"completed\"}]\n");
        // A payload that trims away to nothing is the sentinel, never a blank
        // line that would desynchronise the stream the same way.
        let mut blank = Sink(Vec::new());
        write_one_line(&mut blank, b"\n\n".to_vec());
        assert_eq!(blank.0, b".\n");
    }

    #[test]
    fn a_stray_blank_line_does_not_shift_later_answers() {
        // The reader skips a blank line instead of reporting it as an answer,
        // so one stray newline on the wire cannot put every later caller a
        // request behind.
        // The blank line rides the same answer, because a worker only writes
        // after a request. That is how the real stray one reached the wire.
        let fd = peer(vec![
            "\n{\"code\":0,\"stdout\":\"first\",\"stderr\":\"\",\"truncated\":false}",
            r#"{"code":0,"stdout":"second","stderr":"","truncated":false}"#,
        ]);
        let first = query_gh_host(fd, &["run", "list"]).expect("first");
        assert_eq!(first.stdout, "first");
        let second = query_gh_host(fd, &["run", "list"]).expect("second");
        assert_eq!(second.stdout, "second");
    }

    #[test]
    fn tail_lossy_keeps_the_end_of_an_oversized_body() {
        assert_eq!(
            tail_lossy(b"noise error: the real failure", 23),
            "error: the real failure"
        );
        assert_eq!(tail_lossy(b"short", 100), "short");
    }

    /// Drive the jailed-side transport against an in-process peer speaking the
    /// real worker protocol: one request line in, one answer line out.
    ///
    /// The socket is leaked so its fd stays open and unique for this process:
    /// the transport's map keys on the fd NUMBER, and a reused number would
    /// serve a later test the stale stream. Production never reuses a
    /// session's single worker fd.
    fn peer(answers: Vec<&'static str>) -> i32 {
        use std::io::Read as _;
        use std::os::unix::io::AsRawFd;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let fd = ours.as_raw_fd();
        std::thread::spawn(move || {
            let mut stream = theirs;
            for answer in answers {
                let mut buf = [0u8; 8192];
                if stream.read(&mut buf).unwrap_or(0) == 0 {
                    return;
                }
                let _ = stream.write_all(answer.as_bytes());
                let _ = stream.write_all(b"\n");
                let _ = stream.flush();
            }
        });
        std::mem::forget(ours);
        fd
    }

    #[test]
    fn query_ci_host_roundtrips_a_result() {
        let fd = peer(vec![r#"[{"status":"completed","conclusion":"success"}]"#]);
        let got = query_ci_host(fd, "master").expect("read");
        let text = String::from_utf8(got).expect("utf8");
        assert!(text.contains("\"success\""));
    }

    #[test]
    fn query_ci_host_dot_sentinel_is_off() {
        let fd = peer(vec!["."]);
        assert_eq!(query_ci_host(fd, "master"), None);
    }

    #[test]
    fn query_ci_host_eof_is_off() {
        use std::os::unix::io::AsRawFd;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let fd = ours.as_raw_fd();
        drop(theirs);
        std::mem::forget(ours);
        assert_eq!(query_ci_host(fd, "master"), None);
    }

    #[test]
    fn query_gh_host_roundtrips_a_response() {
        let fd = peer(vec![
            r#"{"code":1,"stdout":"error: test failed","stderr":"","truncated":false}"#,
        ]);
        let got = query_gh_host(fd, &["run", "view", "1", "--log-failed"]).expect("response");
        assert_eq!(got.code, 1);
        assert!(!got.success());
        assert_eq!(got.stdout, "error: test failed");
    }

    #[test]
    fn query_gh_host_sentinel_is_none() {
        let fd = peer(vec!["."]);
        assert_eq!(query_gh_host(fd, &["run", "list"]), None);
    }

    #[test]
    fn one_connection_serves_repeated_queries_in_order() {
        let fd = peer(vec![
            r#"{"code":0,"stdout":"first","stderr":"","truncated":false}"#,
            r#"{"code":0,"stdout":"second","stderr":"","truncated":false}"#,
            r#"{"code":0,"stdout":"third","stderr":"","truncated":false}"#,
        ]);
        for expected in ["first", "second", "third"] {
            let got = query_gh_host(fd, &["run", "list"]).expect("response");
            assert_eq!(got.stdout, expected);
        }
    }

    #[test]
    fn concurrent_callers_never_read_each_others_answers() {
        // The dot polls off a blocking thread while the tool runs its own
        // queries. Without the per-connection lock the two requests interleave
        // into one socket and each reads the other's answer.
        use std::os::unix::io::AsRawFd;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let fd = ours.as_raw_fd();
        std::thread::spawn(move || {
            let mut stream = theirs;
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                // Echo the request back as the body, so an answer that reached
                // the wrong caller is detectable by the one that asked.
                let body = serde_json::json!({
                    "code": 0,
                    "stdout": line.trim(),
                    "stderr": "",
                    "truncated": false,
                });
                let _ = stream.write_all(body.to_string().as_bytes());
                let _ = stream.write_all(b"\n");
                let _ = stream.flush();
            }
        });
        std::mem::forget(ours);

        let threads: Vec<_> = (0..8)
            .map(|index| {
                std::thread::spawn(move || {
                    let branch = format!("branch-{index}");
                    for _ in 0..10 {
                        let response = query_gh_host(fd, &["run", "list", "--branch", &branch])
                            .expect("response");
                        assert!(
                            response.stdout.contains(&branch),
                            "caller {index} read another caller's answer: {}",
                            response.stdout
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("thread");
        }
    }
}
