#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::expect_used)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate

//! # xai-codebase-graph High-performance code graph generation using tree-sitter queries. This crate provides.

#![deny(clippy::indexing_slicing)]

pub mod index_manager;
pub mod interner;
pub mod languages;
pub mod manager;
pub mod navigation;
pub mod scope_graph;
pub mod types;

// Re-exports for convenient access
pub use index_manager::{
    FileEvent, FileEventKind, IndexCommand, IndexManager, IndexManagerConfig, IndexManagerHandle,
    MAX_INDEXABLE_FILE_SIZE, QueryError, QueryResult, SymbolLocation, is_binary_content,
};
pub use languages::{LanguageRegistry, TSLanguageConfig};
pub use manager::{
    CACHE_FILE_NAME, CacheError, IndexBuilder, IndexError, IndexOperation, LockResult,
    WorkspaceLockGuard, cache_exists, cache_size, get_cache_path, is_operation_in_progress,
    load_index, save_index, save_index_async, try_lock,
};
pub use navigation::{Location, NavigationError, NavigationResult, Navigator};
pub use scope_graph::{
    LocalDef, LocalImport, LocalScope, NodeKind, QueryVersion, Reference, ScopeGraph,
    ScopeGraphIndex, ScopeGraphResult, Symbol, SymbolId, build_scope_graph, extract_symbols_fast,
};
pub use types::{FileMeta, IndexStats, Position, Range, SymbolAlias, SymbolOccurrence};

// String interning for memory-efficient storage
pub use interner::{StringId, StringInterner};
