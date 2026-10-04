#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
//! Foundation modules shared by the grok shell crate family.

#![deny(clippy::indexing_slicing)]

pub mod cpu_profile;
pub mod env;
pub mod util;
