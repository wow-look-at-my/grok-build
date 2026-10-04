//! Inter-compaction — the chunked summarisation pipeline shared by both `Basic` and `DivideAndConquer` strategies.

pub mod compact;
pub mod config;
pub mod observer;

pub use compact::{ChunkedCompactionOutput, sample_compaction_chunked};
pub use config::InterCompactionConfig;
pub use observer::InterCompactionObserver;
