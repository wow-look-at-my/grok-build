#![allow(clippy::cast_possible_truncation)] // 1 hit predates the gate
#![allow(clippy::unwrap_used)] // 7 hits predate the gate
<<<<<<< HEAD
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::string_slice)]
#![deny(clippy::indexing_slicing)]
=======
>>>>>>> origin/master

pub mod auto_update;
mod cleanup_downloads;
pub mod version;
mod version_policy;
mod winget;

pub use auto_update::UpdateStatus;
pub use version::{UpdateConfig, channel_label, channel_name, write_version_cache};
pub use version_policy::enforce_version_policy_or_exit;
