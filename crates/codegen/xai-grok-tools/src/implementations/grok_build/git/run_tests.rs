// A repo with commits, a branch, a tag, a stash.

use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn setup_git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@e.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo with commits, a branch, a tag, a stash. A note and a file whose
/// stat no longer matches the index, so `git diff` wants to refresh it.
fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    setup_git(d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    std::fs::write(d.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(d.join("big.txt"), "x".repeat(MAX_STDOUT_BYTES * 2)).unwrap();
    setup_git(d, &["add", "."]);
    setup_git(d, &["commit", "-qm", "first"]);
    setup_git(d, &["tag", "v1"]);
    std::fs::write(d.join("a.txt"), "two\n").unwrap();
    setup_git(d, &["commit", "-qam", "second"]);
    setup_git(d, &["branch", "feature"]);
    setup_git(d, &["notes", "add", "-m", "a note"]);
    std::fs::write(d.join("a.txt"), "stashed\n").unwrap();
    setup_git(d, &["stash", "push", "-q"]);
    // A new mtime with the same content leaves the index stat stale.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(d.join("a.txt"), "two\n").unwrap();
    tmp
}

/// Every file under `root`, with its size, mtime and content.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime, Vec<u8>)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path.clone());
            }
            let content = if meta.is_file() {
                std::fs::read(&path).unwrap()
            } else {
                Vec::new()
            };
            out.insert(path, (meta.len(), meta.modified().unwrap(), content));
        }
    }
    out
}

async fn run(dir: &Path, line: &str) -> String {
    let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
    let approved = policy::approve(&args).unwrap_or_else(|e| panic!("`git {line}` refused: {e}"));
    run_git(dir, &approved.args)
        .await
        .unwrap_or_else(|e| panic!("`git {line}` failed to run: {e}"))
        .text
}

/// The control for the next test: a plain `git diff` writes `.git/index`, so
/// the snapshot sees a write when one happens.
#[test]
fn plain_git_diff_writes_the_index_of_this_repo() {
    let tmp = repo();
    let before = snapshot(tmp.path());
    let out = std::process::Command::new("git")
        .args(["diff"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_ne!(
        before,
        snapshot(tmp.path()),
        "plain git diff must refresh .git/index here"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn every_approved_form_runs_and_leaves_the_repo_unchanged() {
    let tmp = repo();
    let d = tmp.path();
    let before = snapshot(d);
    for line in [
        "diff",
        "diff --stat HEAD~1",
        "diff --cached",
        "describe --tags --dirty",
        "status",
        "status --porcelain",
        "log --oneline",
        "log -p -n 1",
        "show --stat HEAD",
        "diff-tree -r HEAD",
        "diff-index HEAD",
        "diff-files",
        "range-diff HEAD~1..HEAD HEAD~1..HEAD",
        "blame a.txt",
        "annotate a.txt",
        "grep -n two",
        "ls-files",
        "ls-tree HEAD",
        "cat-file -p HEAD",
        "rev-parse HEAD",
        "rev-list --count HEAD",
        "name-rev HEAD",
        "merge-base HEAD feature",
        "cherry feature",
        "for-each-ref",
        "show-ref",
        "show-branch",
        "shortlog -sn HEAD",
        "count-objects -v",
        "check-ignore -v build.log",
        "check-attr -a a.txt",
        "check-ref-format refs/heads/x",
        "version",
        "branch -av",
        "branch --list ma*",
        "branch --show-current",
        "tag",
        "tag -l v*",
        "config --list --local",
        "config --get-regexp ^core\\.",
        "remote -v",
        "stash list",
        "stash show -p",
        "worktree list",
        "notes list",
        "notes show HEAD",
        "reflog",
        "reflog show -n 2",
        "symbolic-ref HEAD",
        "submodule status",
    ] {
        let text = run(d, line).await;
        assert!(
            !text.starts_with("git exited with"),
            "`git {line}` failed:\n{text}"
        );
    }
    assert_eq!(
        before,
        snapshot(d),
        "a read-only git command changed the repo"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn the_diff_shows_no_change_for_a_stat_only_change() {
    let tmp = repo();
    assert_eq!(run(tmp.path(), "diff").await, "(no output)");
    assert_eq!(
        run(tmp.path(), "diff --stat HEAD~1").await.lines().count(),
        2
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn a_failing_command_reports_its_status_and_stderr() {
    let tmp = repo();
    let text = run(tmp.path(), "show no-such-rev").await;
    assert!(text.starts_with("git exited with"), "{text}");
    assert!(text.contains("no-such-rev"), "{text}");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn output_past_the_cap_is_cut_and_says_so() {
    let tmp = repo();
    let text = run(tmp.path(), "cat-file -p HEAD:big.txt").await;
    assert!(
        text.contains("[output cut at"),
        "{}",
        crate::util::truncate::tail_bytes(&text, 200)
    );
    assert!(text.len() < MAX_STDOUT_BYTES + 300);
}

#[test]
fn the_description_names_every_subcommand_the_policy_runs() {
    for verb in [policy::DIFF_FAMILY, policy::READ_ONLY, policy::RESTRICTED].concat() {
        assert!(
            DESCRIPTION.contains(&format!(" {verb}")),
            "the tool description must name `{verb}`"
        );
    }
}
