//! The session actor. Everything else under `session` lives in `xai_grok_shell_core::session` and is re-exported here.
pub use xai_grok_shell_core::session::*;

pub(crate) mod acp_session;
pub use self::acp_session::*;
pub(crate) mod fs_watch;
#[cfg(test)]
mod mcp_dispatcher_e2e_tests;
#[cfg(feature = "test-support")]
pub mod testkit;
pub mod tool_definitions_artifact;
