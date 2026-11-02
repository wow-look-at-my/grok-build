#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
// lib.rs holds the lib code.

#![allow(clippy::unwrap_used)] // Hits predate the gate

//! Asset extraction and bundle management for the grok shell family.

pub mod builtin;
