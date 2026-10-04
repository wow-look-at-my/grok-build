#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::expect_used)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate

//! Shared test utilities for xAI crates.

pub mod env;
pub mod git;
pub mod image;
pub mod runfiles_util;
pub mod tracing_capture;
