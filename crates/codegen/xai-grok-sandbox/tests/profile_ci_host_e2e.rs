//! End-to-end proof that a session confined by the PROFILE sandbox can still
//! answer its CI queries, on macOS, with the real sandbox installed.
//!
//! The profile sandbox is applied to the running process by `sandbox_init`, and
//! the Seatbelt rules it installs deny the keychain mach services. `gh` keeps
//! its OAuth token in the login keychain, so a `gh` running under those rules
//! sends no `Authorization` header and every CI query answers `401`. This test
//! drives the fix: a session starts the unsandboxed worker BEFORE it is
//! confined, and its queries ride that worker instead of an in-jail `gh`.
//!
//! It is run as the shipped arrangement, not a model of it:
//!
//!   * the parent (unconfined at that moment) starts the worker through the
//!     shipped `ci_host::start_ci_host_for_session` and finds it again through
//!     the shipped `ci_host::ci_host_fd`;
//!   * a child re-enters this same binary, applies the real
//!     `ProfileName::Workspace` profile with the shipped `SandboxManager`, and
//!     drives the shipped query path (`ci_host::run_gh`) exactly as the `ci`
//!     tool and the CI dot do;
//!   * the same child, with NO worker handed in, drives the same query and gets
//!     the in-jail `gh` - the behaviour the fix replaces.
//!
//! The two children are told apart by where their `gh` ran: the worker queries
//! from its own working directory (the session workspace), while the unassisted
//! child's own `gh` runs from the directory the query names. So this test
//! discriminates the two paths on a host with no authenticated `gh` too, where
//! both answers are a `401` and the run list is empty either way.
//!
//! `harness = false` (see Cargo.toml): the worker child is this binary
//! re-entered with the marker env var set and nothing else, which is how the
//! shipped spawn starts it, and only a hand-written `main` can dispatch that.

#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

/// The mode a spawned child runs in. Absent means "the parent".
const MODE_ENV: &str = "GROK_PROFILE_CI_HOST_MODE";
/// The session workspace: what the worker runs `gh` in, and what the child
/// confines itself to.
const WORKSPACE_ENV: &str = "GROK_PROFILE_CI_HOST_WORKSPACE";
/// The directory the child names as the query's cwd, instead of the session
/// workspace, so the answer says which side ran `gh`.
const QUERY_CWD_ENV: &str = "GROK_PROFILE_CI_HOST_QUERY_CWD";
/// The branch every query asks about.
const BRANCH_ENV: &str = "GROK_PROFILE_CI_HOST_BRANCH";

const MODE_JAILED: &str = "jailed";
const MODE_JAILED_NO_WORKER: &str = "jailed-no-worker";

/// Every line a child reports so the parent can read it back.
const REPORT: &str = "profile-ci-host: ";

/// The one case this binary runs, under the name a test runner lists it by.
const TEST_NAME: &str = "a_profile_confined_session_answers_its_ci_query_through_the_worker";

fn main() {
    // A worker child re-enters this binary with the marker set and NOTHING
    // else, exactly as the pager's `main` dispatches it.
    if xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        xai_grok_sandbox::ci_host::run_ci_host_worker();
        std::process::exit(0);
    }
    if serve_list_protocol(TEST_NAME) {
        return;
    }
    #[cfg(target_os = "macos")]
    match std::env::var(MODE_ENV).as_deref() {
        Ok(MODE_JAILED) => jailed_child(true),
        Ok(MODE_JAILED_NO_WORKER) => jailed_child(false),
        _ => parent(),
    }
    #[cfg(not(target_os = "macos"))]
    println!("{REPORT}skip: the profile sandbox is a macOS Seatbelt profile");
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

