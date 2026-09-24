//! Branch divergence and uncommitted line counts, shown beside the branch.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchStats {
    pub ahead_behind: Option<(usize, usize)>,
    pub base: Option<String>,
    pub insertions: usize,
    pub deletions: usize,
}

type CacheEntry = (Option<BranchStats>, Instant);
static CACHE: LazyLock<Mutex<HashMap<PathBuf, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const REFRESH_TTL: Duration = Duration::from_secs(5);
const CACHE_CAP: usize = 64;

/// Cached stats for `cwd`. Starts an off-thread refresh when the entry is
/// missing or older than [`REFRESH_TTL`]. Never blocks.
pub fn branch_stats_lazy(cwd: &Path) -> Option<BranchStats> {
    let mut cache = CACHE.lock().ok()?;
    let (cached, needs_refresh) = match cache.get(cwd) {
        Some((stats, ts)) => (stats.clone(), ts.elapsed() >= REFRESH_TTL),
        None => (None, true),
    };
    if needs_refresh {
        insert(
            &mut cache,
            cwd.to_path_buf(),
            (cached.clone(), Instant::now()),
        );
        drop(cache);
        spawn_refresh(cwd.to_path_buf());
    }
    cached
}

fn spawn_refresh(cwd: PathBuf) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn_blocking(move || {
        // libgit2 is C code. A panic in its bindings must not take down the pager.
        let stats =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compute_branch_stats(&cwd)));
        let stats = match stats {
            Ok(Ok(stats)) => stats,
            Ok(Err(e)) => {
                tracing::warn!(cwd = %cwd.display(), error = %e, "branch stats refresh failed");
                None
            }
            Err(_) => {
                tracing::error!(cwd = %cwd.display(), "branch stats refresh panicked");
                None
            }
        };
        if let Ok(mut cache) = CACHE.lock() {
            insert(&mut cache, cwd, (stats, Instant::now()));
        }
    });
}

fn insert(cache: &mut HashMap<PathBuf, CacheEntry>, key: PathBuf, entry: CacheEntry) {
    if cache.len() >= CACHE_CAP
        && !cache.contains_key(&key)
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (_, ts))| *ts)
            .map(|(k, _)| k.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(key, entry);
}

/// Compute the stats for the repository that contains `cwd`. `Ok(None)` means
/// `cwd` is not in a repository or HEAD has no commit yet.
pub fn compute_branch_stats(cwd: &Path) -> Result<Option<BranchStats>, git2::Error> {
    let Ok(repo) = git2::Repository::discover(cwd) else {
        return Ok(None);
    };
    let Ok(head) = repo.head() else {
        return Ok(None);
    };
    let Ok(head_commit) = head.peel_to_commit() else {
        return Ok(None);
    };

    let mut stats = BranchStats::default();
    let head_ref = head.is_branch().then(|| head.name().ok()).flatten();
    if let Some((base_ref, base_oid)) = resolve_base(&repo, head_ref) {
        stats.ahead_behind = Some(repo.graph_ahead_behind(head_commit.id(), base_oid)?);
        stats.base = Some(short_ref_name(&base_ref).to_string());
    }

    if !repo.is_bare() {
        let mut opts = git2::DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let diff =
            repo.diff_tree_to_workdir_with_index(Some(&head_commit.tree()?), Some(&mut opts))?;
        let diff_stats = diff.stats()?;
        stats.insertions = diff_stats.insertions();
        stats.deletions = diff_stats.deletions();
    }
    Ok(Some(stats))
}

/// The branch HEAD was created from, as a full ref name and its tip. The
/// current branch is never its own base.
fn resolve_base(repo: &git2::Repository, head_ref: Option<&str>) -> Option<(String, git2::Oid)> {
    let usable = |name: &str| -> Option<(String, git2::Oid)> {
        if Some(name) == head_ref {
            return None;
        }
        let reference = repo.find_reference(name).ok()?.resolve().ok()?;
        let full = reference.name().ok()?.to_string();
        if Some(full.as_str()) == head_ref {
            return None;
        }
        Some((full, reference.target()?))
    };

    if let Some(head_ref) = head_ref {
        if let Some(from) = reflog_created_from(repo, head_ref)
            && let Some(found) = [format!("refs/heads/{from}"), format!("refs/remotes/{from}")]
                .iter()
                .find_map(|name| usable(name))
        {
            return Some(found);
        }
        if let Ok(upstream) = repo.branch_upstream_name(head_ref)
            && let Ok(upstream) = upstream.as_str()
            && !upstream_tracks_same_branch(upstream, head_ref)
            && let Some(found) = usable(upstream)
        {
            return Some(found);
        }
    }

    [
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
        "refs/remotes/origin/master",
        "refs/heads/main",
        "refs/heads/master",
    ]
    .iter()
    .find_map(|name| usable(name))
}

