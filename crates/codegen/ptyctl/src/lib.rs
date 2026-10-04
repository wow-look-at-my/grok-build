#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::string_slice)]
#![allow(clippy::unwrap_used)] // Hits predate the gate

//! ptyctl — Headless PTY controller built on alacritty_terminal.

pub mod keys;
pub mod pty;
pub mod server;
pub mod session;
pub mod styled;
pub mod term;
pub mod wait;
