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
        // A panic in the git read must not take down the pager.
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
pub fn compute_branch_stats(cwd: &Path) -> anyhow::Result<Option<BranchStats>> {
    let Ok(repo) = xai_grok_workspace::git_repo::discover(cwd) else {
        return Ok(None);
    };
    let Ok(head) = repo.head() else {
        return Ok(None);
    };
    let head_ref = head
        .referent_name()
        .map(|name| name.as_bstr().to_string())
        .filter(|name| name.starts_with("refs/heads/"));
    let Ok(head_commit) = repo.head_commit() else {
        return Ok(None);
    };

    let mut stats = BranchStats::default();
    if let Some((base_ref, base_oid)) = resolve_base(&repo, head_ref.as_deref()) {
        stats.ahead_behind = Some(
            xai_grok_workspace::git_repo::graph_ahead_behind(&repo, head_commit.id, base_oid)
                .ok_or_else(|| anyhow::anyhow!("cannot walk the commit graph"))?,
        );
        stats.base = Some(short_ref_name(&base_ref).to_string());
    }

    if !repo.is_bare() {
        let (insertions, deletions) = worktree_line_stats(&repo, head_commit.tree_id()?.detach())?;
        stats.insertions = insertions;
        stats.deletions = deletions;
    }
    Ok(Some(stats))
}

