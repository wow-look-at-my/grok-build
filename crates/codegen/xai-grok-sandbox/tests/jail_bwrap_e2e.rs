//! Drives the shipped `--sandbox` bwrap builder against real bubblewrap.
//!
//! The unit tests read the argv the builder produces. They cannot say whether
//! bubblewrap accepts that argv, or whether the jail confines anything, and a
//! flag set that only looks right is the failure this file exists to catch.

#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use xai_grok_sandbox::jail::{Access, JailDefaults, JailPlan, Mount, bwrap_command};

/// Whether bubblewrap can build a sandbox here. A host that forbids an
/// unprivileged user namespace has the binary and cannot use it.
fn bwrap_is_usable() -> bool {
    Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "true"])
        .status()
        .is_ok_and(|status| status.success())
}

/// Fixtures live under the target directory, not under `/tmp`. A fixture
/// under `/tmp` is bound back in over the jail's tmpfs, which is exactly what
/// the "tmp is fresh" assertion below is looking for.
fn fixture_dir(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grok-jail-e2e-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    dunce::canonicalize(&path).unwrap()
}

fn plan(script: &str, mounts: Vec<Mount>, grok_home: &Path, cwd: &Path) -> JailPlan {
    JailPlan {
        mounts,
        grok_home: grok_home.to_path_buf(),
        temp_dir: PathBuf::from("/tmp"),
        deny_sink: grok_home.join("sandbox-tmp").join("deny-sink"),
        self_exe: PathBuf::from("/bin/sh"),
        cwd: cwd.to_path_buf(),
        args: vec![OsString::from("-c"), OsString::from(script)],
        // These e2e scenarios are the release-default jail (ro base, tmpfs
        // /tmp, rw $GROK_HOME); they pin that the default bwrap argv confines.
        defaults: JailDefaults::default(),
        // No host worker: these scenarios exec a shell script inside the jail.
        ci_host_fd: None,
    }
}