/// The start point that `git branch`, `checkout -b` and `worktree add -b`
/// record in the oldest reflog entry. `HEAD` names no branch, so it is `None`.
fn reflog_created_from(repo: &git2::Repository, head_ref: &str) -> Option<String> {
    let reflog = repo.reflog(head_ref).ok()?;
    let oldest = reflog.iter().last()?;
    let message = oldest.message().ok()??;
    let from = message.strip_prefix("branch: Created from ")?.trim();
    (!from.is_empty() && from != "HEAD").then(|| from.to_string())
}

/// Whether `upstream` (`refs/remotes/<remote>/<branch>`) is the remote copy of
/// `head_ref` (`refs/heads/<branch>`). A branch name can contain `/`, so only
/// the earliest segment after `refs/remotes/` is taken as the remote name.
fn upstream_tracks_same_branch(upstream: &str, head_ref: &str) -> bool {
    let Some(branch) = head_ref.strip_prefix("refs/heads/") else {
        return false;
    };
    match upstream.strip_prefix("refs/remotes/") {
        Some(rest) => rest
            .split_once('/')
            .is_some_and(|(_, remote_branch)| remote_branch == branch),
        None => upstream == head_ref,
    }
}

fn short_ref_name(full: &str) -> &str {
    full.strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/remotes/"))
        .unwrap_or(full)
}

/// Status-bar parts in the order `↑ahead ↓behind +ins -del`, without the zeros.
pub fn format_parts(stats: &BranchStats) -> Vec<(StatKind, String)> {
    let mut parts = Vec::new();
    if let Some((ahead, behind)) = stats.ahead_behind {
        if ahead > 0 {
            parts.push((StatKind::Ahead, format!("↑{ahead}")));
        }
        if behind > 0 {
            parts.push((StatKind::Behind, format!("↓{behind}")));
        }
    }
    if stats.insertions > 0 {
        parts.push((StatKind::Insertions, format!("+{}", stats.insertions)));
    }
    if stats.deletions > 0 {
        parts.push((StatKind::Deletions, format!("-{}", stats.deletions)));
    }
    parts
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatKind {
    Ahead,
    Behind,
    Insertions,
    Deletions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit_file(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
        git(dir, &["add", name]);
        git(dir, &["commit", "-qm", name]);
    }

    fn repo_on_master() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "master"]);
        commit_file(tmp.path(), "a.txt", "one\n");
        tmp
    }

    #[test]
    fn counts_ahead_behind_against_the_branch_it_was_created_from() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        git(dir, &["branch", "develop"]);
        git(dir, &["switch", "-q", "develop"]);
        commit_file(dir, "d.txt", "d\n");
        git(dir, &["switch", "-q", "-c", "feature", "develop"]);
        commit_file(dir, "f1.txt", "f\n");
        commit_file(dir, "f2.txt", "f\n");
        git(dir, &["switch", "-q", "develop"]);
        commit_file(dir, "d2.txt", "d\n");
        git(dir, &["switch", "-q", "feature"]);

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(
            stats.base.as_deref(),
            Some("develop"),
            "reflog start point wins over master"
        );
        assert_eq!(stats.ahead_behind, Some((2, 1)));
    }

    #[test]
    fn checkout_from_head_falls_back_to_the_default_branch() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        git(dir, &["switch", "-q", "-c", "topic"]);
        commit_file(dir, "t.txt", "t\n");

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(stats.base.as_deref(), Some("master"));
        assert_eq!(stats.ahead_behind, Some((1, 0)));
    }

    #[test]
    fn the_default_branch_itself_has_no_base_without_a_remote() {
        let tmp = repo_on_master();
        let stats = compute_branch_stats(tmp.path()).unwrap().unwrap();
        assert_eq!(stats.base, None);
        assert_eq!(stats.ahead_behind, None);
    }

    #[test]
    fn uncommitted_lines_cover_staged_unstaged_and_untracked() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        commit_file(dir, "b.txt", "keep\ndrop\n");
        std::fs::write(dir.join("b.txt"), "keep\n").unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(dir, &["add", "a.txt"]);
        std::fs::write(dir.join("new.txt"), "x\ny\nz\n").unwrap();

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!((stats.insertions, stats.deletions), (4, 1));
    }

    #[test]
    fn a_non_repo_has_no_stats() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(compute_branch_stats(tmp.path()).unwrap(), None);
    }

    #[test]
    fn upstream_of_the_same_branch_is_not_a_base() {
        assert!(upstream_tracks_same_branch(
            "refs/remotes/origin/claude/x",
            "refs/heads/claude/x"
        ));
        assert!(!upstream_tracks_same_branch(
            "refs/remotes/origin/master",
            "refs/heads/claude/x"
        ));
    }

    #[test]
    fn zero_counts_are_left_out() {
        let stats = BranchStats {
            ahead_behind: Some((3, 0)),
            base: Some("master".into()),
            insertions: 0,
            deletions: 7,
        };
        let text: Vec<String> = format_parts(&stats).into_iter().map(|(_, s)| s).collect();
        assert_eq!(text, ["↑3", "-7"]);
        assert!(format_parts(&BranchStats::default()).is_empty());
    }
}
