//! Syncs TUI sessions to the relay backend over WebSocket for cross-machine session persistence and real-time sharing.
pub mod sync;
pub mod types;

pub use sync::{ConnectionState, RelaySync, RelaySyncState, StatusCallback, SyncStatus};
pub use types::AgentType;
