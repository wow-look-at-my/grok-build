//! Bounded advisory file locks for files under the grok home.

#![deny(clippy::indexing_slicing)]

mod error;
mod lock;
mod locked_file;
mod options;
mod slot;

pub use error::{LockError, Result};
pub use lock::lock_file;
pub use locked_file::LockedFile;
pub use options::{DEFAULT_SLOT_GRACE, LockOptions, SlotPolicy, Wait};
pub use slot::{SLOT_DIR_ENV, slot_path_in};