/// Hex, because a report line carries one value on one line and `gh`'s own
/// error text is several. Splitting the child's stdout into lines kept the
/// first line of a multi-line answer and compared it against the whole one.
#[cfg(target_os = "macos")]
fn encode(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[cfg(target_os = "macos")]
fn decode(hex: &str) -> String {
    let bytes: Option<Vec<u8>> = hex
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect();
    bytes.map_or_else(
        || format!("<undecodable report value {hex:?}>"),
        |bytes| String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// The unconfined side: start the worker the shipped way, then send a child
/// into the real confinement to ask its CI query.
#[cfg(target_os = "macos")]
fn parent() {
    let workspace = match session_workspace() {
        Some(path) => path,
        None => {
            println!("{REPORT}skip: no session workspace to confine");
            return;
        }
    };
    if !xai_grok_sandbox::SandboxManager::support_info().is_supported {
        println!("{REPORT}skip: no kernel sandbox on this host");
        return;
    }
    let branch = branch();
    let query_cwd = fixture_dir("query-cwd");
    println!("{REPORT}branch={branch} workspace={}", workspace.display());

    // What the same query answers with no sandbox anywhere in the picture,
    // from the session workspace. This is the reference the confined answer
    // has to match.
    let baseline = xai_grok_sandbox::ci_host::ci_host_fd();
    assert_eq!(baseline, None, "the parent must start with no worker");
    let direct = query(&workspace, &branch);
    println!("{REPORT}unsandboxed={}", direct.summary());

    // The shipped hand-off: the worker starts here, while this process can
    // still reach the keychain, and the session finds it by fd.
    let fd = xai_grok_sandbox::ci_host::start_ci_host_for_session(&workspace, false)
        .expect("the hand-off must start a worker in the session workspace");
    assert_eq!(
        xai_grok_sandbox::ci_host::ci_host_fd(),
        Some(fd),
        "the session must find the fd the hand-off published"
    );
    println!("{REPORT}fd={fd}");

    // The parent's own query now rides the worker, and must agree with the
    // unsandboxed answer: the worker is a transport, not a different answer.
    let via_worker = query(&query_cwd, &branch);
    println!("{REPORT}unconfined_via_worker={}", via_worker.summary());

    // The child that has a worker. `inherit_across_exec` is how the fd reaches
    // a process the confinement is installed in rather than one it is exec'd
    // into, and it is the shipped half of that contract.
    xai_grok_sandbox::ci_host::inherit_across_exec(fd).expect("clear close-on-exec");
    let with_worker = run_child(MODE_JAILED, &workspace, &query_cwd, &branch, Some(fd));
    println!("{REPORT}{}", with_worker.summary());

    // The child with none: the same confinement, the same query, no worker.
    let without_worker = run_child(MODE_JAILED_NO_WORKER, &workspace, &query_cwd, &branch, None);
    println!("{REPORT}{}", without_worker.summary());

    // The child normally confines itself with the workspace profile. A host
    // that is ALREADY confined cannot nest another profile (`sandbox_init`
    // refuses), and there the child is confined by the profile it inherited;
    // `keychain_before` is how it says so. Either way the query below runs
    // under a real confinement, and the keychain stays out of reach.
    let confined_here = with_worker.applied == "1";
    let confined_by_inheritance = with_worker.applied == "0" && with_worker.keychain_before == "denied";
    assert!(
        confined_here || confined_by_inheritance,
        "the child must be confined: {with_worker}"
    );
    assert_eq!(
        with_worker.keychain, "denied",
        "the confinement must be the real one (the login keychain stays \
         unreachable): {with_worker}"
    );
    println!(
        "{REPORT}confinement={}",
        if confined_here {
            "workspace profile applied in the child"
        } else {
            "workspace profile inherited: this host cannot nest a second one"
        }
    );
    assert_eq!(
        with_worker.fd,
        format!("{fd}"),
        "the confined child must find the worker by the fd name the jail uses: {with_worker}"
    );
    assert_eq!(
        without_worker.fd, "none",
        "the control child must have no worker to find: {without_worker}"
    );

    // Where the child's `gh` ran is what tells the two paths apart: the worker
    // answers from the session workspace, and a `gh` spawned inside the
    // confinement answers from the directory the query named. The child's
    // answer must therefore be the worker's own answer to the same query,
    // whatever that answer is on this host.
    assert_eq!(
        with_worker.answer,
        via_worker.answer,
        "the confined child's query must be answered by the worker, exactly as \
         the unconfined one was: {with_worker}"
    );
    if without_worker.gh_ran() {
        assert!(
            without_worker.answer.contains("not a git repository"),
            "with no worker the query must fall through to a `gh` inside the \
             confinement, running where the query asked: {without_worker}"
        );
    } else {
        println!(
            "{REPORT}no-worker control=no `gh` on this host to run, so the \
             fall-through cannot be observed here"
        );
    }

    // Where the host has an authenticated `gh`, the whole point is that the
    // confined child sees the same runs as the unsandboxed one.
    if direct.is_run_list() {
        assert!(
            with_worker.is_run_list(),
            "a confined session must see the same run list as an unsandboxed \
             one: {with_worker}"
        );
        assert_eq!(
            with_worker.answer, direct.answer,
            "the confined answer must match the unsandboxed one"
        );
        println!("{REPORT}parity=real-runs");
    } else {
        println!(
            "{REPORT}parity=unverified: this host's `gh` answered the \
             unsandboxed query with no run list ({}), so only the transport is \
             asserted",
            direct.summary()
        );
    }
    println!("{REPORT}PASS");
}

/// The confined side. `worker` says whether a worker was handed in; with none,
/// the query is the pre-fix path: a `gh` spawned inside the confinement.
#[cfg(target_os = "macos")]
fn jailed_child(worker: bool) {
    use xai_grok_sandbox::{ProfileName, SandboxManager};

    let workspace = PathBuf::from(std::env::var(WORKSPACE_ENV).expect(WORKSPACE_ENV));
    let query_cwd = PathBuf::from(std::env::var(QUERY_CWD_ENV).expect(QUERY_CWD_ENV));
    let branch = branch();

    // Confine THIS process with the real profile, the way the session does.
    let keychain_before = keychain_state();
    let mut sandbox = SandboxManager::new(ProfileName::Workspace, &workspace);
    let applied = sandbox.apply(&workspace).is_ok() && sandbox.is_applied();
    let fd = xai_grok_sandbox::ci_host::ci_host_fd();
    println!("{REPORT}applied={}", u8::from(applied));
    println!("{REPORT}keychain_before={keychain_before}");
    println!(
        "{REPORT}fd={}",
        match fd {
            Some(fd) => fd.to_string(),
            None => "none".to_string(),
        }
    );
    println!("{REPORT}keychain={}", keychain_state());
    println!("{REPORT}worker={}", u8::from(worker));

    let answer = query(&query_cwd, &branch);
    println!(
        "{REPORT}runs={}",
        if answer.is_run_list() { "yes" } else { "no" }
    );
    println!("{REPORT}answer_text={}", encode(&answer.answer));
    println!("{REPORT}answer_code={}", answer.code);
    println!("{REPORT}child_done");
}

/// Whether the login keychain is reachable from inside this process. `security`
/// answers a bare "valid parameters" error rather than a search list when the
/// keychain mach services are denied, which is exactly what the confinement
/// does to `gh`.
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

/// One answer to the CI query, however it was reached.
#[cfg(target_os = "macos")]
struct Answer {
    /// What `gh` printed, clipped to a reportable length.
    answer: String,
    /// The exit code, or `-1` when the query reached no `gh` at all.
    code: i32,
}

#[cfg(target_os = "macos")]
impl Answer {
    fn summary(&self) -> String {
        format!("code={} answer={:?}", self.code, self.answer)
    }

    /// Whether this answer is a real `gh run list` array.
    fn is_run_list(&self) -> bool {
        self.code == 0
            && self.answer.starts_with('[')
            && serde_json::from_str::<serde_json::Value>(&self.answer)
                .is_ok_and(|value| value.is_array())
    }
}

/// The shipped query path: the same argv the `ci` tool's `status` sends, run
/// wherever this process can reach `gh` (the worker when it has one, its own
/// `gh` otherwise).
#[cfg(target_os = "macos")]
fn query(query_cwd: &Path, branch: &str) -> Answer {
    let args = [
        "run",
        "list",
        "--branch",
        branch,
        "--limit",
        "10",
        "--json",
        "status,conclusion,headBranch,workflowName",
    ];
    match xai_grok_sandbox::ci_host::run_gh(query_cwd, &args) {
        Some(response) => {
            let text = if response.stdout.trim().is_empty() {
                response.stderr.trim().to_string()
            } else {
                response.stdout.trim().to_string()
            };
            Answer {
                answer: clip(&text),
                code: response.code,
            }
        }
        None => Answer {
            answer: "no gh".to_string(),
            code: -1,
        },
    }
}

#[cfg(target_os = "macos")]
fn clip(text: &str) -> String {
    const MAX: usize = 400;
    if text.len() <= MAX {
        return text.to_string();
    }
    let end = (0..=MAX).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    format!("{}...", &text[..end])
}

/// What a child reported back.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct ChildReport {
    applied: String,
    fd: String,
    keychain: String,
    answer: String,
    /// Whether the child's own answer was a real `gh run list` array.
    runs: String,
    /// The exit code of the child's answer, or `-1` when no `gh` ran at all.
    answer_code: String,
    /// Whether the keychain was already unreachable BEFORE the child applied
    /// anything: a host that is confined already cannot nest a second profile,
    /// and that is how this child says it inherited one.
    keychain_before: String,
    /// Whatever the child printed on stderr, so a refused confinement says why.
    stderr: String,
}

#[cfg(target_os = "macos")]
impl ChildReport {
    fn summary(&self) -> String {
        format!(
            "applied={} fd={} keychain={} keychain_before={} runs={} answer={} answer_code={} stderr={:?}",
            self.applied,
            self.fd,
            self.keychain,
            self.keychain_before,
            self.runs,
            self.answer,
            self.answer_code,
            self.stderr
        )
    }

    fn is_run_list(&self) -> bool {
        self.runs == "yes"
    }

    /// Whether a `gh` actually ran for this child, or there was none to run.
    fn gh_ran(&self) -> bool {
        self.answer_code != "-1"
    }
}

#[cfg(target_os = "macos")]
impl std::fmt::Display for ChildReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary())
    }
}

