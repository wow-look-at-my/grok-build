//! Where grok puts the worktrees it manages, and how to recognise them.
//!
//! A checkout grok creates lives under the grok home, at
//! `<grok home>/worktrees/<repo>/<label>`. Checkouts that a release wrote
//! inside the repository itself, at `<main checkout>/.grok/worktrees/<label>`,
//! are still recognised wherever a managed path is asked about.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Name of the directory that holds managed checkouts, under the grok home and
/// under a repository's own `.grok` directory.
pub const WORKTREES_DIR: &str = "worktrees";

/// Parent of the checkouts a release wrote inside their own repository.
const IN_REPO_GROK_DIR: &str = ".grok";

/// The directory one repository keeps its own checkouts in.
pub fn repo_worktrees_root(main_root: &Path) -> PathBuf {
    main_root.join(IN_REPO_GROK_DIR).join(WORKTREES_DIR)
}

/// True when `path` is a `<X>/.grok/worktrees` directory. Shape-only, so it
/// answers for a path that no longer exists on disk. A plain `worktrees/`
/// directory that is not inside a `.grok/` directory is not a managed root.
pub fn is_repo_worktrees_root(path: &Path) -> bool {
    let here_is_worktrees = path.file_name() == Some(OsStr::new(WORKTREES_DIR));
    let parent_is_dot_grok = path
        .parent()
        .is_some_and(|p| p.file_name() == Some(OsStr::new(IN_REPO_GROK_DIR)));
    here_is_worktrees && parent_is_dot_grok
}

/// The outermost `<X>/.grok/worktrees` directory that contains `path`.
pub fn enclosing_repo_worktrees_root(path: &Path) -> Option<PathBuf> {
    let mut outermost = None;
    for ancestor in path.ancestors().skip(1) {
        if is_repo_worktrees_root(ancestor) {
            outermost = Some(ancestor.to_path_buf());
        }
    }
    outermost
}

/// True when a directory's own name can name a checkout. Hidden entries and
/// the pool's claim markers sit beside checkouts without being ones, at
/// either level of a managed root.
pub fn is_worktree_entry_name(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        !(name.starts_with('.')
            || name.ends_with(".ready")
            || name.ends_with(".claimed")
            || name.ends_with(".claiming"))
    })
}

/// True when `path` is itself a checkout.
pub fn is_worktree_dir(path: &Path) -> bool {
    is_worktree_entry_name(path) && path.join(".git").symlink_metadata().is_ok()
}

