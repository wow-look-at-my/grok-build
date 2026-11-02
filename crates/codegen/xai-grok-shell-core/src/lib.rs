//! The lower layers of the grok shell: config, persistence and storage, remote clients, the leader, and the session types the actor drives.
//! `xai-grok-shell` holds the session actor and the agent, and re-exports these modules at their old paths.
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
#![recursion_limit = "256"]
#![deny(clippy::indexing_slicing)]
pub use xai_grok_telemetry::unified_log;
pub use xai_tracing_macros::{teprintln, timed, tprintln};
pub mod agent;
pub mod auth {
    pub use crate::credential_factory::{
        build_bootstrap_otel_credentials, build_storage_client_for_proxy,
    };
    pub use xai_grok_login::*;
}
pub use xai_grok_bundle as bundle;
pub use xai_grok_shell_assets::builtin;
pub mod claude_import;
pub mod claude_import_state;
pub mod cli_models;
pub mod config;
pub mod credential_factory;
pub use xai_grok_shell_base::cpu_profile;
pub use xai_grok_shell_base::env;
pub mod extensions;
pub use xai_grok_foreign_sessions as foreign_sessions;
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
pub use xai_grok_shell_terminal as terminal;
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;
pub mod tier;
pub mod tools;
pub mod upload;
pub mod util;
#[doc(hidden)]
pub mod waterfall;
