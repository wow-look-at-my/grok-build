//! Memory subsystem shapes referenced from `OpsChunk::MemoryChunks`.

use serde::{Deserialize, Serialize};

/// One entry returned from a memory search.
///
/// Carries an optional `f32` `score`, which prevents a useful `Eq` derive (`f32` is `PartialEq` but not `Eq`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryChunk {
    /// Stable identifier (typically the memory entry's hash or path).
    pub id: String,
    /// Content body.
    #[serde(default)]
    pub content: String,
    /// Optional source path the chunk was derived from.
    #[serde(default)]
    pub source: Option<String>,
    /// Optional relevance score from the search backend.
    #[serde(default)]
    pub score: Option<f32>,
}
