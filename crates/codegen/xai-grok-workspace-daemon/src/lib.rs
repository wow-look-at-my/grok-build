//! How the workspace-server daemonizes itself and supervises its preview-proxy child.

#![deny(clippy::indexing_slicing)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::unwrap_used)]

pub mod daemonize;
pub mod preview_supervisor;
