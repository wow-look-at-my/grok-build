#![allow(clippy::cast_lossless)] // 100 hits predate the gate
#![allow(clippy::cast_possible_truncation)] // 160 hits predate the gate
#![allow(clippy::cast_possible_wrap)] // 35 hits predate the gate
#![allow(clippy::cast_precision_loss)] // 78 hits predate the gate
#![allow(clippy::cast_sign_loss)] // 66 hits predate the gate
#![allow(clippy::expect_used)] // 8 hits predate the gate
#![allow(clippy::string_slice)] // 30 hits predate the gate
#![allow(clippy::unwrap_used)] // 6 hits predate the gate

pub mod appearance;
pub mod clipboard;
pub mod gboom;
pub mod glyphs;
pub mod host;
pub mod link_opener;
pub mod modal_window_state;
pub mod prompt_images;
pub mod render;
pub mod syntax;
pub mod terminal;
pub mod theme;
pub mod util;
