use std::path::{Path, PathBuf};
use std::process::Command;

/// Stamps the commit into this crate, which is where `version_with_commit()`
/// reads it. Every crate that reports a version links this one.
fn main() {
    if let Some(head) = git_head(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        println!("cargo:rerun-if-changed={}", head.display());
    }

    println!("cargo:rustc-env=BUILD_COMMIT={}", git(&["rev-parse", "HEAD"]));
    println!(
        "cargo:rustc-env=BUILD_COMMIT_SHORT={}",
        git(&["rev-parse", "--short", "HEAD"])
    );
}

/// A checkout with no git dir (a published tarball) reports `unknown`, which
/// `commit_github_url` renders as plain text rather than a dead link.
fn git(args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Walks up for `.git/HEAD`. This crate sits several directories below the
/// repository root, so a relative path from here breaks when the crate moves.
/// A worktree's `.git` is a file and has no HEAD beside it, so `is_file` on
/// HEAD itself is the test.
fn git_head(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .map(|dir| dir.join(".git").join("HEAD"))
        .find(|head| head.is_file())
}
