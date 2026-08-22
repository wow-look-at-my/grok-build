//! Bash PreToolUse-style guards for git-repo file deletion and history rewrite.
//!
//! Policy (also documented in the user-guide and system prompt):
//! - Never `rm` a non-ignored file in a git repo (tracked or untracked).
//!   Commit first, then `git rm`.
//! - Do not hide a deletion with `git commit --amend` after `git rm`,
//!   `git reset --hard`, `git filter-branch` / `filter-repo`, or a force-push
//!   of rewritten history.
use crate::implementations::editor_infra::duplicate_write::find_git_root;
use std::path::{Path, PathBuf};
use std::process::Command;

const RM_MESSAGE: &str = "Refusing to `rm` a non-ignored file in a git repository. \
Commit the file first, then `git rm` so the deletion is a real commit. \
Do not hide it with `git commit --amend`, `git reset --hard`, `git filter-branch`, \
or a force-push of rewritten history. Gitignored / scratch files may still be `rm`'d.";

const RESET_HARD_MESSAGE: &str = "Refusing `git reset --hard`. That discards history and work. \
If you meant to delete a file, commit it (if needed) and `git rm` in a new commit.";

const FILTER_BRANCH_MESSAGE: &str = "Refusing `git filter-branch` / `git filter-repo`. \
History rewrite is banned as a way to hide a deletion. Commit, then `git rm` in a new commit.";

const AMEND_HIDE_MESSAGE: &str = "Refusing `git commit --amend`: it would drop just-committed \
file(s) from history after a `git rm`. Leave the commit that added them, then `git rm` in a NEW \
commit. Do not amend, reset --hard, filter-branch, or force-push to hide a deletion.";

/// If `command` violates the git-file / history-rewrite policy, return a
/// model-facing rejection. `None` means the command may run.
pub fn bash_command_violation(command: &str, cwd: &Path) -> Option<String> {
    for stmt in split_statements(command) {
        let words = tokenize(stmt);
        if words.is_empty() {
            continue;
        }
        if let Some(msg) = history_rewrite_violation(&words, cwd) {
            return Some(msg);
        }
        if let Some(msg) = untracked_rm_violation(&words, cwd) {
            return Some(msg);
        }
    }
    None
}

fn history_rewrite_violation(words: &[String], cwd: &Path) -> Option<String> {
    let git = git_verb_index(words)?;
    let verb = words[git].as_str();
    let rest = &words[git + 1..];
    match verb {
        "reset" if flag_present(rest, &["--hard"]) => Some(RESET_HARD_MESSAGE.to_owned()),
        "filter-branch" | "filter-repo" => Some(FILTER_BRANCH_MESSAGE.to_owned()),
        "commit" if flag_present(rest, &["--amend"]) => amend_hides_just_committed(cwd),
        _ => None,
    }
}

fn amend_hides_just_committed(cwd: &Path) -> Option<String> {
    let git_root = find_git_root(cwd)?;
    let hidden = files_amend_would_drop(&git_root);
    if hidden.is_empty() {
        None
    } else {
        Some(format!("{AMEND_HIDE_MESSAGE} Files: {}", hidden.join(", ")))
    }
}

/// Files deleted in the index (or worktree, since `git add -A` / `git rm`
/// may already be staged) that were added by HEAD. Used by bash and
/// `session/git.rs`.
pub fn files_amend_would_drop(git_root: &Path) -> Vec<String> {
    let deleted = git_stdout(
        git_root,
        &["diff", "HEAD", "--name-only", "--diff-filter=D"],
    );
    if deleted.is_empty() {
        return Vec::new();
    }
    let added = git_stdout(
        git_root,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "--diff-filter=A",
            "-r",
            "HEAD",
        ],
    );
    let added: std::collections::HashSet<&str> = added
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    deleted
        .into_iter()
        .filter(|f| !f.is_empty() && added.contains(f.as_str()))
        .collect()
}

fn untracked_rm_violation(words: &[String], cwd: &Path) -> Option<String> {
    let Some(idx) = rm_command_index(words) else {
        return None;
    };
    // `git rm` is the allowed path.
    if idx > 0 && is_git_binary(&words[idx - 1]) {
        return None;
    }
    let operands = rm_operands(&words[idx + 1..]);
    if operands.is_empty() {
        return None;
    }
    for raw in operands {
        let path = resolve_operand(cwd, raw);
        if should_block_rm(&path, cwd) {
            return Some(format!("{RM_MESSAGE} Path: {}", path.display()));
        }
    }
    None
}

fn should_block_rm(path: &Path, cwd: &Path) -> bool {
    let Some(git_root) = find_git_root(path).or_else(|| find_git_root(cwd)) else {
        return false;
    };
    if !path_is_inside(path, &git_root) {
        return false;
    }
    !git_is_ignored(&git_root, path)
}

fn path_is_inside(path: &Path, root: &Path) -> bool {
    let canon_path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let canon_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    canon_path.starts_with(&canon_root)
}

