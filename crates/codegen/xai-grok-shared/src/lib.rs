#![allow(clippy::cast_possible_truncation)] // 1 hit predates the gate
#![allow(clippy::cast_sign_loss)] // 2 hits predate the gate
#![allow(clippy::expect_used)] // 3 hits predate the gate
//! Shared utilities used by both `xai-grok-shell` and its downstream clients (e.g. `xai-grok-pager-render`).
//! This crate sits upstream of the tools and shell; keep client utilities independent of their runtimes.

#![deny(clippy::indexing_slicing)]

pub mod clipboard;
pub mod placeholder_images;
pub mod session;
pub mod stderr;
pub mod ui_config;

#[cfg(test)]
mod placeholder_image_format_tests;
