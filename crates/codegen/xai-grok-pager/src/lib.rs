<<<<<<< HEAD
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
//! xai-grok-pager: Grok Build TUI.
=======
#![allow(clippy::cast_lossless)] // 98 hits predate the gate
#![allow(clippy::cast_possible_truncation)] // 785 hits predate the gate
#![allow(clippy::cast_possible_wrap)] // 79 hits predate the gate
#![allow(clippy::cast_precision_loss)] // 65 hits predate the gate
#![allow(clippy::cast_sign_loss)] // 84 hits predate the gate
#![allow(clippy::expect_used)] // 134 hits predate the gate
#![allow(clippy::unwrap_used)] // 60 hits predate the gate

//! xai-grok-pager — Grok Build TUI.
>>>>>>> origin/master
//!
//! A clean-room implementation built on the v3 pager rendering engine.
#![allow(clippy::string_slice)]
#![deny(clippy::indexing_slicing)]
pub mod acp;
pub mod actions;
pub mod agent_runtime;
pub mod app;
<<<<<<< HEAD
pub mod best_effort_stderr;
=======
>>>>>>> origin/master
pub mod branch_stats;
pub mod ci_status;
pub mod client_identity;
pub mod completions_cmd;
mod config_toml_edit;
pub mod diagnostics;
pub mod disk_usage_cmd;
pub mod docs;
pub mod doctor_cmd;
pub mod export_cmd;
pub(crate) mod fs_size;
pub mod git_info;
pub mod headless;
pub mod hyperlink_route;
pub mod inline_media_ffmpeg;
pub mod input_log;
pub mod mcp_cmd;
pub mod memory_cmd;
pub mod memory_release;
pub mod memory_trace;
#[path = "minimal/api.rs"]
pub mod minimal_api;
#[path = "minimal/hook.rs"]
pub mod minimal_hook;
#[path = "minimal/reprint.rs"]
pub mod minimal_reprint;
pub mod models;
pub mod notifications;
#[allow(unused_imports, unused_macros)]
pub mod obf;
pub mod plugin_cmd;
pub mod pty_wrap;
pub mod recent_dirs;
pub mod scrollback;
pub mod sessions_cmd;
pub mod settings;
pub mod share_cmd;
pub mod signal_streams;
pub mod slash;
pub mod startup;
pub mod tips;
pub mod tool_usage;
pub mod tutorial_docs;
pub mod usage_cmd;
pub mod wrap_clipboard_image;
pub mod wrap_cmd;
pub(crate) mod wrap_filter;
pub(crate) mod wrap_restore;
pub use xai_grok_gboom as gboom;
pub use xai_grok_pager_render::key;
pub use xai_grok_pager_render::{
    appearance, clipboard, glyphs, host, input, link_opener, modal_window_state, prompt_images,
    render, search, syntax, terminal, theme, util,
};
#[cfg(test)]
pub mod test_util;
pub mod trace_cmd;
pub mod tracing;
pub mod unified_log;
pub mod views;
pub mod voice;
pub mod worktree_cmd;
