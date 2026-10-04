//! Full-text search over local grok sessions.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::expect_used)]
#![deny(clippy::indexing_slicing)]

mod bootstrap;
mod db;
mod doc;
pub mod fts;
mod manager;
mod recovery;
mod source;

pub use manager::{
    SearchIndexManager, SearchIndexStatus, SessionSearchRequest, SessionSearchResponse,
    evict_session, execute_search,
};
pub use source::{ContentExtractor, IndexableSession, SessionSource, SessionSourceFactory};
