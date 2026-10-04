#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
//! Session-support modules extracted from `xai-grok-shell`'s `session/` tree so they build in parallel and stop rebuilding.
#![deny(clippy::indexing_slicing)]
pub mod managed_mcp;
