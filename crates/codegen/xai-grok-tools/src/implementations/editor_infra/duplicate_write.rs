//! Reject writes whose content is substantially identical to another repo file.
//!
//! The failure mode this catches: a model retypes an entire existing file
//! through `write` / empty-`old_string` `search_replace` instead of
//! `git mv` / `cp` plus a minimal edit.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Files smaller than this are skipped: boilerplate one-liners collide.
pub const MIN_BYTES: usize = 64;

/// Cap on how many candidate files we open. Fail open (allow the write)
/// rather than stalling a huge repo; the description ban still applies.
const MAX_CANDIDATES: usize = 4_000;

/// Skip files larger than this when scanning (the write itself can be any size).
const MAX_CANDIDATE_BYTES: u64 = 8 * 1024 * 1024;

/// Walk `start` (or its parent if it is a file) looking for `.git`.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Normalize for "substantially identical": CRLF → LF, strip trailing
/// whitespace per line. Catches a retype that only drifted line endings.
pub fn normalize_for_identity(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if !text.ends_with('\n') && !text.ends_with('\r') && out.ends_with('\n') {
        out.pop();
    }
    out.into_bytes()
}

fn identity_matches(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    normalize_for_identity(a) == normalize_for_identity(b)
}

fn git_ls_files(git_root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(git_root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| git_root.join(String::from_utf8_lossy(p).as_ref()))
        .collect()
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// If `content` is substantially identical to another non-ignored repo file
/// (not `dest`), return a loud error naming that file and instructing
/// `git mv` / `cp`.
pub fn reject_duplicate_write(cwd: &Path, dest: &Path, content: &[u8]) -> Option<String> {
    if content.len() < MIN_BYTES {
        return None;
    }
    let git_root = find_git_root(cwd).or_else(|| find_git_root(dest))?;
    let candidates = git_ls_files(&git_root);
    let dest_len = content.len() as u64;
    let mut scanned = 0usize;
    for candidate in candidates {
        if scanned >= MAX_CANDIDATES {
            break;
        }
        if same_path(&candidate, dest) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_CANDIDATE_BYTES {
            continue;
        }
        // Exact-size first; normalized identity can differ by CRLF / trailing ws.
        let diff = meta.len().abs_diff(dest_len);
        let size_ok = diff == 0 || diff <= dest_len / 4 || diff <= 256;
        if !size_ok {
            continue;
        }
        scanned += 1;
        let Ok(existing) = std::fs::read(&candidate) else {
            continue;
        };
        if identity_matches(&existing, content) {
            let display = candidate
                .strip_prefix(&git_root)
                .unwrap_or(&candidate)
                .display()
                .to_string();
            return Some(format!(
                "Refusing to write {}: content is substantially identical to existing file `{display}`. \
                 Do not retype or rewrite a file to copy or relocate it. Use `git mv` (tracked) or `cp` \
                 (or the move_file/copy_file tools), then a minimal edit for package/flag/name changes.",
                dest.display()
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
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
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .status();
        let _ = Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir)
            .status();
    }

    const BODY: &str = "fn main() {\n    println!(\"hello from the original crate\");\n    let x = 1;\n    let y = 2;\n    let z = x + y;\n    println!(\"{z}\");\n}\n";

    #[test]
    fn tiny_writes_are_not_scanned() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("a.txt"), "hi\n").unwrap();
        assert!(reject_duplicate_write(tmp.path(), &tmp.path().join("b.txt"), b"hi\n").is_none());
    }

    #[test]
    fn exact_copy_of_tracked_file_is_rejected() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let src = tmp.path().join("src/lib.rs");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, BODY).unwrap();
        assert!(
            Command::new("git")
                .args(["add", "src/lib.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let err = reject_duplicate_write(tmp.path(), &tmp.path().join("cli/lib.rs"), BODY.as_bytes())
            .expect("must reject");
        assert!(err.contains("src/lib.rs"), "{err}");
        assert!(err.contains("git mv") || err.contains("copy_file"), "{err}");
    }

    #[test]
    fn crlf_normalized_copy_is_rejected() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("orig.rs"), BODY).unwrap();
        assert!(
            Command::new("git")
                .args(["add", "orig.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let crlf = BODY.replace('\n', "\r\n");
        let err = reject_duplicate_write(tmp.path(), &tmp.path().join("copy.rs"), crlf.as_bytes())
            .expect("must reject crlf twin");
        assert!(err.contains("orig.rs"), "{err}");
    }

    #[test]
    fn unique_content_is_allowed() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        fs::write(tmp.path().join("orig.rs"), BODY).unwrap();
        assert!(
            Command::new("git")
                .args(["add", "orig.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let unique = BODY.replace("original crate", "brand new crate with different words");
        assert!(
            reject_duplicate_write(tmp.path(), &tmp.path().join("new.rs"), unique.as_bytes())
                .is_none()
        );
    }

    #[test]
    fn writing_the_same_path_is_not_a_duplicate_of_itself() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let src = tmp.path().join("orig.rs");
        fs::write(&src, BODY).unwrap();
        assert!(
            Command::new("git")
                .args(["add", "orig.rs"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(reject_duplicate_write(tmp.path(), &src, BODY.as_bytes()).is_none());
    }

    #[test]
    fn normalize_strips_crlf_and_trailing_ws() {
        assert_eq!(
            normalize_for_identity(b"a  \r\nb\r\n"),
            normalize_for_identity(b"a\nb\n")
        );
    }
}
