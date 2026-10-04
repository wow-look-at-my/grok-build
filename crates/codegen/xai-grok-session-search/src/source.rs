//! The interface between the index and whatever owns the sessions on disk.

use std::io;
use std::path::{Path, PathBuf};

/// The fields of a stored session that the index actually indexes.
#[derive(Debug, Clone)]
pub struct IndexableSession {
    pub session_id: String,
    pub cwd: String,
    /// Last-modified stamp, unix seconds; the recency sort key.
    pub updated_at_unix: i64,
    /// Display title, already resolved by the store (generated title first, falling back to the session summary).
    pub title: String,
    /// Transcript to extract searchable text from, or `None` when the store does not expose one (such a session is indexed title-only).
    pub updates_path: Option<PathBuf>,
}

/// Read-only enumeration of the local session store.
#[async_trait::async_trait]
pub trait SessionSource: Send + Sync {
    /// Every session under this store, in no particular order.
    async fn list_sessions(&self) -> io::Result<Vec<IndexableSession>>;

    /// One session by identity.
    async fn load_session(
        &self,
        session_id: &str,
        cwd: &str,
    ) -> io::Result<Option<IndexableSession>>;
}

/// Opens the session store rooted at one grok home.
pub type SessionSourceFactory = fn(PathBuf) -> Box<dyn SessionSource>;

/// Blocking extraction of a transcript's searchable text plus the bytes read. Always called from a blocking thread.
pub type ContentExtractor = fn(&Path) -> io::Result<(String, u64)>;
