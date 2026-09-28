//! Filesystem scanner for discovering worktrees not yet tracked in the DB.
//!
//! Each managed root under the grok home is read on the same rule the rest of
//! the crate uses ([`crate::managed_root::is_worktree_dir`]): a directory is a
//! checkout when it carries a `.git` entry. Two shapes have written that root
//! over the versions, and both are read: the fork's, which buckets checkouts
//! per repository at `<root>/<repo>/<label>`, and the unforked one, which puts
//! the checkout directly under the root at `<root>/<label>`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::db::{
    WorktreeKind, WorktreeRecord, WorktreeStatus, id_from_path, now_epoch_secs, repo_name_from_path,
};
use crate::managed_root::{WORKTREES_DIR, is_worktree_dir, is_worktree_entry_name};

pub const WORKTREE_POOL_DIR: &str = "worktree_pool";

#[derive(Debug)]
pub struct DiscoveredWorktree {
    pub path: PathBuf,
    pub kind: WorktreeKind,
    pub creation_mode: &'static str,
    pub source_repo: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub found: Vec<DiscoveredWorktree>,
    pub skipped: u64,
}

fn detect_creation_mode(worktree_path: &Path) -> &'static str {
    let git_entry = worktree_path.join(".git");
    if git_entry.is_file() {
        "linked"
    } else if git_entry.is_dir() {
        "standalone"
    } else {
        "unknown"
    }
}

fn detect_source_repo(worktree_path: &Path) -> Option<PathBuf> {
    let git_entry = worktree_path.join(".git");
    if git_entry.is_file() {
        let content = std::fs::read_to_string(&git_entry).ok()?;
        let gitdir = content.trim().strip_prefix("gitdir: ")?;
        // Walk up from .git/worktrees/<name> → .git → repo root
        Path::new(gitdir)
            .parent()?
            .parent()?
            .parent()
            .map(|p| p.to_path_buf())
    } else if git_entry.is_dir() {
        Some(worktree_path.to_path_buf())
    } else {
        None
    }
}

/// One record per checkout under a managed root, and none for anything inside
/// one.
///
/// Both shapes that root has ever been written in are read, and the level is a
/// consequence of the `.git` test rather than its definition: an unforked
/// build's checkout is a direct child of the root, this fork's sits one level
/// lower inside a per-repository bucket. Nothing below the bucket level is ever
/// asked, because every directory further down is inside a checkout that was
/// already reported -- so the walk still costs one listing of the root and one
/// of each bucket, which is all the depth reading it costs.
fn scan_managed_root(base_dir: &Path, kind: WorktreeKind, report: &mut DiscoveryReport) {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !is_worktree_entry_name(&path) {
            report.skipped += 1;
            continue;
        }
        if is_worktree_dir(&path) {
            report.found.push(discovered(path, kind));
            continue;
        }
        scan_bucket(&path, kind, report);
    }
}

/// The bucketed shape: the bucket is never a checkout itself, and a plain
/// directory among its children -- a leftover cache, say -- is not one either.
fn scan_bucket(bucket: &Path, kind: WorktreeKind, report: &mut DiscoveryReport) {
    let Ok(entries) = std::fs::read_dir(bucket) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_worktree_dir(&path) {
            report.skipped += 1;
            continue;
        }
        report.found.push(discovered(path, kind));
    }
}

fn discovered(path: PathBuf, kind: WorktreeKind) -> DiscoveredWorktree {
    DiscoveredWorktree {
        creation_mode: detect_creation_mode(&path),
        source_repo: detect_source_repo(&path),
        path,
        kind,
    }
}

pub fn discover_worktrees(grok_home: &Path) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    scan_managed_root(
        &grok_home.join(WORKTREES_DIR),
        WorktreeKind::Session,
        &mut report,
    );
    scan_managed_root(
        &grok_home.join(WORKTREE_POOL_DIR),
        WorktreeKind::Pool,
        &mut report,
    );
    report
}

fn fs_creation_time(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.created())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(now_epoch_secs)
}

