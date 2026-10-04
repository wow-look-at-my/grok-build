//! Ephemeral tips: one hint line at a time, rendered in the banner rect above the prompt input and cleared after a TTL.

pub mod clear_detector;
pub mod clipboard_focus;
pub mod ephemeral;
pub mod export_copy;
pub mod plan_nudge;
pub mod render;
pub mod send_now;
pub mod small_screen;
pub mod ssh_wrap;
pub mod word_select;

pub use ephemeral::{DEFAULT_TIP_TICKS, EphemeralTip, EphemeralTipState, tip_row_renderable};