/// Lines added and removed between the `head_tree` and the work tree, untracked files included.
fn worktree_line_stats(
    repo: &gix::Repository,
    head_tree: gix::ObjectId,
) -> anyhow::Result<(usize, usize)> {
    use gix::diff::blob::ResourceKind;
    use gix::diff::blob::platform::prepare_diff::Operation;
    use gix::objs::tree::EntryKind;

    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("bare repository"))?
        .to_path_buf();
    let platform = repo
        .status(gix::progress::Discard)?
        .head_tree(head_tree)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .index_worktree_rewrites(None)
        .untracked_files(gix::status::UntrackedFiles::Files);
    let platform = xai_gix_status::with_budgeted_thread_limit(platform);
    let mut paths = std::collections::BTreeSet::new();
    for item in platform.into_iter(Vec::<gix::bstr::BString>::new())? {
        let item = item?;
        if let gix::status::Item::IndexWorktree(gix::status::index_worktree::Item::Modification {
            status: gix::status::plumbing::index_as_worktree::EntryStatus::NeedsUpdate(_),
            ..
        }) = &item
        {
            continue;
        }
        paths.insert(item.location().to_owned());
    }

    let tree = repo.find_tree(head_tree)?;
    let mut cache = repo.diff_resource_cache(
        gix::diff::blob::pipeline::Mode::ToGit,
        gix::diff::blob::pipeline::WorktreeRoots {
            old_root: None,
            new_root: Some(workdir.clone()),
        },
    )?;
    let null = gix::ObjectId::null(repo.object_hash());
    let (mut insertions, mut deletions) = (0usize, 0usize);
    for path in paths {
        let old = tree
            .lookup_entry_by_path(gix::path::from_bstr(path.as_ref()))?
            .map(|entry| (entry.object_id(), entry.mode().kind()));
        let (old_id, old_kind) = match old {
            Some((id, kind @ (EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link))) => {
                (id, kind)
            }
            Some(_) => continue,
            None => (null, EntryKind::Blob),
        };
        let new_kind =
            match std::fs::symlink_metadata(workdir.join(gix::path::from_bstr(path.as_ref()))) {
                Ok(meta) if meta.is_dir() => continue,
                Ok(meta) if meta.file_type().is_symlink() => EntryKind::Link,
                Ok(meta) if is_executable(&meta) => EntryKind::BlobExecutable,
                _ => EntryKind::Blob,
            };
        cache.set_resource(
            old_id,
            old_kind,
            path.as_ref(),
            ResourceKind::OldOrSource,
            &repo.objects,
        )?;
        cache.set_resource(
            null,
            new_kind,
            path.as_ref(),
            ResourceKind::NewOrDestination,
            &repo.objects,
        )?;
        let outcome = cache.prepare_diff()?;
        if let Operation::InternalDiff { algorithm } = outcome.operation {
            let diff = gix::diff::blob::Diff::compute(algorithm, &outcome.interned_input());
            insertions += diff.count_additions() as usize;
            deletions += diff.count_removals() as usize;
        }
    }
    Ok((insertions, deletions))
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// The base the status bar compares HEAD against. The checked-out branch's
/// configured upstream, else the branch it was created from, else the
/// repository's default branch. The current branch is never its own base, and
/// a detached HEAD has none.
fn resolve_base(repo: &gix::Repository, head_ref: Option<&str>) -> Option<(String, gix::ObjectId)> {
    let head_ref = head_ref?;
    let usable = |name: &str| -> Option<(String, gix::ObjectId)> {
        if name == head_ref {
            return None;
        }
        let (full, target) = resolve_ref(repo, name)?;
        if full == head_ref {
            return None;
        }
        Some((full, target))
    };

    if let Ok(head_name) = gix::refs::FullName::try_from(head_ref)
        && let Some(upstream) =
            xai_grok_workspace::git_repo::upstream_ref_name(repo, head_name.as_ref())
        && let Some(found) = usable(&upstream.as_bstr().to_string())
    {
        return Some(found);
    }
    if let Some(from) = reflog_created_from(repo, head_ref)
        && let Some(found) = [format!("refs/heads/{from}"), format!("refs/remotes/{from}")]
            .iter()
            .find_map(|name| usable(name))
    {
        return Some(found);
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
fn reflog_created_from(repo: &gix::Repository, head_ref: &str) -> Option<String> {
    let reference = repo.find_reference(head_ref).ok()?;
    let mut log = reference.log_iter();
    let oldest = log.all().ok()??.next()?.ok()?;
    let message = oldest.message.to_str().ok()?;
    let from = message.strip_prefix("branch: Created from ")?.trim();
    (!from.is_empty() && from != "HEAD").then(|| from.to_string())
}

/// Follow `name` through symbolic refs to a ref that names an object.
/// Returns that ref's full name and the object it names, without peeling a tag.
fn resolve_ref(repo: &gix::Repository, name: &str) -> Option<(String, gix::ObjectId)> {
    let mut name = name.to_owned();
    for _ in 0..5 {
        let reference = repo.find_reference(name.as_str()).ok()?;
        match reference.target() {
            gix::refs::TargetRef::Symbolic(target) => name = target.as_bstr().to_string(),
            gix::refs::TargetRef::Object(id) => {
                return Some((reference.name().as_bstr().to_string(), id.to_owned()));
            }
        }
    }
    None
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

    fn add_origin(dir: &Path) {
        git(dir, &["remote", "add", "origin", "."]);
    }

    /// Configure `branch` to track the existing remote-tracking ref `upstream`.
    fn track(dir: &Path, branch: &str, upstream: &str) {
        let flag = format!("--set-upstream-to={upstream}");
        git(dir, &["branch", flag.as_str(), branch]);
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
    fn a_detached_worktree_at_a_branch_tip_reports_no_divergence() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        // The default branch a detached HEAD used to be compared against.
        git(dir, &["update-ref", "refs/remotes/origin/master", "HEAD"]);
        git(
            dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/master",
            ],
        );
        git(dir, &["switch", "-q", "-c", "feature"]);
        commit_file(dir, "f.txt", "f\n");
        // A linked worktree detached at the branch tip is what a new Grok worktree looks like.
        let outside = tempfile::tempdir().unwrap();
        let wt = outside.path().join("wt");
        git(
            dir,
            &["worktree", "add", "--detach", wt.to_str().unwrap(), "HEAD"],
        );

        let stats = compute_branch_stats(&wt).unwrap().unwrap();
        assert_eq!(stats.base, None);
        assert_eq!(stats.ahead_behind, None);
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
    fn a_same_named_upstream_with_no_divergence_shows_no_ahead_behind() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        add_origin(dir);
        git(dir, &["switch", "-q", "-c", "feature"]);
        commit_file(dir, "f.txt", "f\n");
        git(dir, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        track(dir, "feature", "origin/feature");

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(stats.base.as_deref(), Some("origin/feature"));
        assert_eq!(stats.ahead_behind, Some((0, 0)));
        assert!(
            format_parts(&stats).is_empty(),
            "identical tips show no ahead/behind segment"
        );
    }

    #[test]
    fn a_same_named_upstream_with_newer_commits_reports_them_behind() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        add_origin(dir);
        git(dir, &["switch", "-q", "-c", "feature"]);
        commit_file(dir, "f.txt", "f\n");
        git(dir, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        track(dir, "feature", "origin/feature");
        git(dir, &["switch", "-q", "--detach"]);
        commit_file(dir, "u1.txt", "u1\n");
        commit_file(dir, "u2.txt", "u2\n");
        git(dir, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        git(dir, &["switch", "-q", "feature"]);

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(stats.base.as_deref(), Some("origin/feature"));
        assert_eq!(stats.ahead_behind, Some((0, 2)));
    }

    #[test]
    fn the_configured_upstream_wins_over_the_reflog_start_point() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        add_origin(dir);
        git(dir, &["branch", "develop"]);
        git(dir, &["switch", "-q", "develop"]);
        commit_file(dir, "d.txt", "d\n");
        git(dir, &["switch", "-q", "-c", "feature", "develop"]);
        commit_file(dir, "f.txt", "f\n");
        // The upstream is older than the reflog's develop start point, so the
        // chosen base is visible in the counts.
        git(
            dir,
            &["update-ref", "refs/remotes/origin/feature", "master"],
        );
        track(dir, "feature", "origin/feature");

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(stats.base.as_deref(), Some("origin/feature"));
        assert_eq!(stats.ahead_behind, Some((2, 0)));
    }

    #[test]
    fn a_tracked_upstream_on_another_branch_is_the_base() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        add_origin(dir);
        git(dir, &["update-ref", "refs/remotes/origin/main", "master"]);
        git(dir, &["switch", "-q", "-c", "tracked"]);
        commit_file(dir, "t.txt", "t\n");
        track(dir, "tracked", "origin/main");

        let stats = compute_branch_stats(dir).unwrap().unwrap();
        assert_eq!(stats.base.as_deref(), Some("origin/main"));
        assert_eq!(stats.ahead_behind, Some((1, 0)));
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
