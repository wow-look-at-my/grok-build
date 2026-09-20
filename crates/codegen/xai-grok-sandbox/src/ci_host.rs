//! Unsandboxed `gh` host worker for `--sandbox` (pathbox) sessions.
//!
//! A jail re-execs the whole binary, so a `gh` spawned from the jailed process
//! reaches neither the host credentials nor the network. The worker is started
//! on the host moments before the re-exec and handed in as an open socketpair
//! FD that survives `exec`. It runs `gh` and nothing else, and only the
//! read-only commands in [`ALLOWED_COMMANDS`].
//!
//! Protocol (one `UnixStream`, newline-delimited, request/response):
//!   request  : `gh-status <HEAD_BRANCH>\n`   - the CI dot's fixed query: the
//!              raw `gh run list --json` array
//!   request  : `gh-pr <BRANCH>\n`            - the shell's `x.ai/pr/status`
//!              fixed query: one JSON object carrying the branch's pull
//!              request (`gh pr view`) and its check runs (`gh pr checks`)
//!   request  : `gh <JSON array of argv>\n`   - an allowlisted `gh` run
//!   response : one line, or `.` when the request produced nothing usable
//!              (unknown shape, bad token, `gh` failed, no such PR, or an
//!              oversized reply).

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
    resolve_fd(
        SESSION_CI_HOST_FD.get().copied(),
        std::env::var(CI_HOST_FD_ENV).ok().as_deref(),
    )
}

/// Which fd a session's CI queries ride: the one this process started and
/// published, else the one a jail handed in by name.
///
/// The published fd wins because a process that started its own worker is not
/// going to be handed another, and a stale `CI_HOST_FD_ENV` (inherited by a
/// child that did not get the fd itself) must never override the live one.
fn resolve_fd(published: Option<i32>, from_env: Option<&str>) -> Option<i32> {
    published.or_else(|| from_env.and_then(|raw| raw.parse().ok()))
}

/// The worker fd a session started in THIS process, as
/// [`start_ci_host_for_session`] publishes it.
///
/// The profile sandbox confines the session in place rather than re-execing it,
/// so nothing carries the fd number across except this. It is deliberately not
/// an env var: an `exec` that does not carry the fd would leave the number
/// naming whatever descriptor the child opened next, and every child of the
/// session would inherit the connection.
static SESSION_CI_HOST_FD: std::sync::OnceLock<i32> = std::sync::OnceLock::new();

/// Start the host worker for a session that is confined in place, before the
/// confinement is installed, and publish its fd to this process.
///
/// A confining profile sandbox (`--sandbox=workspace`, `read-only`, `strict`, a
/// custom profile) is applied to the running session by `sandbox_init`. The
/// macOS profile it installs denies the keychain mach services, so a `gh` that
/// runs under it finds its account but no token and answers `401`; the same
/// `gh` outside the confinement works. Forking the worker first is what puts an
/// unconfined `gh` behind the session's queries, which is what the pathbox jail
/// does with [`spawn_ci_host`].
///
/// `survives_exec` says whether an `exec` follows the hand-off (the Linux
/// bwrap re-exec for a deny-carrying profile). Where one does, the fd is made
/// exec-surviving and its NUMBER is exported as [`CI_HOST_FD_ENV`] for the
/// re-executed image. Where none does (macOS, and Linux profiles that need no
/// bwrap), the fd stays close-on-exec and the number moves through
/// [`SESSION_CI_HOST_FD`] alone, so no child of the session inherits it.
///
/// Returns the fd, or `None` when a worker is not this process's to start: this
/// process IS the worker, one is already published, or the process is already
/// inside a jail that started its own.
pub fn start_ci_host_for_session(repo_root: &Path, survives_exec: bool) -> Option<i32> {
    if is_ci_host_subprocess() || ci_host_fd().is_some() || crate::is_jailed() {
        return None;
    }
    let fd = spawn_ci_host_with(repo_root, survives_exec)?;
    if survives_exec {
        // The image that reads this is the session the exec replaces us with,
        // and it is the fd's owner from then on.
        // SAFETY: this runs on the startup path, before the session exists.
        unsafe { std::env::set_var(CI_HOST_FD_ENV, fd.to_string()) };
    }
    let _ = SESSION_CI_HOST_FD.set(fd);
    Some(fd)
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
    spawn_ci_host_with(repo_root, true)
}

