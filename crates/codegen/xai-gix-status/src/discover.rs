//! Repository discovery: the worktree root, the common directory and the `origin` URL of the repository that contains a path.

use std::path::{Path, PathBuf};

/// Open the repository that contains `path`. The walk starts at the canonical
/// form of `path` and crosses file systems. Configuration comes from the
/// repository, system and user files. `GIT_CONFIG_*` variables are not read.
/// A repository that another user owns fails unless `safe.directory` names
/// it.
pub fn discover(path: &Path) -> Option<gix::Repository> {
    let start = dunce::canonicalize(path).ok()?;
    let mut trust_map = gix::sec::trust::Mapping::<gix::open::Options>::default();
    for options in [&mut trust_map.full, &mut trust_map.reduced] {
        options.permissions.config.env = false;
        *options = options.clone().bail_if_untrusted(true);
    }
    let upwards = gix::discover::upwards::Options {
        cross_fs: true,
        ..Default::default()
    };
    gix::ThreadSafeRepository::discover_opts(start, upwards, trust_map)
        .ok()
        .map(Into::into)
}

/// The worktree root of `repo`: canonical, with a trailing separator. `None` for a bare repository.
pub fn workdir(repo: &gix::Repository) -> Option<PathBuf> {
    let dir = repo.workdir()?;
    Some(canonical(dir).join(""))
}

/// The worktree root of the repository that contains `path`, as [`workdir`] gives it.
pub fn discover_workdir(path: &Path) -> Option<PathBuf> {
    workdir(&discover(path)?)
}

/// The canonical common directory of `repo`. A linked worktree shares it with the main worktree.
pub fn common_dir(repo: &gix::Repository) -> PathBuf {
    canonical(repo.common_dir())
}

/// True when `repo` is a linked worktree, not the main worktree.
pub fn is_linked_worktree(repo: &gix::Repository) -> bool {
    canonical(repo.git_dir()) != common_dir(repo)
}

/// The fetch URL of the `origin` remote, with `url.<base>.insteadOf` applied.
pub fn origin_url(repo: &gix::Repository) -> Option<String> {
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    Some(url.to_bstring().to_string())
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A temp dir holding `main/`, a repository with one commit and an `origin` remote.
    fn main_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let main = dunce::canonicalize(tmp.path()).unwrap().join("main");
        std::fs::create_dir_all(main.join("sub")).unwrap();
        git(&main, &["init", "--quiet"]);
        git(
            &main,
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "--no-gpg-sign",
                "-m",
                "i",
            ],
        );
        git(
            &main,
            &["remote", "add", "origin", "git@example.com:org/repo.git"],
        );
        (tmp, main)
    }

    #[test]
    fn workdir_is_canonical_with_trailing_separator() {
        let (_tmp, main) = main_repo();
        let root = discover_workdir(&main.join("sub")).unwrap();
        assert_eq!(root.to_string_lossy(), format!("{}/", main.display()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_start_resolves_to_the_target_repository() {
        let (tmp, main) = main_repo();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(main.join("sub"), &link).unwrap();
        assert_eq!(discover_workdir(&link), Some(main.join("")));
    }

    #[test]
    fn missing_path_finds_nothing() {
        let (_tmp, main) = main_repo();
        assert!(discover(&main.join("missing")).is_none());
    }

    #[test]
    fn linked_worktree_shares_the_main_common_dir() {
        let (tmp, main) = main_repo();
        git(&main, &["worktree", "add", "--quiet", "-b", "wt", "../wt"]);
        let main_repo = discover(&main).unwrap();
        let linked = discover(&tmp.path().join("wt")).unwrap();
        assert!(!is_linked_worktree(&main_repo));
        assert!(is_linked_worktree(&linked));
        assert_eq!(common_dir(&linked), main.join(".git"));
        assert_eq!(common_dir(&main_repo), main.join(".git"));
    }

    #[test]
    fn origin_url_reads_only_origin() {
        let (_tmp, main) = main_repo();
        let repo = discover(&main).unwrap();
        assert_eq!(
            origin_url(&repo).as_deref(),
            Some("git@example.com:org/repo.git")
        );
        git(&main, &["remote", "remove", "origin"]);
        git(
            &main,
            &["remote", "add", "upstream", "https://example.com/a/b.git"],
        );
        assert_eq!(origin_url(&discover(&main).unwrap()), None);
    }
}
