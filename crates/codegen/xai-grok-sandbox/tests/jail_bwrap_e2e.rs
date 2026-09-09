//! Drives the shipped `--sandbox` bwrap builder against real bubblewrap.
//!
//! The unit tests read the argv the builder produces. They cannot say whether
//! bubblewrap accepts that argv, or whether the jail confines anything, and a
//! flag set that only looks right is the failure this file exists to catch.

#![cfg(target_os = "linux")]

use std::ffi::OsString;
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
