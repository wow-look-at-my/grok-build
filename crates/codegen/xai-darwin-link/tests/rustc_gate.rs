//! Checks that `ci/rustc-gate.sh` rations compiles and reports what the compiler said.
//!
//! The gate is what carries the RAM cap now that cargo runs at a high -j. sccache execs it only
//! when it is going to compile, so the cap applies to compiles and not to the roughly 1800 cache
//! lookups this workspace makes. Two properties have to hold, and both were broken at some point
//! while it was written.
//!
//! The cap has to bind: a stand-in compiler records when it ran, so overlap is countable, and a
//! run against as many slots as launches proves the harness can see an uncapped peak.
//!
//! The compiler's exit code has to survive. The gate reads "the slot was busy" off flock, and
//! reading that from the exit code made a compiler exiting with the same number look like
//! contention. It was retried on the next slot, and the next, until the job's timeout.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn gate() -> PathBuf {
    repo_root().join("ci/rustc-gate.sh")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustc-gate-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Runs the gate once with a stand-in compiler, and gives back the gate's own exit code.
fn run_gate(dir: &Path, real: &Path, slots: usize, env: &[(&str, &str)]) -> Option<i32> {
    let mut cmd = Command::new("bash");
    cmd.arg(gate())
        .args(["--crate-name", "probe", "--emit=link"])
        .env("GATE_REAL_RUSTC", real)
        .env("GATE_SLOT_DIR", dir.join("slots"))
        .env("GATE_SLOTS", slots.to_string());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.status().unwrap().code()
}

/// The peak number of stand-in compiles that overlapped, read back off their own timestamps.
fn peak_overlap(log: &Path) -> usize {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut events: Vec<(f64, i32)> = text
        .lines()
        .filter_map(|line| {
            let (kind, at) = line.split_once(' ')?;
            Some((at.parse().ok()?, if kind == "start" { 1 } else { -1 }))
        })
        .collect();
    // An end sharing a timestamp with a start is applied first, so a slot handed straight on does
    // not read as two compiles at once.
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut live = 0;
    let mut peak = 0;
    for (_, delta) in events {
        live += delta;
        peak = peak.max(live);
    }
    peak as usize
}

fn launch_many(dir: &Path, real: &Path, slots: usize, launches: usize) -> usize {
    let log = dir.join("intervals.log");
    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_dir_all(dir.join("slots"));

    let children: Vec<_> = (0..launches)
        .map(|_| {
            Command::new("bash")
                .arg(gate())
                .args(["--crate-name", "probe", "--emit=link"])
                .env("GATE_REAL_RUSTC", real)
                .env("GATE_SLOT_DIR", dir.join("slots"))
                .env("GATE_SLOTS", slots.to_string())
                .env("GATE_TEST_LOG", &log)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success(), "a gated compile failed");
    }
    peak_overlap(&log)
}

/// A stand-in compiler that records the window it ran in.
fn timed_compiler(dir: &Path) -> PathBuf {
    let path = dir.join("timed-rustc");
    write_exe(
        &path,
        "#!/bin/bash\n\
         printf 'start %s\\n' \"$(date +%s.%N)\" >> \"$GATE_TEST_LOG\"\n\
         sleep 0.4\n\
         printf 'end %s\\n' \"$(date +%s.%N)\" >> \"$GATE_TEST_LOG\"\n",
    );
    path
}

#[test]
fn the_cap_holds_and_every_compile_still_runs() {
    let dir = scratch("cap");
    let compiler = timed_compiler(&dir);

    // As many slots as launches, so nothing is held back. A harness that cannot see this peak
    // cannot see the cap failing either.
    let uncapped = launch_many(&dir, &compiler, 12, 12);
    assert_eq!(uncapped, 12, "the harness cannot observe an uncapped peak");

    let capped = launch_many(&dir, &compiler, 3, 12);
    assert_eq!(capped, 3, "more compiles ran at once than the gate allows");
}

#[test]
fn a_probe_is_never_made_to_queue() {
    let dir = scratch("probe");
    // Holds every slot for long enough that a queued probe would time out the test rather than
    // return. Cargo issues these constantly and they allocate nothing worth rationing.
    let compiler = timed_compiler(&dir);
    let log = dir.join("intervals.log");
    let held: Vec<_> = (0..3)
        .map(|_| {
            Command::new("bash")
                .arg(gate())
                .args(["--crate-name", "probe", "--emit=link"])
                .env("GATE_REAL_RUSTC", &compiler)
                .env("GATE_SLOT_DIR", dir.join("slots"))
                .env("GATE_SLOTS", "3")
                .env("GATE_TEST_LOG", &log)
                .spawn()
                .unwrap()
        })
        .collect();

    let echo = dir.join("echo-rustc");
    write_exe(&echo, "#!/bin/bash\nexit 0\n");
    let code = Command::new("bash")
        .arg(gate())
        .arg("--print=cfg")
        .env("GATE_REAL_RUSTC", &echo)
        .env("GATE_SLOT_DIR", dir.join("slots"))
        .env("GATE_SLOTS", "3")
        .status()
        .unwrap()
        .code();
    assert_eq!(code, Some(0), "a --print probe waited on a slot");

    for mut child in held {
        let _ = child.wait();
    }
}

#[test]
fn the_compilers_exit_code_survives_the_gate() {
    let dir = scratch("exit");
    let compiler = dir.join("exit-with");
    write_exe(&compiler, "#!/bin/bash\nexit \"${WANT_EXIT:-0}\"\n");

    // 99 is the value the gate uses for a busy slot. Reading the compiler's status off the same
    // channel made this one retry forever instead of failing.
    for want in ["0", "1", "101", "99"] {
        let code = run_gate(&dir, &compiler, 3, &[("WANT_EXIT", want)]);
        assert_eq!(
            code,
            Some(want.parse().unwrap()),
            "the gate changed a compiler exit of {want}"
        );
    }
}
