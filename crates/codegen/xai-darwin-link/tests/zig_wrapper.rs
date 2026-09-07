//! Checks what `ci/zig-cc` forwards to zig.
//!
//! cc-rs adds `--target=arm64-apple-macosx`, and zig answers that with
//! "unknown architecture: 'arm64'". Every C and assembly file in ring and
//! aws-lc failed on it, so the wrapper drops cc-rs's target selection. A fake
//! `zig` on PATH records the argument list the wrapper produces.

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

/// A `zig` that writes its arguments to `record` and exits.
fn fake_zig(dir: &Path, record: &Path) -> PathBuf {
    let zig = dir.join("zig");
    std::fs::write(
        &zig,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> '{}'; done\n",
            record.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&zig).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&zig, perms).unwrap();
    zig
}

#[test]
fn the_wrapper_replaces_every_target_cc_rs_chose() {
    let dir = std::env::temp_dir().join(format!(
        "zig-wrapper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let record = dir.join("args");
    fake_zig(&dir, &record);

    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let status = Command::new(repo_root().join("ci/zig-cc"))
        .args([
            "--target=arm64-apple-macosx",
            "-O3",
            "-target",
            "arm64-apple-macosx",
            "-c",
            "sha256-armv8-ios64.S",
        ])
        .env("PATH", path)
        .env("SDKROOT", "/sdk")
        .status()
        .expect("the wrapper must run");
    assert!(status.success(), "the wrapper must exit cleanly");

    let got: Vec<String> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        got,
        vec![
            "cc",
            "-target",
            "aarch64-macos",
            "-isysroot",
            "/sdk",
            "-iframework",
            "/sdk/System/Library/Frameworks",
            "-O3",
            "-c",
            "sha256-armv8-ios64.S",
        ],
        "both spellings of cc-rs's target must be gone, and zig's own plus the SDK must be there"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