/// Re-enter this binary in `mode`, with the workspace, the query's directory,
/// the branch, and (when `fd` is given) the worker's fd named in the env var
/// the jail boundary uses.
#[cfg(target_os = "macos")]
fn run_child(
    mode: &str,
    workspace: &Path,
    query_cwd: &Path,
    branch: &str,
    fd: Option<i32>,
) -> ChildReport {
    let mut cmd = std::process::Command::new(std::env::current_exe().expect("current exe"));
    cmd.env(MODE_ENV, mode)
        .env(WORKSPACE_ENV, workspace)
        .env(QUERY_CWD_ENV, query_cwd)
        .env(BRANCH_ENV, branch)
        // The confinement grants writes to this process's `$GROK_HOME`, and the
        // hook write-deny step materializes directories under it: a fixture
        // keeps the session's own home out of a test run.
        .env("GROK_HOME", fixture_dir("grok-home"));
    match fd {
        Some(fd) => cmd.env(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV, fd.to_string()),
        None => cmd.env_remove(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV),
    };
    let output = cmd.output().expect("spawn the confined child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut report = ChildReport {
        applied: "?".to_string(),
        fd: "?".to_string(),
        keychain: "?".to_string(),
        answer: "?".to_string(),
        runs: "?".to_string(),
        answer_code: "?".to_string(),
        keychain_before: "?".to_string(),
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
            ("applied=", &mut report.applied),
            ("fd=", &mut report.fd),
            ("keychain_before=", &mut report.keychain_before),
            ("keychain=", &mut report.keychain),
            ("runs=", &mut report.runs),
            ("answer_code=", &mut report.answer_code),
            ("answer_text=", &mut report.answer),
        ] {
            if let Some(value) = rest.strip_prefix(key) {
                if key == "answer_text=" {
                    *slot = decode(value.trim());
                } else {
                    *slot = value.split_whitespace().next().unwrap_or("").to_string();
                }
            }
        }
    }
    assert!(
        saw_done,
        "the {mode} child must report back; stdout={stdout} stderr={stderr}"
    );
    report
}

/// The branch under test: this repository's own, or `master`.
#[cfg(target_os = "macos")]
fn branch() -> String {
    std::env::var(BRANCH_ENV).unwrap_or_else(|_| "master".to_string())
}

/// The repository root, which is the session workspace a `--sandbox=workspace`
/// session in this repo would confine to.
#[cfg(target_os = "macos")]
fn session_workspace() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let root = dunce::canonicalize(root).ok()?;
    root.join(".git").exists().then_some(root)
}

/// A unique directory under the target tree, so a confined child has somewhere
/// to write that the profile grants.
#[cfg(target_os = "macos")]
fn fixture_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "grok-profile-ci-host-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create fixture dir");
    path
}
