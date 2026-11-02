#![allow(clippy::cast_lossless)] // Hits predate the gate
#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)] // Hits predate the gate
#![allow(clippy::cast_precision_loss)] // Hits predate the gate
#![allow(clippy::cast_sign_loss)] // Hits predate the gate
#![allow(clippy::expect_used)] // Hits predate the gate
#![allow(clippy::string_slice)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate
#![allow(
    unused_imports,
    unused_variables,
    unused_mut,
    unreachable_code,
    dead_code
)]
#![warn(unreachable_pub)]
#![recursion_limit = "256"]
#![deny(clippy::indexing_slicing)]
#[cfg(all(test, feature = "dhat-heap"))]
#[global_allocator]
static DHAT_ALLOC: dhat::Alloc = dhat::Alloc;
pub(crate) use xai_grok_telemetry::unified_log;
pub use xai_tracing_macros::{teprintln, timed, tprintln};
pub mod agent;
pub mod auth {
    pub use crate::agent::init::run_cli_logout;
    pub use xai_grok_shell_core::auth::*;
}
pub use xai_grok_bundle as bundle;
pub use xai_grok_shell_assets::builtin;
#[cfg(all(test, feature = "config-docs"))]
pub mod config_docs;
pub use xai_grok_shell_base::cpu_profile;
pub use xai_grok_shell_base::env;
pub mod extensions;
pub use xai_grok_foreign_sessions as foreign_sessions;
pub use xai_grok_http as http;
pub mod leader {
    pub use xai_grok_shell_core::leader::*;
    #[cfg(feature = "test-support")]
    pub mod in_process;
}
pub use xai_grok_models as models;
pub mod session;
#[doc(hidden)]
pub use xai_grok_shell_core::waterfall;
pub use xai_grok_shell_core::{
    claude_import, claude_import_state, cli_models, config, credential_factory, inspect,
    instrumentation, instrumentation_timer, managed_config, mcp_doctor, plugin, relay, remote,
    sampling, tier, tools, upload, util,
};
pub use xai_grok_shell_terminal as terminal;
#[cfg(test)]
pub(crate) mod test_support;
