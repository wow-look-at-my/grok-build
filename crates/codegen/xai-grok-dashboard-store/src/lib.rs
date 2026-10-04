//! SQLite-backed persistent dashboard workspace: membership, layout ranks, and grouping.

#![deny(clippy::indexing_slicing)]
#![allow(clippy::cast_possible_wrap)]

use std::path::{Path, PathBuf};

mod error;
mod owner_only;
mod schema;
mod store;
mod store_open;
#[cfg(test)]
mod test_support;
mod types;

pub use error::{Result, StoreError};
pub use schema::USER_VERSION;
pub use store::WorkspaceStore;
pub use types::{
    Grouping, InsertOutcome, LayoutApplyOutcome, LayoutGrouping, LayoutPatch, MAX_CWD_BYTES,
    MAX_ENUM_BYTES, MAX_MODEL_BYTES, MAX_SESSION_ID_BYTES, MAX_SUMMARY_BYTES, MAX_TITLE_BYTES,
    Member, MemberKey, MemberKind, MemberMetadata, MemberOrigin, NewMember, PinAssignment,
    RANK_GAP, RankAssignment, RekeyOutcome, RemoveOutcome, SchemaState, SessionId, UnknownGrouping,
    UnknownMemberKind, UnknownMemberOrigin, WORKSPACE_CAPACITY, WorkspaceSnapshot,
};

/// The canonical store path under a grok home.
pub fn default_db_path(grok_home: &Path) -> PathBuf {
    grok_home.join("dashboard").join("workspace.db")
}