/// The boundary of a managed layout that `path` sits in, if any. The returned
/// directory is never itself a checkout: a path is grok-managed exactly when
/// it is strictly below the boundary. `home_root` is the grok home's
/// `worktrees` directory, where new checkouts are created; a repository's own
/// `.grok/worktrees` is recognised as well, for the checkouts a release wrote
/// there.
pub fn managed_worktrees_boundary(path: &Path, home_root: &Path) -> Option<PathBuf> {
    if path.starts_with(home_root) {
        return Some(home_root.to_path_buf());
    }
    enclosing_repo_worktrees_root(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_worktrees_root_hangs_off_the_main_checkout() {
        assert_eq!(
            repo_worktrees_root(Path::new("/repos/thing")),
            PathBuf::from("/repos/thing/.grok/worktrees")
        );
    }

    #[test]
    fn managed_root_is_recognised_by_shape() {
        assert!(is_repo_worktrees_root(Path::new(
            "/repos/thing/.grok/worktrees"
        )));
        assert!(!is_repo_worktrees_root(Path::new(
            "/repos/thing/.grok/worktrees/label"
        )));
        assert!(!is_repo_worktrees_root(Path::new("/repos/thing/worktrees")));
        assert!(!is_repo_worktrees_root(Path::new("/repos/thing/.grok")));
    }

    #[test]
    fn enclosing_root_is_the_outermost_one() {
        // A checkout of a repo nested inside another checkout's managed tree
        // belongs to the outer repository.
        assert_eq!(
            enclosing_repo_worktrees_root(Path::new(
                "/outer/.grok/worktrees/inner/.grok/worktrees/label"
            )),
            Some(PathBuf::from("/outer/.grok/worktrees"))
        );
        assert_eq!(
            enclosing_repo_worktrees_root(Path::new("/r/.grok/worktrees/label/src/lib.rs")),
            Some(PathBuf::from("/r/.grok/worktrees"))
        );
        assert_eq!(
            enclosing_repo_worktrees_root(Path::new("/r/.grok/config.toml")),
            None
        );
    }

    #[test]
    fn boundary_prefers_the_given_home_root() {
        let home_root = Path::new("/home/me/.grok/worktrees");
        assert_eq!(
            managed_worktrees_boundary(Path::new("/home/me/.grok/worktrees/repo/label"), home_root),
            Some(home_root.to_path_buf())
        );
        assert_eq!(
            managed_worktrees_boundary(Path::new("/r/.grok/worktrees/label"), home_root),
            Some(PathBuf::from("/r/.grok/worktrees"))
        );
        assert_eq!(
            managed_worktrees_boundary(Path::new("/r/plain/dir"), home_root),
            None
        );
    }

    fn checkout_at(path: &Path, linked_gitdir: Option<&str>) {
        std::fs::create_dir_all(path).unwrap();
        match linked_gitdir {
            Some(gitdir) => {
                std::fs::write(path.join(".git"), format!("gitdir: {gitdir}\n")).unwrap()
            }
            None => std::fs::create_dir_all(path.join(".git")).unwrap(),
        }
    }

    /// The test every reader of the location asks, over both shapes that
    /// location has ever had.
    #[test]
    fn a_directory_is_a_checkout_by_its_git_entry() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path();
        let source = home.join("source-repo");

        // The unforked shape: the checkout sits directly under the root.
        let depth_one = home.join("worktrees/go-toolchain-dats-sandbox");
        checkout_at(
            &depth_one,
            Some(&format!(
                "{}/.git/worktrees/go-toolchain-dats-sandbox",
                source.display()
            )),
        );
        // The fork's shape: a bucket per repository, a checkout per label.
        let bucket = home.join("worktrees/repos-buildhost");
        let depth_two = bucket.join("2026-09-14-reclaim");
        checkout_at(
            &depth_two,
            Some("/nowhere/.git/worktrees/2026-09-14-reclaim"),
        );
        // A pool entry, which is the bucket shape under the other root.
        let pool = home.join("worktree_pool/inst-1/pool-a");
        checkout_at(&pool, None);
        // A leftover directory in a bucket that was never a checkout -- a go build cache, on the real machine.
        let cache = bucket.join("2026-09-14-gocache");
        std::fs::create_dir_all(cache.join("00")).unwrap();
        // Names that sit beside checkouts without being ones.
        checkout_at(&bucket.join(".hidden-wt"), None);
        checkout_at(&bucket.join("claim.ready"), None);
        // Inside an accepted checkout.
        std::fs::create_dir_all(depth_one.join("src")).unwrap();
        let nested = depth_one.join("vendor/lib");
        checkout_at(&nested, Some("/elsewhere/.git/worktrees/lib"));

        assert!(is_worktree_dir(&depth_one), "the depth-1 checkout is one");
        assert!(is_worktree_dir(&depth_two), "the depth-2 checkout is one");
        assert!(is_worktree_dir(&pool), "a pool entry is one");
        assert!(
            is_worktree_dir(&nested),
            "a nested repository really is a checkout by this test -- it is the \
             scan that must never ask below a checkout it already accepted"
        );

        assert!(!is_worktree_dir(&bucket), "a bucket is not a checkout");
        assert!(
            !is_worktree_dir(&cache),
            "a plain directory in a bucket is not a checkout"
        );
        assert!(
            !is_worktree_dir(&cache.join("00")),
            "nor is anything inside it"
        );
        assert!(
            !is_worktree_dir(&depth_one.join("src")),
            "a directory inside a checkout is not a second checkout"
        );
        assert!(
            !is_worktree_dir(&bucket.join(".hidden-wt")),
            "a hidden name is skipped whatever it holds"
        );
        assert!(
            !is_worktree_dir(&bucket.join("claim.ready")),
            "a claim marker is skipped whatever it holds"
        );
        assert!(
            !is_worktree_dir(home),
            "the managed root itself is never a checkout"
        );
    }

    #[test]
    fn entry_names_separate_markers_from_labels() {
        assert!(is_worktree_entry_name(Path::new("/r/wt/my-feature")));
        assert!(is_worktree_entry_name(Path::new(
            "/r/wt/2026-09-26-69e7b886"
        )));
        assert!(!is_worktree_entry_name(Path::new("/r/wt/.tmp_creating")));
        assert!(!is_worktree_entry_name(Path::new("/r/wt/abc.ready")));
        assert!(!is_worktree_entry_name(Path::new("/r/wt/abc.claimed")));
        assert!(!is_worktree_entry_name(Path::new("/r/wt/abc.claiming")));
        assert!(!is_worktree_entry_name(Path::new("")));
    }
}
