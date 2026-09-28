#![allow(clippy::cast_possible_truncation)] // 1 hit predates the gate
#![allow(clippy::cast_possible_wrap)] // 2 hits predate the gate
#![allow(clippy::cast_precision_loss)] // 2 hits predate the gate
#![allow(clippy::string_slice)] // 1 hit predates the gate

//! Foundation modules shared by the grok shell crate family. Extracted from
//! `xai-grok-shell` (which re-exports them at their original paths) so they
//! build in parallel and stop rebuilding on shell edits.

pub mod cpu_profile;
pub mod env;
pub mod util;
