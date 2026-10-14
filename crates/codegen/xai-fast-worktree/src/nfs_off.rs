//! Grove worktree operations. Create, remove, and status decline in this build.
#![allow(dead_code)]

use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use anyhow::Result;

pub(crate) use crate::grove_api::is_safe_worktree_id;
pub use crate::grove_api::{
    CAP_CANCEL_WORKTREE_CREATE, CAP_FORK_FROM_BACKING, CleanArtifactsReply, DetachReply,
    GroveHardFail, NfsAdopted, NfsCreateDecision, NfsStatusView, NfsWorktreeOpts, SalvageReply,
    daemon_capability_class, grove_hard_fail,
};
#[allow(unused_imports)] // re-exported for discovery / execute when those modules are on
pub(crate) use crate::grove_api::{default_grove_creation_mode, nfs_error_blocks_fallback};

pub const WORKTREE_BACKING_DIR: &str = "worktree-backing";

pub fn dest_is_nfs_mount(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
fn folded_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

#[cfg(windows)]
pub(crate) fn dest_paths_equivalent(a: &Path, b: &Path) -> bool {
    folded_components(a) == folded_components(b)
}

#[cfg(windows)]
pub(crate) fn dest_path_contains(parent: &Path, child: &Path) -> bool {
    let parent = folded_components(parent);
    let child = folded_components(child);
    child.len() >= parent.len() && child[..parent.len()] == parent[..]
}

/// macOS exposes `/tmp`, `/var`, `/etc` and `/tftpboot` as symlinks into `/private/`.
#[cfg(target_os = "macos")]
const PRIVATE_ALIAS_DIRS: [&str; 4] = ["tmp", "var", "etc", "tftpboot"];

/// Rewrites a leading `/private/<alias>` to `/<alias>`. Both spellings of a
/// macOS alias land on the same lexical form.
#[cfg(target_os = "macos")]
fn fold_private_alias(path: &Path) -> PathBuf {
    let mut components = path.components();
    let (Some(std::path::Component::RootDir), Some(first), Some(second)) =
        (components.next(), components.next(), components.next())
    else {
        return path.to_path_buf();
    };
    if first.as_os_str() != "private"
        || !PRIVATE_ALIAS_DIRS.contains(&second.as_os_str().to_string_lossy().as_ref())
    {
        return path.to_path_buf();
    }
    let mut folded = PathBuf::from("/");
    folded.push(second.as_os_str());
    folded.extend(components);
    folded
}

#[cfg(target_os = "macos")]
pub(crate) fn dest_paths_equivalent(a: &Path, b: &Path) -> bool {
    fold_private_alias(a) == fold_private_alias(b)
}

#[cfg(target_os = "macos")]
pub(crate) fn dest_path_contains(parent: &Path, child: &Path) -> bool {
    fold_private_alias(child).starts_with(fold_private_alias(parent))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn dest_paths_equivalent(a: &Path, b: &Path) -> bool {
    a == b
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn dest_path_contains(parent: &Path, child: &Path) -> bool {
    child.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn dest_paths_match_across_the_private_alias() {
        assert!(dest_path_contains(
            Path::new("/tmp/nfs-wt"),
            Path::new("/private/tmp/nfs-wt/sub")
        ));
        assert!(dest_path_contains(
            Path::new("/private/var/lib/wt"),
            Path::new("/var/lib/wt/inner")
        ));
        assert!(dest_paths_equivalent(
            Path::new("/tmp/nfs-wt"),
            Path::new("/private/tmp/nfs-wt")
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_alias_folding_stays_within_the_named_directory() {
        assert!(!dest_path_contains(
            Path::new("/private/tmp/wt"),
            Path::new("/private/tmp/wt2/sub")
        ));
        assert!(!dest_path_contains(
            Path::new("/tmp/wt"),
            Path::new("/private/tmpfoo/wt/sub")
        ));
        assert!(!dest_paths_equivalent(
            Path::new("/tmp/wt"),
            Path::new("/private/home/wt")
        ));
    }
}

#[derive(Debug, Clone)]
pub struct NfsWorktreeClient;

impl NfsWorktreeClient {
    #[must_use]
    pub fn from_opts(_opts: &NfsWorktreeOpts) -> Self {
        Self
    }

    pub fn detach_worktree(&self, _dest: &Path, _allow_copy: bool) -> Result<DetachReply> {
        anyhow::bail!("not available in this build")
    }

    pub fn salvage_worktree(&self, _dest: &Path, _out: &Path) -> Result<SalvageReply> {
        anyhow::bail!("not available in this build")
    }

    pub fn clean_artifacts(&self, _dest: &Path) -> Result<CleanArtifactsReply> {
        anyhow::bail!("not available in this build")
    }

    pub fn redirect_events(
        &self,
        _ack_through: u64,
        _timeout: std::time::Duration,
    ) -> Result<(Vec<serde_json::Value>, u64)> {
        anyhow::bail!("not available in this build")
    }

    pub fn status_for_dir(&self, _dest: &Path) -> Option<NfsStatusView> {
        None
    }

    pub fn source_is_linked_local_view(&self, _source: &Path) -> bool {
        false
    }
}

#[must_use]
pub fn source_is_linked_local_view(_opts: &NfsWorktreeOpts, _source: &Path) -> bool {
    false
}

pub fn source_keeps_grove_create(_opts: &NfsWorktreeOpts, _source: &Path) -> bool {
    false
}

pub fn try_nfs_remove(_worktree_path: &Path) -> Result<Option<crate::RemoveReport>> {
    Ok(None)
}

#[must_use]
pub(crate) fn probe_daemon_capability_class(
    _opts: Option<&NfsWorktreeOpts>,
) -> Option<&'static str> {
    None
}

pub fn dest_is_known_unmounted(_path: &Path) -> bool {
    true
}

pub fn dest_is_mountpoint(_path: &Path) -> bool {
    false
}

pub fn dest_is_projected_mount(_path: &Path) -> bool {
    false
}

pub fn dest_is_grove_projection(_path: &Path) -> bool {
    false
}

#[must_use]
pub fn source_is_grove_parent(_path: &Path) -> bool {
    false
}

#[cfg(feature = "metadata")]
mod metadata {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use anyhow::Result;

    use crate::db::WorktreeRecord;

    pub const RANK_DB: u8 = 0;

    #[derive(Debug, Clone)]
    pub struct NfsIdentity {
        pub worktree_id: String,
        pub dest: Option<PathBuf>,
        pub source_repo: Option<PathBuf>,
        pub pin_ref: Option<String>,
        pub backing: Option<PathBuf>,
        pub mount_id: Option<i64>,
        pub rank: u8,
        pub phase: Option<String>,
    }

    #[derive(Debug, Default)]
    pub struct PinGcReport {
        pub examined: u64,
        pub pruned: u64,
        pub deferred_grace: u64,
        pub kept_live: u64,
        pub pruned_ids: Vec<String>,
    }

    pub fn candidate_data_dirs() -> Vec<PathBuf> {
        Vec::new()
    }

    pub fn nfs_record_is_dead(dest: &Path, _backing: Option<&Path>) -> bool {
        if crate::nfs::dest_is_mountpoint(dest) || !crate::nfs::dest_is_known_unmounted(dest) {
            return false;
        }
        std::fs::symlink_metadata(dest).is_err()
    }

    pub fn identities_from_worktree_records(_recs: &[WorktreeRecord]) -> Vec<NfsIdentity> {
        Vec::new()
    }

    pub fn collect_identities(
        _data_dir: &Path,
        _worktrees: &[NfsIdentity],
    ) -> HashMap<String, NfsIdentity> {
        HashMap::new()
    }

    pub fn merge_nfs_identities(
        _into: &mut HashMap<String, NfsIdentity>,
        _src: impl IntoIterator<Item = NfsIdentity>,
    ) {
    }

    pub fn gc_orphan_pins(
        _data_dir: &Path,
        _worktrees: &[NfsIdentity],
        _now: i64,
        _dry_run: bool,
    ) -> Result<PinGcReport> {
        Ok(PinGcReport::default())
    }
}

#[cfg(feature = "metadata")]
pub use metadata::*;
