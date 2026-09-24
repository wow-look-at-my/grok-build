//! Drives `ci/cache-deps.sh prune` over a synthetic target directory.
//!
//! The prune decides what the dependency cache entry carries. Over-matching deletes a
//! registry artifact the entry exists to hold. Under-matching leaves this workspace's own
//! test binaries in it, which are most of the bytes.
//!
//! It lives beside the tests for `ci/darwin-relink.sh` and `ci/zig-cc` because this crate
//! is where a `ci/` script's test goes. A crate holding nothing but this test would add a
//! workspace member, and that moves `Cargo.lock`, which is the key the cache entry uses.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the manifest dir has a repo root above it")
        .to_path_buf()
}

/// A `cargo` that answers `metadata` and nothing else, so the check states its own
/// workspace rather than depending on what this repo happens to hold today.
fn stub_cargo(dir: &Path, metadata: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        format!("#!/bin/sh\ncat <<'JSON'\n{metadata}\nJSON\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn touch(path: PathBuf) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn prune_keeps_registry_artifacts_and_takes_every_workspace_one() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cache-deps");
    let _ = fs::remove_dir_all(&tmp);
    let profile = tmp.join("target/debug");

    let workspace = [
        "deps/libxai_grok_pager-aaaa.rlib",
        "deps/xai_grok_pager-aaaa.d",
        "deps/libxai_grok_shell-cccc.rlib",
        "deps/libxai_grok_pager_pty_harness-dddd.rlib",
        // Stemmed by TARGET name, with no package name in it anywhere.
        "deps/pty_e2e_smoke-bbbb",
        ".fingerprint/xai-grok-pager-aaaa/lib-xai_grok_pager",
        ".fingerprint/xai-grok-pager-pty-harness-dddd/lib",
        "build/xai-grok-shell-cccc/output",
    ];
    // `xai_grok_pagerish` is the near miss: a bare prefix match takes it and must not.
    let registry = [
        "deps/libserde-1111.rlib",
        "deps/libtokio-2222.rlib",
        "deps/libxai_grok_pagerish-3333.rlib",
        ".fingerprint/serde-1111/lib-serde",
        "build/aws-lc-sys-4444/output",
    ];
    for rel in workspace.iter().chain(registry.iter()) {
        touch(profile.join(rel));
    }

    let metadata = r#"{"packages":[
 {"name":"xai-grok-pager","targets":[{"name":"xai-grok-pager"},{"name":"pty_e2e_smoke"}]},
 {"name":"xai-grok-shell","targets":[{"name":"xai-grok-shell"}]},
 {"name":"xai-grok-pager-pty-harness","targets":[{"name":"xai-grok-pager-pty-harness"}]}]}"#;
    let bin = stub_cargo(&tmp, metadata);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = Command::new(repo_root().join("ci/cache-deps.sh"))
        .args(["prune", profile.to_str().unwrap()])
        .env("PATH", path)
        .output()
        .expect("cache-deps.sh runs");
    assert!(
        out.status.success(),
        "prune failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    for rel in workspace {
        assert!(
            !profile.join(rel).exists(),
            "{rel} survived the prune, so the cache entry would carry a workspace artifact"
        );
    }
    for rel in registry {
        assert!(
            profile.join(rel).exists(),
            "{rel} was pruned, so the cache entry loses a registry artifact it exists to hold"
        );
    }
}

fn run_script(args: &[&str], path: Option<&str>, cwd: &Path) -> std::process::Output {
    let mut cmd = Command::new(repo_root().join("ci/cache-deps.sh"));
    cmd.args(args).current_dir(cwd);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let out = cmd.output().expect("cache-deps.sh runs");
    assert!(
        out.status.success(),
        "cache-deps.sh {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn a_stashed_prune_puts_back_every_workspace_artifact_unchanged() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cache-deps-stash");
    let _ = fs::remove_dir_all(&tmp);
    let profile = tmp.join("target/debug");
    let stash = tmp.join("stash");

    let workspace = [
        "deps/libxai_grok_shell-cccc.rlib",
        ".fingerprint/xai-grok-shell-cccc/lib-xai_grok_shell",
        "build/xai-grok-shell-cccc/output",
    ];
    let registry = "deps/libserde-1111.rlib";
    for rel in workspace.iter().chain([&registry]) {
        touch(profile.join(rel));
    }
    let mtime = |p: PathBuf| fs::metadata(p).unwrap().modified().unwrap();
    let before: Vec<_> = workspace.iter().map(|r| mtime(profile.join(r))).collect();

    let bin = stub_cargo(
        &tmp,
        r#"{"packages":[{"name":"xai-grok-shell","targets":[{"name":"xai-grok-shell"}]}]}"#,
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (profile_s, stash_s) = (profile.to_str().unwrap(), stash.to_str().unwrap());

    run_script(&["prune", profile_s, stash_s], Some(&path), &tmp);
    for rel in workspace {
        assert!(
            !profile.join(rel).exists(),
            "{rel} stayed in the cached dir"
        );
    }
    assert!(
        profile.join(registry).exists(),
        "the prune took a registry artifact"
    );

    run_script(&["unstash", profile_s, stash_s], Some(&path), &tmp);
    for (rel, then) in workspace.iter().zip(before) {
        assert_eq!(
            mtime(profile.join(rel)),
            then,
            "{rel} came back with a new mtime, so cargo would rebuild it"
        );
    }
    assert!(!stash.exists(), "unstash left the stash behind");

    // A cached run skipped on a hit stashes nothing. Unstash must then succeed and change nothing.
    run_script(&["unstash", profile_s, stash_s], Some(&path), &tmp);
}

/// The point of the stash: after prune and unstash, the same build compiles nothing.
#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn cargo_reads_unstashed_workspace_artifacts_as_fresh() {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cache-deps-cargo");
    let _ = fs::remove_dir_all(&tmp);
    let ws = tmp.join("ws");
    let write = |rel: &str, body: &str| {
        let p = ws.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\", \"lib\"]\nresolver = \"2\"\n",
    );
    write(
        "lib/Cargo.toml",
        "[package]\nname = \"stash-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write("lib/src/lib.rs", "pub fn n() -> u32 { 7 }\n");
    write(
        "app/Cargo.toml",
        "[package]\nname = \"stash-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nstash-lib = { path = \"../lib\" }\n",
    );
    write(
        "app/src/main.rs",
        "fn main() { println!(\"{}\", stash_lib::n()); }\n",
    );

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = tmp.join("target");
    let build = || {
        let out = Command::new(&cargo)
            .args(["build", "--offline", "-p", "stash-app", "--target-dir"])
            .arg(&target)
            .current_dir(&ws)
            .env_remove("RUSTC_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .output()
            .expect("cargo runs");
        assert!(
            out.status.success(),
            "cargo build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    build();
    let profile = target.join("debug");
    let stash = tmp.join("stash");
    let cargo_dir = Path::new(&cargo)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let path = format!("{cargo_dir}:{}", std::env::var("PATH").unwrap_or_default());
    let (profile_s, stash_s) = (profile.to_str().unwrap(), stash.to_str().unwrap());
    run_script(&["prune", profile_s, stash_s], Some(&path), &ws);
    run_script(&["unstash", profile_s, stash_s], Some(&path), &ws);

    let second = build();
    assert!(
        !second.contains("Compiling"),
        "cargo recompiled after unstash, so CI would compile the workspace twice:\n{second}"
    );
}
