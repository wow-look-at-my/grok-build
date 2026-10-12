#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::string_slice)]
#![allow(clippy::unwrap_used)] // Hits predate the gate

//! ptyctl — Headless PTY controller built on alacritty_terminal.
//!
//! Provides programmatic control of terminal sessions: spawn processes
//! in a PTY, send keystrokes, read screen content as text/styled/HTML,
//! and expose it all via HTTP REST API.

pub mod keys;
pub mod pty;
pub mod server;
pub mod session;
pub mod styled;
pub mod term;
pub mod wait;
