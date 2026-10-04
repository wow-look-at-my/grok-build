//! Memory system shim.

pub(crate) mod capture_transcript;
pub mod hooks;
pub(crate) mod v2_capture;

pub use xai_grok_memory::{
    EndpointScopedCredentials, MemoryBackendImpl, MemoryBackendParams, MemoryIndex, MemoryScope,
    MemorySearchSource, MemoryStorage, V2ManifestBudget, V2MemoryAccessPolicy, V2MemoryScope,
    archive, backend, chunker, dream, dream_lock, embed_missing_chunks, embedding, index,
    init_sqlite_vec, mmr, noop_memory_observation_sink, query_expansion, regenerate_scope_manifest,
    schema, search, storage, text_utils, v2, watcher,
};
