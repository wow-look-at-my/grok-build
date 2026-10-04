//! Time helpers shared across the crate's feature configurations.

use std::time::{SystemTime, UNIX_EPOCH};

/// This is the source for the timestamps the DB and the reclaim writer
/// store.
pub(crate) fn epoch_secs() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(secs).unwrap_or(i64::MAX)
}
