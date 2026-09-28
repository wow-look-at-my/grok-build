#![allow(clippy::cast_possible_truncation)] // 1 hit predates the gate
#![allow(clippy::string_slice)] // 2 hits predate the gate
#![allow(clippy::unwrap_used)] // 7 hits predate the gate

pub mod auto_update;
pub mod version;
mod version_policy;

pub use auto_update::UpdateStatus;
pub use version::{UpdateConfig, channel_label, channel_name, write_version_cache};
pub use version_policy::enforce_version_policy_or_exit;