impl DiscoveredWorktree {
    pub fn into_record(self) -> WorktreeRecord {
        let repo_name = self
            .source_repo
            .as_deref()
            .map(repo_name_from_path)
            .unwrap_or_else(|| "unknown".to_string());
        let source_repo = self.source_repo.unwrap_or_else(|| PathBuf::from("unknown"));
        let created_at = fs_creation_time(&self.path);
        // Match `WorktreeDb::get`, which looks up by canonical path.
        let path = dunce::canonicalize(&self.path).unwrap_or(self.path);

        WorktreeRecord {
            id: id_from_path(&path),
            path,
            source_repo,
            repo_name,
            kind: self.kind,
            creation_mode: self.creation_mode.to_owned(),
            git_ref: None,
            head_commit: None,
            session_id: None,
            creator_pid: None,
            created_at,
            last_accessed_at: None,
            status: WorktreeStatus::Alive,
            metadata: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RebuildReport {
    pub discovered: u64,
    pub registered: u64,
    pub already_tracked: u64,
}

pub fn managed_worktree_roots(grok_home: &Path) -> [PathBuf; 2] {
    [
        grok_home.join(WORKTREES_DIR),
        grok_home.join(WORKTREE_POOL_DIR),
    ]
    .map(|root| dunce::canonicalize(&root).unwrap_or(root))
}

/// True when `path` is under a managed root (`worktrees/` or `worktree_pool/`).
/// Prefer an already-canonical `path`; the roots are canonicalized inside.
pub fn path_under_managed_worktree_roots(path: &Path, grok_home: &Path) -> bool {
    path_under_worktree_roots(path, &managed_worktree_roots(grok_home))
}

/// True when `path` is under (or is) one of `roots`, both already canonical.
pub fn path_under_worktree_roots(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

pub fn rebuild_worktree_db(
    db: &crate::db::WorktreeDb,
    grok_home: &Path,
) -> anyhow::Result<RebuildReport> {
    let discovery = discover_worktrees(grok_home);
    let mut report = RebuildReport {
        discovered: discovery.found.len() as u64,
        ..Default::default()
    };
    let now = now_epoch_secs();
    let roots = managed_worktree_roots(grok_home);

    for wt in discovery.found {
        let path = dunce::canonicalize(&wt.path).unwrap_or_else(|_| wt.path.clone());
        // Refuse symlink escape outside managed roots.
        if !path_under_worktree_roots(&path, &roots) {
            tracing::warn!(
                path = %path.display(),
                "rebuild skipped path outside grok worktrees/worktree_pool"
            );
            continue;
        }
        let id = id_from_path(&path);
        let path_str = path.to_string_lossy();
        if db.get_by_id(&id)?.is_some() || db.get(&path_str)?.is_some() {
            report.already_tracked += 1;
            continue;
        }
        let mut rec = wt.into_record();
        // Touch so same-pass age GC does not reclaim solely from old FS mtime.
        rec.last_accessed_at = Some(now);
        db.register(&rec)?;
        report.registered += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fake_linked_worktree(path: &Path, gitdir_target: &str) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join(".git"), format!("gitdir: {gitdir_target}\n")).unwrap();
    }

    fn make_fake_standalone_worktree(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
    }

    #[test]
    fn discover_session_worktrees() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();

        let wt = grok_home.join("worktrees/myrepo/worktree-abc123");
        make_fake_linked_worktree(&wt, "/repo/.git/worktrees/abc123");

        let report = discover_worktrees(grok_home);
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.found[0].kind, WorktreeKind::Session);
        assert_eq!(report.found[0].creation_mode, "linked");
        assert_eq!(report.found[0].path, wt);
    }

    #[test]
    fn discover_pool_worktrees() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();

        let wt = grok_home.join("worktree_pool/inst-1/pool-a");
        make_fake_standalone_worktree(&wt);

        let report = discover_worktrees(grok_home);
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.found[0].kind, WorktreeKind::Pool);
        assert_eq!(report.found[0].creation_mode, "standalone");
    }

    #[test]
    fn skips_dot_prefixed_and_markers() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();

        let base = grok_home.join("worktrees/myrepo");
        std::fs::create_dir_all(&base).unwrap();

        std::fs::create_dir_all(base.join(".tmp_creating")).unwrap();
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        std::fs::write(base.join("abc.ready"), "").unwrap();
        std::fs::write(base.join("abc.claimed"), "").unwrap();

        make_fake_standalone_worktree(&base.join("real-session"));

        let report = discover_worktrees(grok_home);
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.found[0].path, base.join("real-session"));
        assert!(report.skipped > 0);
    }

    #[test]
    fn discover_empty_dirs_is_fine() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = discover_worktrees(tmp.path());
        assert!(report.found.is_empty());
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn rebuild_registers_and_skips_duplicates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();

        let wt = grok_home.join("worktrees/repo/worktree-sess1");
        make_fake_standalone_worktree(&wt);

        let db = crate::db::WorktreeDb::open_in_memory().unwrap();

        let r1 = rebuild_worktree_db(&db, grok_home).unwrap();
        assert_eq!(r1.discovered, 1);
        assert_eq!(r1.registered, 1);
        assert_eq!(r1.already_tracked, 0);

        let r2 = rebuild_worktree_db(&db, grok_home).unwrap();
        assert_eq!(r2.discovered, 1);
        assert_eq!(r2.registered, 0);
        assert_eq!(r2.already_tracked, 1);
    }

    #[test]
    fn rebuild_keeps_same_basename_worktrees_in_different_repos() {
        // The cross-repo eviction bug: two repos each have a `wt-abc`
        // worktree. Discovery + rebuild must register BOTH (distinct ids), not
        // collapse them into one and then permanently skip the other.
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();

        let wt_a = grok_home.join("worktrees/repo-a/wt-abc");
        let wt_b = grok_home.join("worktrees/repo-b/wt-abc");
        make_fake_standalone_worktree(&wt_a);
        make_fake_standalone_worktree(&wt_b);

        let db = crate::db::WorktreeDb::open_in_memory().unwrap();
        let report = rebuild_worktree_db(&db, grok_home).unwrap();
        assert_eq!(report.discovered, 2);
        assert_eq!(
            report.registered, 2,
            "both same-basename worktrees must register"
        );

        let all = db.list(&crate::db::ListFilter::default()).unwrap();
        assert_eq!(all.len(), 2);
        assert!(db.get(&wt_a.to_string_lossy()).unwrap().is_some());
        assert!(db.get(&wt_b.to_string_lossy()).unwrap().is_some());

        // Idempotent: a second rebuild finds both already tracked, skips neither.
        let report2 = rebuild_worktree_db(&db, grok_home).unwrap();
        assert_eq!(report2.registered, 0);
        assert_eq!(report2.already_tracked, 2);
    }

    #[test]
    fn detect_source_repo_from_linked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let wt = tmp.path().join("wt");
        let gitdir = "/home/user/myrepo/.git/worktrees/wt";
        make_fake_linked_worktree(&wt, gitdir);

        let source = detect_source_repo(&wt);
        assert_eq!(source, Some(PathBuf::from("/home/user/myrepo")));
    }

    #[test]
    fn rebuild_report_serde_round_trip() {
        let report = RebuildReport {
            discovered: 5,
            registered: 3,
            already_tracked: 2,
        };
        let json = serde_json::to_string(&report).unwrap();
        let deser: RebuildReport = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.discovered, 5);
        assert_eq!(deser.registered, 3);
        assert_eq!(deser.already_tracked, 2);
    }

    #[test]
    fn rebuild_sets_last_accessed_at() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path();
        let wt = grok_home.join("worktrees/repo/sess");
        make_fake_standalone_worktree(&wt);
        let db = crate::db::WorktreeDb::open_in_memory().unwrap();
        rebuild_worktree_db(&db, grok_home).unwrap();
        let rec = db.get(&wt.to_string_lossy()).unwrap().expect("registered");
        assert!(
            rec.last_accessed_at.is_some(),
            "rebuild must touch last_accessed_at for same-pass age safety"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_skips_symlink_escape_outside_managed_roots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let grok_home = tmp.path().join("grok");
        let outside = tmp.path().join("outside-real");
        make_fake_standalone_worktree(&outside);
        let link_parent = grok_home.join("worktrees/repo");
        std::fs::create_dir_all(&link_parent).unwrap();
        std::os::unix::fs::symlink(&outside, link_parent.join("escaped")).unwrap();

        let db = crate::db::WorktreeDb::open_in_memory().unwrap();
        let report = rebuild_worktree_db(&db, &grok_home).unwrap();
        assert_eq!(report.discovered, 1);
        assert_eq!(report.registered, 0, "symlink escape must not register");
        assert!(
            db.list(&crate::db::ListFilter::default())
                .unwrap()
                .is_empty()
        );
        assert!(!path_under_managed_worktree_roots(
            &dunce::canonicalize(&outside).unwrap(),
            &grok_home
        ));
    }

    /// A real source repository with a commit and a subdirectory, so a checkout
    /// of it has something inside it for the scan to wrongly report.
    fn source_repo(temp: &tempfile::TempDir) -> PathBuf {
        let repo = temp.path().join("source-repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        xai_test_utils::git::init_git_repo(&repo);
        std::fs::write(repo.join("tracked.txt"), "content").unwrap();
        std::fs::write(repo.join("src/main.rs"), "fn main() {}").unwrap();
        xai_test_utils::git::git_commit_all(&repo, "initial");
        repo
    }

    fn add_worktree(repo: &Path, dest: &Path, branch: &str) {
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        xai_test_utils::git::run_git(
            repo,
            &["worktree", "add", "-b", branch, &dest.to_string_lossy()],
        );
    }

    fn found_paths(report: &DiscoveryReport) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = report.found.iter().map(|w| w.path.clone()).collect();
        paths.sort();
        paths
    }

    /// Criterion 1: the shape an unforked grok build leaves -- the checkout is a
    /// direct child of the root -- is one worktree, described by its own `.git`.
    #[test]
    fn discovers_a_checkout_sitting_directly_under_the_managed_root() {
        xai_test_utils::require_git!();
        let temp = tempfile::TempDir::new().unwrap();
        let grok_home = temp.path().join("grok-home");
        let repo = source_repo(&temp);
        let checkout = grok_home.join("worktrees/go-toolchain-dats-sandbox");
        add_worktree(&repo, &checkout, "dats-sandbox");

        let report = discover_worktrees(&grok_home);
        assert_eq!(
            found_paths(&report),
            vec![checkout.clone()],
            "the checkout is reported once, not with a record per directory inside it"
        );
        let found = &report.found[0];
        assert_eq!(found.kind, WorktreeKind::Session);
        assert_eq!(found.creation_mode, "linked");
        assert_eq!(
            found.source_repo.as_deref(),
            Some(dunce::canonicalize(&repo).unwrap().as_path()),
            "the source repository comes from the checkout's own gitdir pointer"
        );
        assert!(
            checkout.join("src").is_dir(),
            "the checkout really does hold a subdirectory the scan could report"
        );
    }

    /// Criterion 2: the bucketed depth and the pool root are still read.
    #[test]
    fn discovers_every_shape_the_old_location_has_been_written_in() {
        xai_test_utils::require_git!();
        let temp = tempfile::TempDir::new().unwrap();
        let grok_home = temp.path().join("grok-home");
        let repo = source_repo(&temp);
        let shallow = grok_home.join("worktrees/go-toolchain-dats-sandbox");
        let bucketed = grok_home.join("worktrees/repos-buildhost/2026-09-14-reclaim");
        let pool = grok_home.join("worktree_pool/inst-1/pool-a");
        add_worktree(&repo, &shallow, "shallow-branch");
        add_worktree(&repo, &bucketed, "bucketed-branch");
        add_worktree(&repo, &pool, "pool-branch");

        let report = discover_worktrees(&grok_home);
        let mut found: Vec<(PathBuf, WorktreeKind, &'static str)> = report
            .found
            .iter()
            .map(|w| (w.path.clone(), w.kind, w.creation_mode))
            .collect();
        found.sort();
        let mut expected: Vec<(PathBuf, WorktreeKind, &'static str)> = vec![
            (bucketed, WorktreeKind::Session, "linked"),
            (pool, WorktreeKind::Pool, "linked"),
            (shallow, WorktreeKind::Session, "linked"),
        ];
        expected.sort();
        assert_eq!(
            found, expected,
            "both depths and the pool root, each exactly once and under its own kind"
        );
    }

    /// Criteria 3 and 6's negative: a plain directory under a managed root is
    /// not a checkout -- here the go build cache a bucket actually holds on the
    /// developer's machine -- and neither is anything below a checkout.
    #[test]
    fn reports_no_record_for_a_directory_that_is_not_a_checkout() {
        xai_test_utils::require_git!();
        let temp = tempfile::TempDir::new().unwrap();
        let grok_home = temp.path().join("grok-home");
        let repo = source_repo(&temp);
        let checkout = grok_home.join("worktrees/repos-buildhost/2026-09-14-reclaim");
        add_worktree(&repo, &checkout, "reclaim");

        let cache = grok_home.join("worktrees/repos-buildhost/2026-09-14-gocache");
        std::fs::create_dir_all(cache.join("00")).unwrap();
        std::fs::write(cache.join("00/blob"), "not a checkout").unwrap();
        // A repository nested inside a checkout: a submodule's checkout dir.
        let nested = checkout.join("vendor/lib");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            "gitdir: /elsewhere/.git/worktrees/lib\n",
        )
        .unwrap();

        let report = discover_worktrees(&grok_home);
        assert_eq!(
            found_paths(&report),
            vec![checkout.clone()],
            "one record for the checkout, none for the cache beside it, its \
             directories, or a repository nested inside it"
        );
    }

    /// Criterion 5: the rebuild registers both depths and adds nothing twice.
    #[test]
    fn rebuild_registers_both_depths_once_and_adds_nothing_on_a_second_pass() {
        xai_test_utils::require_git!();
        let temp = tempfile::TempDir::new().unwrap();
        let grok_home = temp.path().join("grok-home");
        let repo = source_repo(&temp);
        let shallow = grok_home.join("worktrees/go-toolchain-dats-sandbox");
        let bucketed = grok_home.join("worktrees/repos-buildhost/2026-09-14-reclaim");
        add_worktree(&repo, &shallow, "shallow-branch");
        add_worktree(&repo, &bucketed, "bucketed-branch");

        let db = crate::db::WorktreeDb::open_in_memory().unwrap();
        let first = rebuild_worktree_db(&db, &grok_home).unwrap();
        assert_eq!(first.discovered, 2);
        assert_eq!(first.registered, 2, "both depths register");

        let listed = db.list(&crate::db::ListFilter::default()).unwrap();
        let mut registered: Vec<PathBuf> = listed.iter().map(|r| r.path.clone()).collect();
        registered.sort();
        let mut expected = vec![
            dunce::canonicalize(&bucketed).unwrap(),
            dunce::canonicalize(&shallow).unwrap(),
        ];
        expected.sort();
        assert_eq!(
            registered, expected,
            "each is registered under its own path"
        );

        let second = rebuild_worktree_db(&db, &grok_home).unwrap();
        assert_eq!(second.discovered, 2);
        assert_eq!(second.registered, 0);
        assert_eq!(second.already_tracked, 2, "a second pass adds nothing");
    }

    /// Criterion 7: discovery and the rebuild it feeds are read-only.
    #[test]
    fn a_scan_and_a_rebuild_leave_the_checkout_on_disk_as_they_found_it() {
        xai_test_utils::require_git!();
        let temp = tempfile::TempDir::new().unwrap();
        let grok_home = temp.path().join("grok-home");
        let repo = source_repo(&temp);
        let shallow = grok_home.join("worktrees/go-toolchain-dats-sandbox");
        let bucketed = grok_home.join("worktrees/repos-buildhost/2026-09-14-reclaim");
        add_worktree(&repo, &shallow, "shallow-branch");
        add_worktree(&repo, &bucketed, "bucketed-branch");

        let listing_before: Vec<String> = std::fs::read_dir(grok_home.join("worktrees"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let gitdir_before = std::fs::read_to_string(shallow.join(".git")).unwrap();
        let branch_before =
            xai_test_utils::git::run_git(&shallow, &["rev-parse", "--abbrev-ref", "HEAD"]);

        let db = crate::db::WorktreeDb::open_in_memory().unwrap();
        discover_worktrees(&grok_home);
        rebuild_worktree_db(&db, &grok_home).unwrap();

        let listing_after: Vec<String> = std::fs::read_dir(grok_home.join("worktrees"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            listing_after, listing_before,
            "the scan added, moved, or removed nothing"
        );
        assert_eq!(
            std::fs::read_to_string(shallow.join(".git")).unwrap(),
            gitdir_before,
            "the checkout's gitdir pointer is untouched"
        );
        assert_eq!(
            xai_test_utils::git::run_git(&shallow, &["rev-parse", "--abbrev-ref", "HEAD"]),
            branch_before,
            "and so is the branch checked out in it"
        );
    }
}
