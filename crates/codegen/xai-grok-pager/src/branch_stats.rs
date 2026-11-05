//! Branch divergence and uncommitted line counts, shown beside the branch.
use std::collections::HashMap;

use gix::bstr::ByteSlice;
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
    // An own thread, not the blocking pool: an unprivileged thread cannot raise its priority again.
    // A pool thread would keep the low priority for unrelated work.
    let refresh = move || {
        let _runtime = handle.enter();
        lower_thread_priority();
        // A panic in the git read must not take down the pager.
        let stats = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compute(&cwd, LineStats::Cached)
        }));
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
    };
    if let Err(e) = std::thread::Builder::new()
        .name("branch-stats".into())
        .spawn(refresh)
    {
        tracing::warn!(error = %e, "branch stats: cannot start the refresh thread");
    }
}

/// The lowest CPU priority for the calling thread: the highest nice value on Linux. The work-tree scan starts its
/// worker threads from this thread, and on Linux they inherit the value. On macOS
/// `setpriority(PRIO_PROCESS)` acts on the whole process, so the thread uses the background class.
fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: gettid and setpriority read no memory; they only change this thread's nice value.
    let rc = unsafe {
        let tid = libc::gettid() as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 19)
    };
    #[cfg(target_os = "macos")]
    // SAFETY: as above. PRIO_DARWIN_THREAD with a value of zero names the calling thread.
    let rc = unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let rc = 0;
    if rc != 0 {
        tracing::warn!(
            error = %std::io::Error::last_os_error(),
            "branch stats: cannot lower the refresh thread's priority; the scan runs at normal priority"
        );
    }
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
    compute(cwd, LineStats::Fresh)
}

/// How [`compute`] gets the line counts.
#[derive(Clone, Copy)]
enum LineStats {
    /// Scan the work tree now.
    Fresh,
    /// Reuse the last scan until the work tree changes.
    Cached,
}

