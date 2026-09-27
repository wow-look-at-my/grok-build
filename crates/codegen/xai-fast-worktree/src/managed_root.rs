//! Where grok puts the worktrees it manages, and how to recognise them.
//!
//! Every checkout grok creates for a repository lives in that repository, under
//! `<main checkout root>/.grok/worktrees/<label>`. Resolving the destination
//! off the main checkout rather than the current worktree is what makes
//! creation from inside an existing worktree land as a sibling instead of
//! nesting inside it.
//!
//! Checkouts grok created before that layout existed live under the user grok
//! home, at `<grok home>/worktrees/<repo bucket>/<label>`. Both shapes are
//! recognised, so a worktree at either location stays grok-managed; only new
//! destinations changed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Name of the directory that holds a repository's own managed checkouts.
pub const WORKTREES_DIR: &str = "worktrees";

/// Per-repository parent of [`WORKTREES_DIR`].
pub const REPO_DOT_DIR: &str = ".grok";

/// Line registered in the main checkout's exclude data so its managed
/// checkouts stay out of `git status`.
pub const WORKTREES_EXCLUDE_LINE: &str = ".grok/worktrees/";

/// The directory one repository keeps its managed checkouts in.
pub fn repo_worktrees_root(main_root: &Path) -> PathBuf {
    main_root.join(REPO_DOT_DIR).join(WORKTREES_DIR)
}

/// True when `path` is a `<X>/.grok/worktrees` directory.
///
/// Shape-only, so it answers for a path that no longer exists on disk. A plain
/// `worktrees/` directory that is not inside a `.grok/` directory is not a
/// managed root.
pub fn is_repo_worktrees_root(path: &Path) -> bool {
    let here_is_worktrees = path.file_name() == Some(OsStr::new(WORKTREES_DIR));
    let parent_is_dot_grok = path
        .parent()
        .is_some_and(|p| p.file_name() == Some(OsStr::new(REPO_DOT_DIR)));
    here_is_worktrees && parent_is_dot_grok
}

/// The outermost `<X>/.grok/worktrees` directory that contains `path`.
///
/// Walking outward (rather than stopping at the innermost match) keeps a
/// checkout of a repository nested inside another checkout's managed tree
/// grouped with the outer repository, so its siblings are placed beside it
/// instead of inside it.
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

/// The boundary of the managed layout that `path` sits in, if any.
///
/// The returned directory is never itself a checkout: a path is grok-managed
/// exactly when it is strictly below the boundary. Two layouts produce a
/// boundary:
///
/// - A repository's own `<X>/.grok/worktrees`.
/// - The legacy `legacy_root` (the `<grok home>/worktrees` this tool wrote
///   before checkouts moved into their repository).
///
/// `legacy_root` is checked first because it is the configured answer: a
/// managed checkout of a repository that happens to live under the grok home
/// belongs to that repository, so preferring the legacy boundary would move it
/// out from under its own repo.
pub fn managed_worktrees_boundary(path: &Path, legacy_root: &Path) -> Option<PathBuf> {
    if path.starts_with(legacy_root) {
        return Some(legacy_root.to_path_buf());
    }
    enclosing_repo_worktrees_root(path)
}

/// True when `path` is at or below a grok-managed worktrees directory, in
/// either layout.
pub fn path_in_managed_worktrees(path: &Path, legacy_root: &Path) -> bool {
    managed_worktrees_boundary(path, legacy_root).is_some()
}

/// Keeps the managed worktrees directory out of `main_root`'s `git status`.
///
/// The entry goes in the repository's own exclude data (`.git/info/exclude`),
/// never in the tracked `.gitignore`: which directories a clone happens to have
/// checked out is not a property of the project. The write is idempotent, so a
/// repository with fifty worktrees carries one line.
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
}
