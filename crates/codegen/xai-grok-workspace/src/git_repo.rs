//! Repository discovery and ref reads through `gix`.

use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;

/// The result of a failed [`discover`].
#[derive(Debug)]
pub enum DiscoverError {
    /// No repository contains the path.
    NotARepo(anyhow::Error),
    /// A repository may contain the path, but it cannot be opened.
    Failed(anyhow::Error),
}

impl DiscoverError {
    /// The underlying error, whatever the kind.
    pub fn into_error(self) -> anyhow::Error {
        match self {
            Self::NotARepo(e) | Self::Failed(e) => e,
        }
    }
}

/// Search upwards from `path` for a repository, across filesystem boundaries.
/// The repository has no `filter.<driver>` config, so no read through it runs a clean or smudge command.
pub fn discover(path: &Path) -> Result<gix::Repository, DiscoverError> {
    without_filter_drivers(discover_with_filter_drivers(path)?).map_err(DiscoverError::Failed)
}

/// [`discover`], with the `filter.<driver>` config kept. Use it only to read the config itself.
pub fn discover_with_filter_drivers(path: &Path) -> Result<gix::Repository, DiscoverError> {
    let options = gix::discover::upwards::Options {
        cross_fs: true,
        ..Default::default()
    };
    let repo: gix::Repository =
        match gix::ThreadSafeRepository::discover_opts(path, options, Default::default()) {
            Ok(repo) => repo.into(),
            Err(gix::discover::Error::Discover(
                e @ (gix::discover::upwards::Error::NoGitRepository { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinCeiling { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinFs { .. }
                | gix::discover::upwards::Error::NoTrustedGitRepository { .. }),
            )) => return Err(DiscoverError::NotARepo(e.into())),
            Err(e) => return Err(DiscoverError::Failed(e.into())),
        };
    check_format(&repo).map_err(DiscoverError::Failed)?;
    Ok(repo)
}

/// Open the repository at `path` (a work tree or a git dir) without an upward
/// search.
pub fn open(path: &Path) -> anyhow::Result<gix::Repository> {
    let repo = gix::open(path)?;
    check_format(&repo)?;
    without_filter_drivers(repo)
}

/// Remove every `filter.<driver>` section from the in-memory config.
/// A status or blob read then cannot spawn a repository-configured program such as `git-lfs`.
fn without_filter_drivers(mut repo: gix::Repository) -> anyhow::Result<gix::Repository> {
    let ids: Vec<_> = repo
        .config_snapshot()
        .plumbing()
        .sections_and_ids_by_name("filter")
        .into_iter()
        .flatten()
        .map(|(_, id)| id)
        .collect();
    if ids.is_empty() {
        return Ok(repo);
    }
    let mut config = repo.config_snapshot_mut();
    for id in ids {
        config.remove_section_by_id(id);
    }
    config.commit()?;
    Ok(repo)
}

/// The extensions a repository may name and still be opened.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "noop",
    "objectformat",
    "worktreeconfig",
    "preciousobjects",
    "relativeworktrees",
];

/// Refuse a repository format this crate does not read.
fn check_format(repo: &gix::Repository) -> anyhow::Result<()> {
    let config = repo.config_snapshot();
    let version = config.integer("core.repositoryformatversion").unwrap_or(0);
    if version < 0 {
        anyhow::bail!("invalid repository version {version}");
    }
    if version > 1 {
        anyhow::bail!(
            "unsupported repository version {version}; only versions up to 1 are supported"
        );
    }
    if version < 1 {
        return Ok(());
    }
    let Some(sections) = config.plumbing().sections_by_name("extensions") else {
        return Ok(());
    };
    for section in sections {
        for name in section.body().value_names() {
            let name = name.to_str_lossy().to_ascii_lowercase();
            if !SUPPORTED_EXTENSIONS.contains(&name.as_str()) {
                anyhow::bail!("unsupported extension name extensions.{name}");
            }
        }
    }
    Ok(())
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The canonical work tree root, or `None` for a bare repository.
pub fn workdir(repo: &gix::Repository) -> Option<PathBuf> {
    repo.workdir().map(canonical)
}

/// The canonical git dir. A linked worktree has its own, under the common dir.
pub fn git_dir(repo: &gix::Repository) -> PathBuf {
    canonical(repo.git_dir())
}

/// The canonical common dir, which the main checkout and all its linked worktrees share.
pub fn common_dir(repo: &gix::Repository) -> PathBuf {
    canonical(repo.common_dir())
}

/// Whether `repo` is a linked worktree.
pub fn is_linked_worktree(repo: &gix::Repository) -> bool {
    git_dir(repo) != common_dir(repo)
}

/// The commit id HEAD names, read from refs only.
/// It never loads the object, so it can name one that this repository does not have.
/// `None` when HEAD is unborn or cannot be read.
pub fn head_id(repo: &gix::Repository) -> Option<gix::ObjectId> {
    match repo.head() {
        Ok(head) => match head.kind {
            gix::head::Kind::Symbolic(reference) => {
                reference.target.try_id().map(ToOwned::to_owned)
            }
            gix::head::Kind::Detached { target, .. } => Some(target),
            gix::head::Kind::Unborn(_) => None,
        },
        Err(e) => {
            tracing::debug!(
                path = %repo.git_dir().display(),
                error = %e,
                "git.head: unresolvable",
            );
            None
        }
    }
}

/// The short name of the checked-out branch, `"HEAD"` when detached, `None` when unborn or unreadable.
pub fn head_shorthand(repo: &gix::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    match head.kind {
        gix::head::Kind::Symbolic(reference) => Some(reference.name.shorten().to_string()),
        gix::head::Kind::Detached { .. } => Some("HEAD".to_owned()),
        gix::head::Kind::Unborn(_) => None,
    }
}

/// The full name of the ref that `branch` tracks, from `branch.<name>.remote` and `branch.<name>.merge`.
/// A branch whose remote is `.` tracks a local branch.
pub fn upstream_ref_name(
    repo: &gix::Repository,
    branch: &gix::refs::FullNameRef,
) -> Option<gix::refs::FullName> {
    let short = branch.shorten();
    let config = repo.config_snapshot();
    let remote = config.string(format!("branch.{short}.remote").as_str())?;
    if remote.as_ref() == "." {
        let merge = config.string(format!("branch.{short}.merge").as_str())?;
        return gix::refs::FullName::try_from(merge.into_owned()).ok();
    }
    let tracking = repo.branch_remote_tracking_ref_name(branch, gix::remote::Direction::Fetch)?;
    tracking.ok().map(|name| name.into_owned())
}

/// The commits only `local` reaches and the commits only `upstream` reaches, counted.
pub fn graph_ahead_behind(
    repo: &gix::Repository,
    local: gix::ObjectId,
    upstream: gix::ObjectId,
) -> Option<(usize, usize)> {
    let count = |tip: gix::ObjectId, hidden: gix::ObjectId| -> Option<usize> {
        let walk = repo.rev_walk([tip]).with_hidden([hidden]).all().ok()?;
        let mut n = 0;
        for info in walk {
            info.ok()?;
            n += 1;
        }
        Some(n)
    };
    Some((count(local, upstream)?, count(upstream, local)?))
}

/// The fetch URL of every remote, in remote-name order, after `insteadOf` rewrites.
pub fn remote_urls(repo: &gix::Repository) -> Vec<String> {
    repo.remote_names()
        .iter()
        .filter_map(|name| remote_url(repo, name.as_ref()))
        .collect()
}

/// The fetch URL of the remote called `name`.
pub fn remote_url(repo: &gix::Repository, name: &gix::bstr::BStr) -> Option<String> {
    let remote = repo.find_remote(name).ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    url.to_bstring().to_str().ok().map(str::to_owned)
}
