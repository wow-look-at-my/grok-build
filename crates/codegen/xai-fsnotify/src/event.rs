//! Public event types — the wire contract for `xai-fsnotify`.

use std::path::PathBuf;

/// One semantic event from the local workspace. Causal order on the source's broadcast channel.
/// `FilesChanged` paths share a single `kind` (per-debounce-window grouping).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FsEvent {
    /// Workspace file changes; all paths share `kind`.
    FilesChanged {
        paths: Vec<PathBuf>,
        kind: FsEventKind,
    },

    /// A git metadata file changed (HEAD, index, refs/, FETCH_HEAD).
    GitMetaChanged { kind: GitMetaKind },

    /// VCS lock activity observed, or an event for a lock that is already gone (fast ops finish inside one batch).
    GitOperationStarted,

    /// The lock has been gone for [`crate::SETTLE_MS`]: rapid lock cycles merge into one operation.
    GitOperationCompleted { head_changed: bool },
}

/// Aligned with `xai_grok_workspace_types::FsEventKind` (identity map at the
/// workspace boundary).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FsEventKind {
    Created,
    #[default]
    Modified,
    Removed,
    Renamed,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum GitMetaKind {
    /// `.git/HEAD` (branch switch, commit, rebase step).
    HeadChanged,
    /// `.git/index` (`git add`, `git reset`, `git commit`).
    IndexChanged,
    /// `.git/refs/*` or `.git/packed-refs` (ref updates).
    RefsChanged,
    /// `.git/FETCH_HEAD` (fetch / pull).
    FetchHeadChanged,
}
