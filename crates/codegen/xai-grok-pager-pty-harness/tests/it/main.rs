//! All integration tests of this crate, linked as one binary. Each former
//! test target is a module.

// The `pty_e2e_*` families share these helpers. Each family uses only part of them.
#[allow(dead_code, unused_imports)]
#[path = "pty_e2e/common.rs"]
mod common;
#[allow(dead_code, unused_imports)]
#[path = "pty_e2e/scroll.rs"]
mod scroll;

mod doctor_early_dispatch;
mod empty_enter_send_now;
mod env_op_compile;
mod exit_error_dead_stderr;
mod exit_timeout;
mod leader_pty_e2e;
mod live_model;
mod mcp_toggle_cli;
mod orphan_reap;
mod plan_approval_resume;
mod privacy_banner_e2e;
mod prompt_history_browse_order;
mod prompt_history_durable_quit;
mod prompt_stash;
mod pty_auto_mode;
mod pty_e2e_clipboard;
mod pty_e2e_config_ui;
mod pty_e2e_persistence;
mod pty_e2e_queue;
mod pty_e2e_scroll_selection;
mod pty_e2e_shell_tools;
mod pty_e2e_smoke;
mod pty_xtversion;
mod scripted_scenarios;
mod scroll_correctness_ptyctl;
mod scroll_matrix_curated;
mod settings_locked_row_e2e;
mod x10_mouse_leak;
