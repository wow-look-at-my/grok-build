//! BTRFS snapshot support for fast worktree creation.

pub mod detect;
pub mod snapshot;

pub use detect::{BtrfsInfo, is_btrfs, is_btrfs_subvolume};
pub use snapshot::{
    BTRFS_META_SUFFIX, BTRFS_SNAPSHOT_SUBDIRS, BtrfsSnapshotMetadata, SnapshotMetaState,
    btrfs_meta_path, create_snapshot, create_snapshot_with_symlink, create_worktree_symlink,
    delete_snapshot, is_safe_snapshot_delete_target, remove_btrfs_metadata, snapshot_dest_path,
    snapshot_meta_state, snapshot_meta_targets, write_btrfs_metadata,
};