fn compute(cwd: &Path, line_stats: LineStats) -> anyhow::Result<Option<BranchStats>> {
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
        let head_tree = head_commit.tree_id()?.detach();
        let (insertions, deletions) = match line_stats {
            LineStats::Fresh => worktree_line_stats(&repo, head_tree)?,
            LineStats::Cached => cached_line_stats(&repo, head_tree)?,
        };
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
        .index(gix::worktree::IndexPersistedOrInMemory::Persisted(
            xai_grok_workspace::git_repo::index(repo)?,
        ))
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
            .lookup_entry_by_path(gix::path::from_bstr(path.as_bstr()))?
            .map(|entry| (entry.object_id(), entry.mode().kind()));
        let (old_id, old_kind) = match old {
            Some((id, kind @ (EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link))) => {
                (id, kind)
            }
            Some(_) => continue,
            None => (null, EntryKind::Blob),
        };
        let new_kind =
            match std::fs::symlink_metadata(workdir.join(gix::path::from_bstr(path.as_bstr()))) {
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

/// A line count is a full work-tree scan, the same work as `git status`, on several cores.
const UNWATCHED_SCAN_INTERVAL: Duration = Duration::from_secs(60);

/// The last line count for one work tree.
struct WorktreeScan {
    head_tree: gix::ObjectId,
    counts: (usize, usize),
    at: Instant,
    scans: u64,
    /// The watcher skips ignored paths, so a build output does not cause a scan.
    watch: Option<xai_fsnotify::ChangeCounter>,
    /// The watcher count before the scan.
    changes: Option<u64>,
}

impl WorktreeScan {
    fn is_current(&self, head_tree: gix::ObjectId) -> bool {
        if self.head_tree != head_tree {
            return false;
        }
        let now = self.watch.as_ref().and_then(|watch| watch.current());
        match (now, self.changes) {
            (Some(now), Some(then)) => now == then,
            _ => self.at.elapsed() < UNWATCHED_SCAN_INTERVAL,
        }
    }
}

static SCANS: LazyLock<Mutex<HashMap<PathBuf, WorktreeScan>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// [`worktree_line_stats`], reused until the watcher reports a change or HEAD moves.
/// The status bar asks every [`REFRESH_TTL`], so a scan on each request keeps
/// cores busy on an idle session.
fn cached_line_stats(
    repo: &gix::Repository,
    head_tree: gix::ObjectId,
) -> anyhow::Result<(usize, usize)> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("bare repository"))?
        .to_path_buf();
    let (watch, scans) = {
        let mut map = SCANS
            .lock()
            .map_err(|_| anyhow::anyhow!("scan cache poisoned"))?;
        match map.get(&workdir) {
            Some(scan) if scan.is_current(head_tree) => return Ok(scan.counts),
            _ => {}
        }
        let previous = map.remove(&workdir);
        let scans = previous.as_ref().map_or(0, |scan| scan.scans);
        let watch = previous
            .and_then(|mut scan| scan.watch.take())
            .filter(|watch| watch.current().is_some());
        (watch, scans)
    };
    let watch = match watch {
        Some(watch) => Some(watch),
        None => match xai_fsnotify::ChangeCounter::start(&workdir) {
            Ok(watch) => Some(watch),
            Err(e) => {
                tracing::warn!(
                    workdir = %workdir.display(),
                    error = %e,
                    "no file watcher for the status-bar line counts; they refresh every {}s",
                    UNWATCHED_SCAN_INTERVAL.as_secs()
                );
                None
            }
        },
    };
    // Read the count before the scan, so an edit during the scan causes the next one.
    let changes = watch.as_ref().and_then(|watch| watch.current());
    let counts = worktree_line_stats(repo, head_tree)?;
    let mut map = SCANS
        .lock()
        .map_err(|_| anyhow::anyhow!("scan cache poisoned"))?;
    if map.len() >= CACHE_CAP
        && let Some(oldest) = map
            .iter()
            .min_by_key(|(_, scan)| scan.at)
            .map(|(k, _)| k.clone())
    {
        map.remove(&oldest);
    }
    map.insert(
        workdir,
        WorktreeScan {
            head_tree,
            counts,
            at: Instant::now(),
            scans: scans + 1,
            watch,
            changes,
        },
    );
    Ok(counts)
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

    #[cfg(target_os = "linux")]
    fn this_thread_nice() -> i32 {
        // SAFETY: gettid and getpriority read no memory.
        unsafe { libc::getpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t) }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_refresh_thread_and_its_scan_workers_run_at_the_lowest_priority() {
        let (refresh, worker) = std::thread::spawn(|| {
            lower_thread_priority();
            let worker = std::thread::spawn(this_thread_nice).join().unwrap();
            (this_thread_nice(), worker)
        })
        .join()
        .unwrap();
        assert_eq!(refresh, 19, "the refresh thread runs at nice 19");
        assert_eq!(
            worker, 19,
            "a thread it starts, as gix does for the scan, inherits nice 19"
        );
        assert_ne!(this_thread_nice(), 19, "the caller's priority is untouched");
    }

    async fn cached(dir: &Path) -> (BranchStats, u64) {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let stats = compute(&dir, LineStats::Cached).unwrap().unwrap();
            let repo = xai_grok_workspace::git_repo::discover(&dir).unwrap();
            let workdir = repo.workdir().unwrap().to_path_buf();
            let scans = SCANS.lock().unwrap().get(&workdir).map_or(0, |s| s.scans);
            (stats, scans)
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unchanged_work_tree_is_not_scanned_again() {
        let tmp = repo_on_master();
        let dir = tmp.path();

        let (stats, scans) = cached(dir).await;
        assert_eq!((stats.insertions, scans), (0, 1));
        for _ in 0..5 {
            let (_, scans) = cached(dir).await;
            assert_eq!(
                scans, 1,
                "a refresh with no change must reuse the last scan"
            );
        }

        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (stats, _) = cached(dir).await;
            if stats.insertions == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the watcher never reported the edit"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // The events of one write can arrive in more than one batch. Let them settle.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (_, settled) = cached(dir).await;
        for _ in 0..5 {
            let (_, scans) = cached(dir).await;
            assert_eq!(scans, settled, "a quiet work tree is not scanned again");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_commit_rescans_the_work_tree() {
        let tmp = repo_on_master();
        let dir = tmp.path();
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let (stats, _) = cached(dir).await;
        assert_eq!(stats.insertions, 1);

        git(dir, &["commit", "-qam", "two"]);
        let (stats, _) = cached(dir).await;
        assert_eq!(stats.insertions, 0, "a new HEAD tree forces a scan");
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
