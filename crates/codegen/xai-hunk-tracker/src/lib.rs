#![allow(clippy::cast_lossless)] // 2 hits predate the gate
#![allow(clippy::cast_possible_truncation)] // 1 hit predates the gate
#![allow(clippy::cast_possible_wrap)] // 6 hits predate the gate
#![allow(clippy::unwrap_used)] // 2 hits predate the gate
<<<<<<< HEAD
//! Track file hunks with agent vs external attribution.
=======

//! xai-hunk-tracker - Track file hunks (diffs) with agent/external attribution.
>>>>>>> origin/master
//!
//! `HunkTrackerActor` owns tracker state on a dedicated tokio task. Callers
//! send commands through [`HunkTrackerHandle`] and receive [`HunkEvent`]s.

#![deny(clippy::indexing_slicing)]

pub mod actor;
pub mod commands;
pub mod diff;
pub mod events;
pub mod handle;
pub mod loc;
pub mod types;

pub use actor::{HunkTrackerActor, REFRESH_SCAN_LOG_PREFIX, REFRESH_SKIP_LOG_PREFIX};
pub use events::{HunkEvent, HunkRemovalReason};
pub use handle::HunkTrackerHandle;
pub use loc::{
    AuthorType, EventType, HunkRecord, HunkRecordWriter, JsonlHunkRecordWriter, LocAggregate,
    LocSinkContext, SourceType, run_loc_sink,
};
pub use types::*;