fn git_is_ignored(git_root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(git_root).unwrap_or(path);
    Command::new("git")
        .args([
            "--no-optional-locks",
            "check-ignore",
            "-q",
            "--",
            &rel.to_string_lossy(),
        ])
        .current_dir(git_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn resolve_operand(cwd: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { cwd.join(p) }
}

fn rm_command_index(words: &[String]) -> Option<usize> {
    words.iter().position(|w| {
        let base = w.rsplit(['/', '\\']).next().unwrap_or(w);
        base == "rm" || base == "unlink"
    })
}

fn is_git_binary(word: &str) -> bool {
    word.rsplit(['/', '\\']).next().unwrap_or(word) == "git"
}

fn git_verb_index(words: &[String]) -> Option<usize> {
    let git = words.iter().position(|w| is_git_binary(w))?;
    words[git + 1..]
        .iter()
        .position(|w| !w.starts_with('-') && w != "-C")
        .map(|i| git + 1 + i)
}

fn flag_present(words: &[String], flags: &[&str]) -> bool {
    words.iter().any(|w| {
        flags
            .iter()
            .any(|f| w == f || w.starts_with(&format!("{f}=")))
    })
}

fn rm_operands(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut end_of_flags = false;
    for a in args {
        if !end_of_flags && a == "--" {
            end_of_flags = true;
            continue;
        }
        if !end_of_flags && a.starts_with('-') {
            continue;
        }
        out.push(a.as_str());
    }
    out
}

fn git_stdout(git_root: &Path, args: &[&str]) -> Vec<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(git_root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    // `git diff` exits 1 when there *are* differences; that is still stdout we want.
    let code = output.status.code().unwrap_or(1);
    if code != 0 && code != 1 {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split on `&&` `||` `;` `|` and newlines that are not inside quotes.
fn split_statements(command: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = command.as_bytes();
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q && (q != b'\\') {
                quote = None;
            }
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' {
            quote = Some(b);
            i += 1;
            continue;
        }
        let rest = &command[i..];
        let sep_len = if rest.starts_with("&&") || rest.starts_with("||") {
            2
        } else if rest.starts_with(';') || rest.starts_with('|') || rest.starts_with('\n') {
            1
        } else {
            0
        };
        if sep_len > 0 {
            let stmt = command[start..i].trim();
            if !stmt.is_empty() {
                out.push(stmt);
            }
            i += sep_len;
            start = i;
            continue;
        }
        i += 1;
    }
    let stmt = command[start..].trim();
    if !stmt.is_empty() {
        out.push(stmt);
    }
    out
}

fn tokenize(stmt: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut chars = stmt.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    // Drop VAR=value prefixes.
    while words.first().is_some_and(|w| {
        w.contains('=') && !w.starts_with('-') && !w.starts_with('/') && !w.starts_with('.')
    }) {
        let name = words[0].split('=').next().unwrap_or("");
        if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
            words.remove(0);
        } else {
            break;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo(dir: &Path) {
        assert!(
            Command::new("git")
                .args(["init"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        );
        let _ = Command::new("git")
            .args(["config", "user.email", "t@e.com"])
            .current_dir(dir)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(dir)
            .status();
    }

    #[test]
    fn blocks_rm_of_untracked_repo_file() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("secret.rs"), "fn x() {}\n").unwrap();
        let err = bash_command_violation("rm secret.rs", tmp.path()).expect("must block");
        assert!(err.contains("git rm"), "{err}");
        assert!(err.contains("secret.rs"), "{err}");
    }

    #[test]
    fn allows_rm_of_gitignored_file() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join(".gitignore"), "scratch/\n").unwrap();
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("scratch/tmp.txt"), "x\n").unwrap();
        assert!(
            bash_command_violation("rm scratch/tmp.txt", tmp.path()).is_none(),
            "gitignored rm must be allowed"
        );
    }

    #[test]
    fn allows_git_rm() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("a.rs"), "fn a() {}\n").unwrap();
        assert!(bash_command_violation("git rm a.rs", tmp.path()).is_none());
    }

    #[test]
    fn allows_rm_outside_repo() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("x.txt"), "x\n").unwrap();
        assert!(bash_command_violation("rm x.txt", tmp.path()).is_none());
    }

    #[test]
    fn blocks_reset_hard() {
        let tmp = TempDir::new().unwrap();
        let err = bash_command_violation("git reset --hard HEAD", tmp.path()).expect("block");
        assert!(err.contains("reset --hard"), "{err}");
    }

    #[test]
    fn blocks_filter_branch() {
        let tmp = TempDir::new().unwrap();
        let err = bash_command_violation("git filter-branch -- --all", tmp.path()).expect("block");
        assert!(err.contains("filter-branch"), "{err}");
    }

    #[test]
    fn blocks_amend_after_git_rm_of_just_committed_file() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("hide.rs"), "fn hide() {}\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "hide.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "add hide"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["rm", "hide.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let err = bash_command_violation("git commit --amend -m 'nope'", tmp.path())
            .expect("must block hide-via-amend");
        assert!(err.contains("hide.rs"), "{err}");
        assert!(err.contains("amend"), "{err}");
    }

    #[test]
    fn allows_message_only_amend() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("keep.rs"), "fn keep() {}\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "keep.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-m", "add"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            bash_command_violation("git commit --amend -m 'better message'", tmp.path()).is_none()
        );
    }

    #[test]
    fn split_respects_quotes() {
        let parts = split_statements("echo 'a && b' && rm foo");
        assert_eq!(parts, vec!["echo 'a && b'", "rm foo"]);
    }
}
