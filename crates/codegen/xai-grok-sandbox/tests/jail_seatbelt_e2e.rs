//! Drives the shipped `--sandbox` Seatbelt builder against real `sandbox-exec`.
//!
//! The unit tests read the command the builder produces. They cannot say
//! whether `sandbox-exec` accepts it, nor — the part that matters for the CI
//! dot — whether the host worker's fd and its `GROK_CI_HOST_FD` name actually
//! arrive inside the jail. A command that merely looks right is the failure
//! this file exists to catch, and on macOS it is the only test that launches
//! the jail at all.

#![cfg(target_os = "macos")]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use xai_grok_sandbox::jail::{Access, JailDefaults, JailPlan, Mount, seatbelt_command};

/// Whether `sandbox-exec` can build a sandbox here. The binary is deprecated
/// and absent on some hosts; where it is gone the jail cannot start at all.
fn sandbox_exec_is_usable() -> bool {
    Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
}

/// A unique fixture directory under the target directory, canonicalized: the
/// Seatbelt profile matches canonical paths, so the plan it is built from must
/// name them the way the kernel does.
fn fixture_dir(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grok-jail-seatbelt-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    dunce::canonicalize(&path).unwrap()
}

/// The release-default jail around a shell script, as `jail_bwrap_e2e.rs`
/// builds for Linux: the cwd mounted read-write, the rest of the filesystem
/// confined by the profile.
fn plan(script: &str, grok_home: &Path, cwd: &Path) -> JailPlan {
    JailPlan {
        mounts: vec![Mount {
            access: Access::Rw,
            path: cwd.to_path_buf(),
        }],
        grok_home: grok_home.to_path_buf(),
        temp_dir: grok_home.join("sandbox-tmp"),
        deny_sink: grok_home.join("sandbox-tmp").join("deny-sink"),
        self_exe: PathBuf::from("/bin/sh"),
        cwd: cwd.to_path_buf(),
        args: vec![OsString::from("-c"), OsString::from(script)],
        defaults: JailDefaults::default(),
        // Set per test: the point of these two is what happens with, and
        // without, a host worker to hand over.
        ci_host_fd: None,
    }
}

