#![allow(clippy::cast_lossless)] // Hits predate the gate
#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)] // Hits predate the gate
#![allow(clippy::expect_used)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate
#![deny(clippy::indexing_slicing)]

pub mod appearance;
pub mod clipboard;
pub mod glyphs;
pub mod host;
pub mod input;
pub mod link_opener;
mod location_path;
pub mod modal_window_state;
pub mod prompt_images;
pub mod render;
pub mod search;
pub mod syntax;
pub mod terminal;
pub mod theme;
pub mod util;
