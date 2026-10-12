//! Where grok puts the worktrees it manages, and how to recognise them.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Name of the directory that holds a repository's own managed checkouts.
pub const WORKTREES_DIR: &str = "worktrees";

/// Per-repository parent of [`WORKTREES_DIR`].
pub const REPO_DOT_DIR: &str = ".grok";

/// Line registered in the main checkout's exclude data so its managed checkouts stay out of `git status`.
pub const WORKTREES_EXCLUDE_LINE: &str = ".grok/worktrees/";

/// The directory one repository keeps its managed checkouts in.
pub fn repo_worktrees_root(main_root: &Path) -> PathBuf {
    main_root.join(REPO_DOT_DIR).join(WORKTREES_DIR)
}

/// True when `path` is a `<X>/.grok/worktrees` directory. Shape-only, so it
/// answers for a path that no longer exists on disk. A plain `worktrees/`
/// directory that is not inside a `.grok/` directory is not a managed root.
pub fn is_repo_worktrees_root(path: &Path) -> bool {
    let here_is_worktrees = path.file_name() == Some(OsStr::new(WORKTREES_DIR));
    let parent_is_dot_grok = path
        .parent()
        .is_some_and(|p| p.file_name() == Some(OsStr::new(REPO_DOT_DIR)));
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

/// The main checkout that owns `path`, when `path` is one of its managed
/// checkouts or lives inside one.
pub fn main_root_for_managed_path(path: &Path) -> Option<PathBuf> {
    enclosing_repo_worktrees_root(path)
        .as_deref()
        .and_then(|managed_root| managed_root.parent())
        .and_then(|dot_grok| dot_grok.parent())
        .map(Path::to_path_buf)
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

/// The boundary of the managed layout that `path` sits in, if any. The
/// returned directory is never itself a checkout: a path is grok-managed
/// exactly when it is strictly below the boundary.
pub fn managed_worktrees_boundary(path: &Path, legacy_root: &Path) -> Option<PathBuf> {
    if path.starts_with(legacy_root) {
        return Some(legacy_root.to_path_buf());
    }
    enclosing_repo_worktrees_root(path)
}

/// Keeps the managed worktrees directory out of `main_root`'s `git status`.
/// The entry goes in the repository's own exclude data (`.git/info/exclude`),
/// never in the tracked `.gitignore`. Which directories a clone happens to
/// have checked out is not a property of the project. The write is
/// idempotent, so a repository with multiple worktrees carries one line.
pub fn exclude_managed_worktrees_dir(main_root: &Path) -> std::io::Result<()> {
    // A repository whose `.git` is a pointer file keeps `info/exclude` in the
    // common dir it names, not beside the working tree.
    let git_dir = gix::discover(main_root)
        .ok()
        .map(|repo| repo.common_dir().to_path_buf())
        .unwrap_or_else(|| main_root.join(".git"));
    let info_dir = git_dir.join("info");
    std::fs::create_dir_all(&info_dir)?;
    let exclude_path = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim_end() == WORKTREES_EXCLUDE_LINE)
    {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(WORKTREES_EXCLUDE_LINE);
    updated.push('\n');
    std::fs::write(&exclude_path, updated)
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
    fn main_root_is_the_owner_of_a_managed_checkout() {
        assert_eq!(
            main_root_for_managed_path(Path::new("/r/.grok/worktrees/label/crates/x")),
            Some(PathBuf::from("/r"))
        );
        assert_eq!(
            main_root_for_managed_path(Path::new("/r/.grok/worktrees")),
            None
        );
        assert_eq!(main_root_for_managed_path(Path::new("/r/plain/dir")), None);
    }

    #[test]
    fn boundary_prefers_the_configured_legacy_root() {
        let legacy = Path::new("/home/me/.grok/worktrees");
        assert_eq!(
            managed_worktrees_boundary(Path::new("/home/me/.grok/worktrees/repo/label"), legacy),
            Some(legacy.to_path_buf())
        );
        assert_eq!(
            managed_worktrees_boundary(Path::new("/r/.grok/worktrees/label"), legacy),
            Some(PathBuf::from("/r/.grok/worktrees"))
        );
        assert_eq!(
            managed_worktrees_boundary(Path::new("/r/plain/dir"), legacy),
            None
        );
    }

    #[test]
    fn exclusion_line_is_registered_once() {
        let temp = tempfile::TempDir::new().unwrap();
        let main_root = temp.path();
        std::fs::create_dir_all(main_root.join(".git")).unwrap();

        exclude_managed_worktrees_dir(main_root).unwrap();
        exclude_managed_worktrees_dir(main_root).unwrap();
        let written = std::fs::read_to_string(main_root.join(".git/info/exclude")).unwrap();
        assert_eq!(
            written.matches(WORKTREES_EXCLUDE_LINE).count(),
            1,
            "a repeated registration must not pile up entries: {written:?}"
        );
    }

    #[test]
    fn exclusion_keeps_an_existing_entry_intact() {
        let temp = tempfile::TempDir::new().unwrap();
        let main_root = temp.path();
        let exclude = main_root.join(".git/info/exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        std::fs::write(&exclude, "*.swp").unwrap();

        exclude_managed_worktrees_dir(main_root).unwrap();
        assert_eq!(
            std::fs::read_to_string(&exclude).unwrap(),
            "*.swp\n.grok/worktrees/\n"
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