/// The jailed process must find the worker both ways: the fd itself (which is
/// what the connection rides on) and the env var naming it (which is how the
/// pager finds the connection). Neither can be merely present in the emitted
/// command — the fd has to survive `sandbox-exec`'s exec, and the env var has
/// to be in the child's environment.
#[test]
fn the_seatbelt_jail_hands_the_host_worker_fd_and_its_env_to_the_jailed_process() {
    if !sandbox_exec_is_usable() {
        eprintln!("SKIP: sandbox-exec cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("work");
    let grok_home = fixture_dir("grok-home");
    std::fs::create_dir_all(grok_home.join("sandbox-tmp")).unwrap();

    // The host worker's own arrangement: a socketpair whose far end stays in
    // the unsandboxed process, its fd made exec-surviving, and its NUMBER
    // passed into the jail as `GROK_CI_HOST_FD`.
    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    let fd = ours.as_raw_fd();
    xai_grok_sandbox::ci_host::inherit_across_exec(fd).expect("clear close-on-exec");

    // Beyond the fd, this is the profile's path-resolution half: a grant whose
    // ancestors cannot be stat-ed leaves the jailed process unable to resolve
    // its own working directory. An unbound path must stay invisible at the
    // same time — that confinement is what the profile is for.
    let unbound = fixture_dir("unbound");
    let script = format!(
        "printf 'env=%s\\n' \"${{GROK_CI_HOST_FD:-unset}}\"\n\
         printf 'cwd=%s\\n' \"$(pwd)\"\n\
         touch ./probe && printf 'cwd_writable=1\\n'\n\
         test -e {unbound} && printf 'UNBOUND_WAS_VISIBLE\\n' || printf 'unbound_absent=1\\n'\n\
         printf alive >&{fd}\n",
        unbound = unbound.display(),
    );
    let mut jailed = plan(&script, &grok_home, &work);
    jailed.ci_host_fd = Some(fd);

    let output = seatbelt_command(&jailed)
        .output()
        .expect("sandbox-exec must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the jail must start the command: status={} stdout={stdout} stderr={stderr}\n{}",
        output.status,
        xai_grok_sandbox::jail::seatbelt_profile(&jailed)
    );
    assert!(
        stdout.contains(&format!("env={fd}")),
        "the jailed process must read the worker's fd from the env var the \
         backend set: {stdout}"
    );
    assert!(
        stdout.contains(&format!("cwd={}", work.display())),
        "the jailed process must resolve its own working directory: {stdout}"
    );
    assert!(
        stdout.contains("cwd_writable=1"),
        "the cwd mount must be writable inside the jail: {stdout}"
    );
    assert!(
        stdout.contains("unbound_absent=1"),
        "a path the jail did not grant must stay invisible: {stdout}"
    );

    // The fd is the connection, not just a number: a write from inside the
    // jail has to reach the peer outside it.
    let mut alive = String::new();
    theirs
        .take(5)
        .read_to_string(&mut alive)
        .expect("read the jailed write");
    assert_eq!(
        alive, "alive",
        "the inherited fd must survive sandbox-exec into the jailed process"
    );
}

/// The whole point of the fd contract, end to end on macOS: a process inside
/// the real Seatbelt jail, holding only the fd number the jail handed it,
/// reaches the real host worker — the shipped worker loop running as a real
/// unsandboxed child — and gets a framed answer back.
///
/// The answer is checked for SHAPE, not content: a runner with no `gh` (and no
/// credentials) is answered by the worker's nothing-usable sentinel, and that
/// still proves the jail carried the connection, which is the claim here.
#[test]
fn the_jailed_process_reaches_the_host_worker_through_the_seatbelt_jail() {
    if !sandbox_exec_is_usable() {
        eprintln!("SKIP: sandbox-exec cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("worker-work");
    let grok_home = fixture_dir("worker-home");
    std::fs::create_dir_all(grok_home.join("sandbox-tmp")).unwrap();

    // The host side: the shipped worker loop in a real child of this binary,
    // its socket end dup2'd onto a known fd — what `spawn_ci_host` does before
    // a jail exec.
    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    let theirs_fd = theirs.into_raw_fd();
    const WORKER_FD: i32 = 3;
    let mut worker = Command::new(std::env::current_exe().expect("current test binary"));
    worker
        .env(xai_grok_sandbox::ci_host::CI_HOST_MARKER_ENV, "1")
        .env(WORKER_FD_ENV, WORKER_FD.to_string())
        .arg("--exact")
        .arg("seatbelt_worker_self_entry")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        // SAFETY: between fork and exec, `dup2` only; it also clears CLOEXEC on
        // the new fd, which is what carries the socket into the worker.
        std::os::unix::process::CommandExt::pre_exec(&mut worker, move || {
            if libc::dup2(theirs_fd, WORKER_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut worker = worker.spawn().expect("spawn the host worker");
    // The child owns the only write end now, or the worker waits for a request
    // that never comes.
    unsafe {
        libc::close(theirs_fd);
    }

    let client_fd = ours.as_raw_fd();
    xai_grok_sandbox::ci_host::inherit_across_exec(client_fd).expect("clear close-on-exec");
    std::mem::forget(ours); // the jailed child owns it from here.

    // The jailed side: this test binary, in the jail, resolving the worker the
    // way the shipped pager does — from the env var the jail set.
    let answer_path = work.join("jail-answer.txt");
    let mut jailed = plan("", &grok_home, &work);
    jailed.self_exe = std::env::current_exe().expect("current test binary");
    jailed.args = vec![
        OsString::from("--exact"),
        OsString::from("seatbelt_jailed_child_self_entry"),
    ];
    jailed.ci_host_fd = Some(client_fd);

    let output = seatbelt_command(&jailed)
        // The branch is also the switch that puts the child in jailed-client
        // mode, exactly as the marker env names the worker.
        .env(JAILED_CHILD_ENV, "master")
        .output()
        .expect("sandbox-exec must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the jailed child must run: status={} stdout={stdout} stderr={stderr}",
        output.status
    );

    let answer = std::fs::read_to_string(&answer_path)
        .unwrap_or_else(|e| panic!("the jailed child must report back ({e}): {stdout}{stderr}"));
    assert!(
        !answer.contains("fd=none"),
        "the jailed child must find the worker's fd in the env var: {answer}"
    );
    let answered = answer
        .lines()
        .find_map(|line| line.strip_prefix("answer="))
        .unwrap_or_else(|| panic!("the jailed child must report its answer: {answer}"));
    assert!(
        answered.starts_with('[') || answered == ".",
        "the worker must answer a jailed request with a run list or its \
         nothing-usable sentinel, not nothing at all: {answered:?}"
    );

    // The jailed child is gone, but its copy of the client end was a dup: this
    // process still holds the original, and until that closes the worker's read
    // loop has no EOF to end on. (`ours` is forgotten above precisely so this
    // is the single close of that fd.)
    unsafe {
        libc::close(client_fd);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match worker.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => panic!("the host worker must exit once the jail lets go of the socket"),
            Err(e) => panic!("waiting on the host worker: {e}"),
        }
    }
}

/// Names the fd the parent handed this child its socket on.
const WORKER_FD_ENV: &str = "GROK_CI_HOST_TEST_FD";
/// The branch the jailed child asks about; its presence is also the switch that
/// makes the child run in jailed-client mode rather than as a test.
const JAILED_CHILD_ENV: &str = "GROK_JAILED_CHILD_BRANCH";

/// Delegate the parent spawns as the unsandboxed host worker.
#[test]
fn seatbelt_worker_self_entry() {
    if !xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        return; // only meaningful when spawned as the worker
    }
    let Some(fd) = std::env::var(WORKER_FD_ENV)
        .ok()
        .and_then(|raw| raw.parse::<i32>().ok())
    else {
        return;
    };
    // SAFETY: the parent dup2'd its socketpair end onto this fd before exec,
    // and nothing else in this process owns it.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    xai_grok_sandbox::ci_host::run_ci_host_worker_on(stream);
}

/// Delegate the parent jails: find the worker the way the shipped pager does,
/// ask it one fixed-shape question, and write what came back where the parent
/// can read it (the jailed cwd is a granted mount).
#[test]
fn seatbelt_jailed_child_self_entry() {
    let Ok(branch) = std::env::var(JAILED_CHILD_ENV) else {
        return; // only meaningful when jailed by the parent
    };
    let work = std::env::current_dir().expect("cwd");
    let mut report = String::new();
    let Some(fd) = xai_grok_sandbox::ci_host::ci_host_fd() else {
        report.push_str("fd=none\n");
        std::fs::write(work.join("jail-answer.txt"), report).unwrap();
        return;
    };
    report.push_str(&format!("fd={fd}\n"));
    // SAFETY: the jail handed this process the fd by number and nothing else
    // in this process owns it.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let answer = match stream.write_all(format!("gh-status {branch}\n").as_bytes()) {
        Err(e) => format!("write failed: {e}"),
        Ok(()) => {
            let mut line = String::new();
            match BufReader::new(stream).read_line(&mut line) {
                Ok(0) => "no answer (connection closed)".to_string(),
                Ok(_) => line.trim_end().to_string(),
                Err(e) => format!("read failed: {e}"),
            }
        }
    };
    report.push_str(&format!("answer={answer}\n"));
    std::fs::write(work.join("jail-answer.txt"), report).unwrap();
}

/// With no worker there is nothing to hand over, and the jailed process must
/// see no fd rather than an inherited one it did not ask for. That is the
/// dot's "off" state: no CI, not a query against a stale descriptor.
#[test]
fn the_seatbelt_jail_advertises_no_host_worker_when_none_was_started() {
    if !sandbox_exec_is_usable() {
        eprintln!("SKIP: sandbox-exec cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("no-worker-work");
    let grok_home = fixture_dir("no-worker-home");
    std::fs::create_dir_all(grok_home.join("sandbox-tmp")).unwrap();

    let output = seatbelt_command(&plan(
        "printf 'env=%s\\n' \"${GROK_CI_HOST_FD:-unset}\"",
        &grok_home,
        &work,
    ))
    .output()
    .expect("sandbox-exec must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the jail must start the command: {stdout}"
    );
    assert!(
        stdout.contains("env=unset"),
        "no worker means no fd in the jail: {stdout}"
    );
}
