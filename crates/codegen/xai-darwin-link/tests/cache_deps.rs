//! Drives `ci/cache-deps.sh` over synthetic target directories and a stub `cargo`.

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

fn write_exe(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A `cargo` whose `metadata` answer is whatever `metadata.json` in `dir` holds, and a
/// fixed `rustc`, so each check states its own workspace.
fn stub_tools(dir: &Path, metadata: &str) -> String {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(dir.join("metadata.json"), metadata).unwrap();
    write_exe(
        &bin.join("cargo"),
        &format!("#!/bin/sh\ncat '{}'\n", dir.join("metadata.json").display()),
    );
    write_exe(
        &bin.join("rustc"),
        "#!/bin/sh\necho 'rustc 1.94.1 (stub)'\n",
    );
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// `stub_tools` for a caller that sets up its own PATH.
fn stub_cargo(dir: &Path, metadata: &str) -> PathBuf {
    stub_tools(dir, metadata);
    dir.join("bin")
}

fn touch(path: PathBuf) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
}

fn fresh_dir(name: &str) -> PathBuf {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    tmp
}

fn run(cwd: &Path, path: &str, args: &[&str]) -> std::process::Output {
    let out = Command::new(repo_root().join("ci/cache-deps.sh"))
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .env_remove("RUSTFLAGS")
        .output()
        .expect("cache-deps.sh runs");
    assert!(
        out.status.success(),
        "cache-deps.sh {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

const H_PAGER: &str = "aaaaaaaaaaaaaaaa";
const H_SHELL: &str = "cccccccccccccccc";
const H_HARNESS: &str = "dddddddddddddddd";
const H_SMOKE: &str = "bbbbbbbbbbbbbbbb";
const H_SERDE: &str = "1111111111111111";
const H_TOKIO: &str = "2222222222222222";
const H_PAGERISH: &str = "3333333333333333";
const H_AWS_LC: &str = "4444444444444444";
const H_OLD: &str = "9999999999999999";

const MEMBERS: &str = r#"{"packages":[
 {"name":"xai-grok-pager","targets":[{"name":"xai-grok-pager"},{"name":"pty_e2e_smoke"}]},
 {"name":"xai-grok-shell","targets":[{"name":"xai-grok-shell"}]},
 {"name":"xai-grok-pager-pty-harness","targets":[{"name":"xai-grok-pager-pty-harness"}]}]}"#;

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn prune_keeps_live_registry_artifacts_stashes_the_workspace_and_drops_the_stale() {
    let tmp = fresh_dir("cache-deps-prune");
    let profile = tmp.join("target/debug");

    let workspace = [
        format!("deps/libxai_grok_pager-{H_PAGER}.rlib"),
        format!("deps/xai_grok_pager-{H_PAGER}.d"),
        format!("deps/libxai_grok_shell-{H_SHELL}.rlib"),
        format!("deps/libxai_grok_pager_pty_harness-{H_HARNESS}.rlib"),
        // Stemmed by TARGET name, with no package name in it anywhere.
        format!("deps/pty_e2e_smoke-{H_SMOKE}"),
        format!(".fingerprint/xai-grok-pager-{H_PAGER}/lib-xai_grok_pager"),
        format!(".fingerprint/xai-grok-pager-pty-harness-{H_HARNESS}/lib"),
        format!("build/xai-grok-shell-{H_SHELL}/output"),
    ];
    // `xai_grok_pagerish` is the near miss: a bare prefix match takes it and must not.
    let registry = [
        format!("deps/libserde-{H_SERDE}.rlib"),
        format!("deps/libtokio-{H_TOKIO}.rlib"),
        format!("deps/libxai_grok_pagerish-{H_PAGERISH}.rlib"),
        format!(".fingerprint/serde-{H_SERDE}/lib-serde"),
        format!("build/aws-lc-sys-{H_AWS_LC}/out/lib.a"),
    ];
    // Left by an older dependency set that a fallback restore brought back.
    let stale = [
        format!("deps/libserde-{H_OLD}.rlib"),
        format!(".fingerprint/serde-{H_OLD}/lib-serde"),
        format!("build/aws-lc-sys-{H_OLD}/output"),
    ];
    for rel in workspace.iter().chain(&registry).chain(&stale) {
        touch(profile.join(rel));
    }

    // What cargo reported for this build: every unit but the stale ones.
    let p = profile.display();
    let units: String = [
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libxai_grok_pager-{H_PAGER}.rlib"]}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libxai_grok_shell-{H_SHELL}.rlib"]}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libxai_grok_pager_pty_harness-{H_HARNESS}.rlib"]}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/pty_e2e_smoke-{H_SMOKE}"],"executable":"{p}/deps/pty_e2e_smoke-{H_SMOKE}"}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libserde-{H_SERDE}.rlib"]}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libtokio-{H_TOKIO}.rlib"]}}"#),
        format!(r#"{{"reason":"compiler-artifact","filenames":["{p}/deps/libxai_grok_pagerish-{H_PAGERISH}.rlib"]}}"#),
        format!(r#"{{"reason":"build-script-executed","out_dir":"{p}/build/aws-lc-sys-{H_AWS_LC}/out"}}"#),
    ]
    .join("\n");
    fs::write(tmp.join("units.json"), units).unwrap();

    let path = stub_tools(&tmp, MEMBERS);
    let stash = tmp.join("stash");
    run(
        &tmp,
        &path,
        &[
            "prune",
            profile.to_str().unwrap(),
            stash.to_str().unwrap(),
            tmp.join("units.json").to_str().unwrap(),
        ],
    );

    for rel in &workspace {
        assert!(
            !profile.join(rel).exists(),
            "{rel} survived the prune, so the cache entry would carry a workspace artifact"
        );
    }
    for rel in &registry {
        assert!(
            profile.join(rel).exists(),
            "{rel} was pruned, so the cache entry loses a registry artifact it exists to hold"
        );
    }
    for rel in &stale {
        assert!(
            !profile.join(rel).exists(),
            "{rel} survived the prune, so a fallback restore grows the entry with every dependency change"
        );
    }

    run(
        &tmp,
        &path,
        &[
            "unstash",
            profile.to_str().unwrap(),
            stash.to_str().unwrap(),
        ],
    );
    for rel in &workspace {
        assert!(
            profile.join(rel).exists(),
            "{rel} did not come back from the stash, so the build after the cache step compiles the workspace again"
        );
    }
    assert!(!stash.exists(), "unstash left the stash directory behind");
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn prune_refuses_a_units_file_that_names_no_unit() {
    let tmp = fresh_dir("cache-deps-empty-units");
    let profile = tmp.join("target/debug");
    touch(profile.join(format!("deps/libserde-{H_SERDE}.rlib")));
    fs::write(tmp.join("units.json"), r#"{"reason":"build-finished"}"#).unwrap();
    let path = stub_tools(&tmp, MEMBERS);

    let out = Command::new(repo_root().join("ci/cache-deps.sh"))
        .args([
            "prune",
            profile.to_str().unwrap(),
            tmp.join("stash").to_str().unwrap(),
            tmp.join("units.json").to_str().unwrap(),
        ])
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a prune with no live set would delete every registry artifact"
    );
    assert!(
        profile
            .join(format!("deps/libserde-{H_SERDE}.rlib"))
            .exists()
    );
}

const RESOLVED: &str = r#"{"workspace_members":["path+file:///w#app@0.1.0"],
 "resolve":{"nodes":[
  {"id":"path+file:///w#app@0.1.0","features":[],"deps":[{"pkg":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0"}]},
  {"id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0","features":["std"],"deps":[]}]}}"#;

fn key(cwd: &Path, path: &str) -> (String, String) {
    let out = run(cwd, path, &["key"]);
    (
        String::from_utf8(out.stdout).unwrap().trim().to_owned(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn key_ignores_manifest_edits_that_build_no_dependency_differently() {
    let tmp = fresh_dir("cache-deps-key");
    let path = stub_tools(&tmp, RESOLVED);
    let manifest = "[workspace]\nmembers = [\"app\"]\n\n[profile.release]\nlto = true\n";
    fs::write(tmp.join("Cargo.toml"), manifest).unwrap();
    fs::write(
        tmp.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.94.1\"\n",
    )
    .unwrap();

    let (base, inputs) = key(&tmp, &path);
    assert_eq!(base.len(), 40, "the key is a 40-hex digest, got {base:?}");
    for needed in [
        "serde@1.0.0",
        "\"std\"",
        "rustc 1.94.1",
        "lto = true",
        "channel = \"1.94.1\"",
    ] {
        assert!(
            inputs.contains(needed),
            "the logged key inputs omit {needed}:\n{inputs}"
        );
    }

    fs::write(
        tmp.join("Cargo.toml"),
        format!("{manifest}\n[workspace.lints.clippy]\nunwrap_used = \"deny\" # a comment\n"),
    )
    .unwrap();
    assert_eq!(key(&tmp, &path).0, base, "a lint table moved the key");

    fs::write(
        tmp.join("Cargo.toml"),
        manifest.replace("lto = true", "lto = false"),
    )
    .unwrap();
    assert_ne!(
        key(&tmp, &path).0,
        base,
        "a profile change left the key alone"
    );

    fs::write(tmp.join("Cargo.toml"), manifest).unwrap();
    fs::write(
        tmp.join("metadata.json"),
        RESOLVED.replace(r#"["std"]"#, r#"["derive","std"]"#),
    )
    .unwrap();
    assert_ne!(
        key(&tmp, &path).0,
        base,
        "a new feature on a registry crate left the key alone"
    );
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn check_fresh_names_registry_units_that_compiled_after_a_hit() {
    let tmp = fresh_dir("cache-deps-check-fresh");
    let path = stub_tools(&tmp, MEMBERS);
    fs::write(
        tmp.join("units.json"),
        [
            r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#jemalloc-sys@0.5.4","fresh":false}"#,
            r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0","fresh":true}"#,
            r#"{"reason":"compiler-artifact","package_id":"path+file:///w#app@0.1.0","fresh":false}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let out = String::from_utf8(
        run(
            &tmp,
            &path,
            &["check-fresh", tmp.join("units.json").to_str().unwrap()],
        )
        .stdout,
    )
    .unwrap();
    assert!(
        out.contains("::warning::"),
        "no warning for a compiled registry unit: {out}"
    );
    assert!(
        out.contains("jemalloc-sys"),
        "the warning does not name the unit: {out}"
    );
    assert!(
        !out.contains("serde"),
        "a fresh unit was reported as compiled: {out}"
    );
    assert!(
        !out.contains("app@"),
        "a workspace unit was reported: {out}"
    );
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