/// One jail, every guarantee the flag promises: the working directory
/// survives, a `--ro` path refuses a write, `$GROK_HOME` takes one, `/tmp` is
/// a fresh tmpfs, and an unbound path is absent.
#[test]
fn the_jail_binds_what_it_promises_and_nothing_else() {
    if !bwrap_is_usable() {
        eprintln!("SKIP: bubblewrap cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("work");
    let readonly = fixture_dir("readonly");
    let unbound = fixture_dir("unbound");
    let grok_home = fixture_dir("grok-home");
    std::fs::write(readonly.join("data"), "secret").unwrap();
    let script = format!(
        r#"
        set -u
        echo "cwd=$(pwd)"
        touch ./writable && echo work_rw
        cat {ro}/data >/dev/null && echo readonly_readable
        touch {ro}/probe 2>/dev/null && echo READONLY_WAS_WRITABLE || echo readonly_denied
        touch {home}/probe && echo grok_home_rw
        test -e {unbound} && echo UNBOUND_WAS_VISIBLE || echo unbound_absent
        test -z "$(ls -A /tmp)" && echo tmp_is_fresh || echo TMP_LEAKED
        echo "jail=${{__GROK_SANDBOX_JAIL:-unset}}"
        "#,
        ro = readonly.display(),
        home = grok_home.display(),
        unbound = unbound.display(),
    );
    let mounts = vec![
        Mount {
            access: Access::Rw,
            path: work.clone(),
        },
        Mount {
            access: Access::Ro,
            path: readonly.clone(),
        },
    ];
    let output = bwrap_command(&plan(&script, mounts, &grok_home, &work))
        .output()
        .expect("bwrap must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the generated bwrap argv must be valid: {stderr}"
    );
    for expected in [
        &format!("cwd={}", work.display()) as &str,
        "work_rw",
        "readonly_readable",
        "readonly_denied",
        "grok_home_rw",
        "unbound_absent",
        "tmp_is_fresh",
        "jail=1",
    ] {
        assert!(stdout.contains(expected), "missing '{expected}': {stdout}");
    }
    for dir in [&work, &readonly, &unbound, &grok_home] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The whole point of the ordering rule: `--rw DIR --ro DIR/sub` keeps the
/// directory writable and takes the subtree back.
#[test]
fn a_later_flag_beats_an_earlier_one() {
    if !bwrap_is_usable() {
        eprintln!("SKIP: bubblewrap cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("precedence");
    let nested = work.join("secrets");
    std::fs::create_dir_all(&nested).unwrap();
    let grok_home = fixture_dir("precedence-home");
    let script = "touch ./outer && echo outer_rw; touch ./secrets/inner 2>/dev/null \
                  && echo INNER_WAS_WRITABLE || echo inner_denied";
    let mounts = vec![
        Mount {
            access: Access::Rw,
            path: work.clone(),
        },
        Mount {
            access: Access::Ro,
            path: nested,
        },
    ];
    let output = bwrap_command(&plan(script, mounts, &grok_home, &work))
        .output()
        .expect("bwrap must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("outer_rw"), "{stdout}");
    assert!(stdout.contains("inner_denied"), "{stdout}");
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&grok_home);
}

/// Names the fd the parent handed this child its socket on.
const WORKER_FD_ENV: &str = "GROK_CI_HOST_TEST_FD";
/// The branch the jailed child asks about; its presence is also the switch that
/// makes the child run in jailed-client mode rather than as a test.
const JAILED_CHILD_ENV: &str = "GROK_JAILED_CHILD_BRANCH";

/// A `--sandbox` session reaches `gh` only through the host worker, and the
/// jail is entered by exec, so the ONE thing that carries the connection is an
/// exec-surviving fd whose number the jail sets in the new image's environment.
///
/// bwrap makes that a placement question the macOS side does not have:
/// `--setenv` is an option for bwrap and everything after `--` is argv for the
/// jailed program. Emitting it on the wrong side of that separator sets no
/// variable at all, and the jailed pager then reports no CI while the run
/// gains three stray arguments. Only running real bubblewrap catches it.
///
/// The answer is checked for SHAPE, not content: a runner with no `gh` (and no
/// credentials) is answered by the worker's nothing-usable sentinel, and that
/// still proves the jail carried the connection, which is the claim here.
#[test]
fn the_jailed_process_reaches_the_host_worker_through_the_bwrap_jail() {
    if !bwrap_is_usable() {
        eprintln!("SKIP: bubblewrap cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("worker-work");
    let grok_home = fixture_dir("worker-home");
    std::fs::create_dir_all(grok_home.join("sandbox-tmp")).unwrap();

    // The host side: the shipped worker loop in a real child of this binary,
    // its socket end dup2'd onto a known fd - what `spawn_ci_host` does before
    // a jail exec.
    let (ours, theirs) = UnixStream::pair().expect("socketpair");
    let theirs_fd = theirs.into_raw_fd();
    const WORKER_FD: i32 = 3;
    let mut worker = Command::new(std::env::current_exe().expect("current test binary"));
    worker
        .env(xai_grok_sandbox::ci_host::CI_HOST_MARKER_ENV, "1")
        .env(WORKER_FD_ENV, WORKER_FD.to_string())
        .arg("--exact")
        .arg("bwrap_worker_self_entry")
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

    let answer_path = work.join("jail-answer.txt");
    let mounts = vec![Mount {
        access: Access::Rw,
        path: work.clone(),
    }];
    let mut jailed = plan("", mounts, &grok_home, &work);
    jailed.self_exe = std::env::current_exe().expect("current test binary");
    jailed.args = vec![
        OsString::from("--exact"),
        OsString::from("bwrap_jailed_child_self_entry"),
    ];
    jailed.ci_host_fd = Some(client_fd);

    let output = bwrap_command(&jailed)
        // The branch is also the switch that puts the child in jailed-client
        // mode, exactly as the marker env names the worker.
        .env(JAILED_CHILD_ENV, "master")
        .output()
        .expect("bwrap must run");
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
        "the jailed child must find the worker's fd in the env var the jail set, \
         which is what `--setenv` before bwrap's `--` is for: {answer}"
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
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&grok_home);
}

/// Delegate the parent spawns as the unsandboxed host worker.
#[test]
fn bwrap_worker_self_entry() {
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
fn bwrap_jailed_child_self_entry() {
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
fn the_bwrap_jail_advertises_no_host_worker_when_none_was_started() {
    if !bwrap_is_usable() {
        eprintln!("SKIP: bubblewrap cannot build a sandbox on this host");
        return;
    }
    let work = fixture_dir("no-worker-work");
    let grok_home = fixture_dir("no-worker-home");
    std::fs::create_dir_all(grok_home.join("sandbox-tmp")).unwrap();

    let mounts = vec![Mount {
        access: Access::Rw,
        path: work.clone(),
    }];
    let output = bwrap_command(&plan(
        "printf 'env=%s\\n' \"${GROK_CI_HOST_FD:-unset}\"",
        mounts,
        &grok_home,
        &work,
    ))
    .output()
    .expect("bwrap must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the jail must start the command: {stdout}"
    );
    assert!(
        stdout.contains("env=unset"),
        "no worker means no fd in the jail: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&grok_home);
}
