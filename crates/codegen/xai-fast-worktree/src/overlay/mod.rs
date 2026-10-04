//! Overlay-on-FUSE worktree support.

pub(crate) mod detect;
pub(crate) mod snapshot;

pub(crate) use detect::{OverlayInfo, detect_fuse_overlay};
pub(crate) use snapshot::{
    cleanup_orphaned_overlay_snapshots, create_overlay_worktree, remove_overlay_worktree,
    try_remove_from_metadata, try_remove_from_mountinfo,
};
