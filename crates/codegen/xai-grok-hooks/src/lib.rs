#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::expect_used)]

//!
#![deny(clippy::indexing_slicing)]

pub mod config;
pub mod discovery;
pub mod dispatcher;
mod env_expand;
pub mod error;
pub mod event;
pub mod matcher;
pub mod result;
pub mod runner;
#[cfg(test)]
mod test_support;
pub mod trust;
