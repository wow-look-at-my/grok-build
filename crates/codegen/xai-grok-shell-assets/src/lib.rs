//! Asset extraction and bundle management for the grok shell family.
//!
//! Extracted from `xai-grok-shell` into a separate crate so the shell crate's
//! single rustc test-harness compile has a smaller monomorphization surface
//! (part of the compile-RAM work; see the goal's plan/ram-agg.log). The shell
//! crate re-exports this crate's modules (`pub use xai_grok_shell_assets::...`)
//! so existing `crate::builtin::*` call sites are unchanged. The bundle cache
//! lives in `xai-grok-bundle`.
//!
//! `builtin` extracts the built-in metadata files (e.g. README) to `~/.grok/`.

pub mod builtin;
