#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
#![warn(unreachable_pub)]
#[cfg(all(test, feature = "dhat-heap"))]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;
pub(crate) use xai_grok_telemetry::unified_log;
pub use xai_tracing_macros::{teprintln, timed, tprintln};
pub mod active_sessions;
pub mod agent;
pub mod auth;
// `bundle` and `builtin` live in the `xai-grok-shell-assets` sub-crate (kept
// out of the shell crate so its giant test-harness rustc has a smaller
// monomorphization surface — part of the compile-RAM work). Re-exported here
// so existing `crate::bundle::*` / `crate::builtin::*` call sites are
// unchanged.
pub use xai_grok_shell_assets::bundle;
pub use xai_grok_shell_assets::builtin;
pub mod claude_import;
pub mod claude_import_state;
pub mod cli_models;
pub(crate) mod codex_provider;
pub mod config;
/// Stable ACP auth-method id for the additive Codex/ChatGPT provider.
pub const CODEX_AUTH_METHOD_ID: &str = codex_provider::AUTH_METHOD_ID;
pub use xai_grok_shell_base::cpu_profile;
pub use xai_grok_shell_base::env;
pub mod extensions;
pub use xai_grok_workspace::foreign_sessions;
pub mod heap_profile;
pub use xai_grok_http as http;
pub mod inspect;
pub mod instrumentation;
pub mod leader;
pub mod managed_config;
pub mod mcp_doctor;
pub use xai_grok_models as models;
pub mod plugin;
pub mod relay;
pub mod remote;
pub mod sampling;
pub mod session;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tier;
pub mod tools;
pub mod upload;
pub mod util;