/// [`spawn_ci_host`] with the exec rule made explicit: `survives_exec` clears
/// `FD_CLOEXEC` on the fd this process keeps, and passes the fd number on to the
/// image it execs into.
fn spawn_ci_host_with(repo_root: &Path, survives_exec: bool) -> Option<i32> {
    if is_ci_host_subprocess() {
        return None;
    }
    use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    let exe = std::env::current_exe().ok()?;
    let (ours, theirs) = UnixStream::pair().ok()?;
    let our_fd: RawFd = ours.as_raw_fd();
    // The pair is created close-on-exec. A jail is entered by exec, so without this the fd is gone before the jailed pager reads the env var that names it.
    if survives_exec {
        inherit_across_exec(our_fd)?;
    }
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
///
/// Public because it is half of the jail boundary's contract and the other half
/// is a `Command` the tests drive: a socketpair is created close-on-exec, the
/// jail is entered by exec, and a worker connection that does not survive that
/// exec leaves the jailed pager reading a dead fd off `GROK_CI_HOST_FD`.
#[cfg(unix)]
pub fn inherit_across_exec(fd: std::os::unix::io::RawFd) -> Option<()> {
    // SAFETY: fcntl on an fd this process owns; F_GETFD and F_SETFD only touch its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return None;
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    (rc >= 0).then_some(())
}

/// Take the worker back from an exec that did not happen.
///
/// The hand-off clears `FD_CLOEXEC` and names the fd in the environment for the
/// image an exec is about to produce. Where that exec fails and this process
/// carries on instead, both have to be undone: the session is confined in place
/// after all, and every child it spawns would otherwise inherit a live socket to
/// an UNCONFINED `gh`, with the environment naming the number to read it on.
///
/// The session itself keeps the worker. It finds the fd through
/// [`SESSION_CI_HOST_FD`], which no child of it can read.
#[cfg(unix)]
pub fn reclaim_from_failed_exec(fd: std::os::unix::io::RawFd) {
    // SAFETY: fcntl on an fd this process owns; F_GETFD and F_SETFD only touch its flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags >= 0 {
        // SAFETY: see above.
        unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
    // SAFETY: this runs on the startup path, before the session exists.
    unsafe { std::env::remove_var(CI_HOST_FD_ENV) };
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
/// testable: drives the fixed `gh` queries (`gh-status <branch>` for the CI
/// dot, `gh-pr <branch>` for the shell's `x.ai/pr/status`), runs an
/// allowlisted `gh <json argv>`, and answers the `.` nothing-usable sentinel
/// for anything else (confinement).
pub fn handle_request<W: Write>(line: &str, out: &mut W) {
    if let Some(branch) = line.strip_prefix("gh-status ") {
        // A lone `.` is the "nothing usable" sentinel (see module docs).
        write_one_line(out, query_branch(branch).unwrap_or_else(|| vec![b'.']));
        return;
    }
    if let Some(branch) = line.strip_prefix("gh-pr ") {
        write_one_line(out, query_pr_checks(branch).unwrap_or_else(|| vec![b'.']));
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
    let stdout = run_gh_safely(
        &[
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "10",
            "--json",
            "status,conclusion,headBranch,workflowName",
        ],
        &[0],
    )?;
    bounded_json(stdout)
}

/// Run the fixed PR/checks query for `branch`: the branch's pull request
/// (`gh pr view`) and its check runs (`gh pr checks`), combined into a single
/// JSON object on one line:
///   `{"state":"OPEN","merged":false,"isDraft":false,"url":..,"number":..,"title":..,"checks":[...]}`
///
/// Returns `None` when `gh` is unavailable, the branch has no pull request
/// (`gh pr view` exits non-zero), or the combined document overflows the cap.
/// The caller maps `None` to the `.` nothing-usable sentinel, and the jailed
/// side never falls through to an in-jail `gh`.
fn query_pr_checks(branch: &str) -> Option<Vec<u8>> {
    if !valid_branch_token(branch) {
        return None;
    }
    let pr_view = run_gh_safely(
        &[
            "pr",
            "view",
            branch,
            "--json",
            "state,merged,isDraft,url,number,title",
        ],
        &[0],
    )?;
    let pr: serde_json::Value = serde_json::from_slice(&pr_view).ok()?;
    let state = pr.get("state").and_then(serde_json::Value::as_str).unwrap_or("");
    let merged = pr
        .get("merged")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let is_draft = pr
        .get("isDraft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    // `gh pr checks` reports its verdict in the exit code (1: a check failed,
    // 8: a check is pending) and still prints the full list. Only a `gh` that
    // printed no array is an empty list.
    let checks = run_gh_safely(
        &["pr", "checks", branch, "--json", "name,state,conclusion"],
        &[0, 1, 8],
    )
    .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
    .filter(serde_json::Value::is_array)
    .unwrap_or_else(|| serde_json::Value::Array(vec![]));

    let doc = serde_json::json!({
        "state": state,
        "merged": merged,
        "isDraft": is_draft,
        "url": pr.get("url").cloned().unwrap_or(serde_json::Value::Null),
        "number": pr.get("number").cloned().unwrap_or(serde_json::Value::Null),
        "title": pr.get("title").cloned().unwrap_or(serde_json::Value::Null),
        "checks": checks,
    });
    let bytes = serde_json::to_vec(&doc).ok()?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    Some(bytes)
}

/// Run a fixed-shape `gh` argument vector in the worker's cwd and return the
/// raw stdout bytes when the exit code is one of `ok_codes`. Each query shape
/// fixes its argv in this module; only a validated branch token comes from a
/// request.
fn run_gh_safely(args: &[&str], ok_codes: &[i32]) -> Option<Vec<u8>> {
    let output = spawn_gh(args.iter().copied())?;
    let code = output.status.code()?;
    if !ok_codes.contains(&code) {
        return None;
    }
    Some(output.stdout)
}

/// Keep a raw `gh` stdout body only when it is within the response cap and
/// not empty, so a misbehaving `gh` cannot grow the jail's memory without
/// bound or hand back an unusable blank line.
fn bounded_json(stdout: Vec<u8>) -> Option<Vec<u8>> {
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
    query_ci_host_shape(fd, "gh-status", branch)
}

/// Jailed-side read for the shell's `x.ai/pr/status`: ask the host worker for
/// `branch`'s pull-request state and check runs (`gh-pr`). Returns the raw
/// single-line JSON object, or `None` on the nothing-usable sentinel. It rides
/// the same connection and lock as every other query on `fd`.
#[cfg(unix)]
pub fn query_ci_host_pr(fd: i32, branch: &str) -> Option<Vec<u8>> {
    query_ci_host_shape(fd, "gh-pr", branch)
}

/// Send one fixed-shape `<prefix> <branch>` request and read its answer.
#[cfg(unix)]
fn query_ci_host_shape(fd: i32, prefix: &str, branch: &str) -> Option<Vec<u8>> {
    exchange(fd, &format!("{prefix} {branch}"))
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

    /// Set an env var for one test and restore it on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: every test that mutates these vars is serialized on
            // `ci_host_env`, so no other thread is reading the environment.
            unsafe { std::env::set_var(key, val) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `EnvGuard::set`.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// A session finds the worker it published in its own process, and the
    /// fd a jail named is what it finds when nothing was published. The
    /// env-var route is what a re-executed session has and an ordinary
    /// confined session does not, so both have to resolve.
    #[test]
    fn a_session_finds_its_own_worker_fd_before_the_one_a_jail_named() {
        assert_eq!(resolve_fd(Some(7), Some("9")), Some(7));
        assert_eq!(resolve_fd(Some(7), None), Some(7));
        assert_eq!(resolve_fd(None, Some("9")), Some(9));
        assert_eq!(resolve_fd(None, None), None);
        assert_eq!(resolve_fd(None, Some("")), None);
        assert_eq!(
            resolve_fd(None, Some("not-an-fd")),
            None,
            "a name that is not an fd number must resolve to no worker"
        );
    }

    /// The hand-off starts a worker only where one is this process's to start.
    /// Each of these is a process that must NOT fork another: the worker
    /// itself re-entering `main`, a session a jail already handed a worker, and
    /// a process already inside the pathbox jail.
    #[test]
    #[serial_test::serial(ci_host_env)]
    fn the_hand_off_starts_no_worker_where_one_is_not_this_processes_to_start() {
        let _marker = EnvGuard::set(CI_HOST_MARKER_ENV, "1");
        assert_eq!(
            start_ci_host_for_session(Path::new("/tmp"), false),
            None,
            "the worker must not start a worker"
        );
    }

    #[test]
    #[serial_test::serial(ci_host_env)]
    fn the_hand_off_is_skipped_when_a_worker_fd_is_already_present() {
        let _fd = EnvGuard::set(CI_HOST_FD_ENV, "9");
        assert_eq!(
            start_ci_host_for_session(Path::new("/tmp"), false),
            None,
            "a session that already has a worker does not start a second"
        );
    }

    #[test]
    #[serial_test::serial(ci_host_env)]
    fn the_hand_off_is_skipped_inside_the_pathbox_jail() {
        let _jail = EnvGuard::set(crate::jail::JAIL_ENV_VAR, "1");
        assert_eq!(
            start_ci_host_for_session(Path::new("/tmp"), false),
            None,
            "the jail brought its own worker; the jailed process starts none"
        );
    }

    /// An exec that was prepared for and then did not happen must leave nothing
    /// behind. The session is confined in place in that case, and an
    /// inheritable fd whose number is in the environment is a live socket to an
    /// unconfined `gh` for every child the session spawns.
    #[test]
    #[serial_test::serial(ci_host_env)]
    fn a_failed_exec_puts_the_worker_back_out_of_reach_of_children() {
        let (ours, _theirs) = UnixStream::pair().expect("socketpair");
        let fd = ours.as_raw_fd();
        inherit_across_exec(fd).expect("clear close-on-exec");
        let _env = EnvGuard::set(CI_HOST_FD_ENV, &fd.to_string());

        reclaim_from_failed_exec(fd);

        // SAFETY: fcntl F_GETFD on an fd this test owns.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(flags >= 0, "the fd must still be open");
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "the worker fd must not survive an exec once the exec is off"
        );
        assert!(
            std::env::var(CI_HOST_FD_ENV).is_err(),
            "no child may be handed the number that names the worker"
        );
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
    fn worker_answers_dot_for_any_request_that_is_not_a_fixed_shape() {
        // The confined worker must never run an arbitrary command. Any request
        // outside its shapes (`gh-status`, `gh-pr`, `gh <json argv>`) is
        // answered with the nothing-usable sentinel: the jailed side degrades
        // to "off", and nothing is executed.
        for line in [
            "run list --json",
            "--branch master",
            "gh run list",
            "gh-status",
            "gh-pr",
            "rm -rf /",
            "mission run --json",
            "status ",
            "pr view --json state",
            "gh pr view master --json state",
        ] {
            assert_eq!(answer(line), b".\n", "line {line:?} must be refused");
        }
    }

    #[test]
    fn worker_refuses_invalid_branch_tokens() {
        // Ever with the `gh-status`/`gh-pr` prefix, a token that could not be
        // a real branch is rejected before `gh` is ever invoked (returns `.`).
        for line in [
            "gh-status ".to_string(),
            format!("gh-status {}", "a".repeat(300)),
            "gh-status evil\nbranch".to_string(),
            "gh-status has space".to_string(),
            "gh-pr ".to_string(),
            format!("gh-pr {}", "a".repeat(300)),
            "gh-pr evil\nbranch".to_string(),
            "gh-pr has space".to_string(),
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
        peer_expecting("", answers)
    }

    /// [`peer`] that also requires every request line to start with
    /// `request_prefix`. A request of another shape makes the peer hang up
    /// instead of answering, so the caller reads EOF and fails its `expect`.
    fn peer_expecting(request_prefix: &'static str, answers: Vec<&'static str>) -> i32 {
        use std::io::Read as _;
        use std::os::unix::io::AsRawFd;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let fd = ours.as_raw_fd();
        std::thread::spawn(move || {
            let mut stream = theirs;
            for answer in answers {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    return;
                }
                if !buf[..n].starts_with(request_prefix.as_bytes()) {
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
        let fd = peer_expecting(
            "gh-status ",
            vec![r#"[{"status":"completed","conclusion":"success"}]"#],
        );
        let got = query_ci_host(fd, "master").expect("read");
        let text = String::from_utf8(got).expect("utf8");
        assert!(text.contains("\"success\""));
    }

    #[test]
    fn query_ci_host_pr_roundtrips_a_pr_object() {
        // The PR/checks reader uses the same one-line framing as run-list; a
        // real host worker answers the `gh-pr` shape with one JSON line.
        let fd = peer_expecting(
            "gh-pr feature/x\n",
            vec![
                r#"{"state":"OPEN","merged":false,"isDraft":false,"checks":[{"name":"CI","state":"SUCCESS","conclusion":"SUCCESS"}]}"#,
            ],
        );
        let got = query_ci_host_pr(fd, "feature/x").expect("read");
        let text = String::from_utf8(got).expect("utf8");
        assert!(text.contains("\"state\":\"OPEN\""), "got {text}");
        assert!(text.contains("\"SUCCESS\""), "got {text}");
    }

    #[test]
    fn query_ci_host_pr_dot_sentinel_is_none() {
        // A worker with no usable PR/checks answer (no PR for the branch, `gh`
        // missing, etc.) sends `.`; the reader must surface nothing, never a
        // half-parsed value.
        let fd = peer_expecting("gh-pr ", vec!["."]);
        assert_eq!(query_ci_host_pr(fd, "master"), None);
    }

    #[test]
    fn query_ci_host_pr_eof_is_none() {
        use std::os::unix::io::AsRawFd;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let fd = ours.as_raw_fd();
        drop(theirs);
        std::mem::forget(ours);
        assert_eq!(query_ci_host_pr(fd, "master"), None);
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
