#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
#![allow(clippy::expect_used)]
#![allow(clippy::string_slice)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate
#![allow(clippy::cast_possible_wrap)]
#![deny(clippy::indexing_slicing)]

pub use rmcp;

#[doc(hidden)]
pub fn isolate_grok_home_for_tests() {
    static HOME: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::TempDir::new().expect("test grok home").keep();
        // SAFETY: OnceLock-guarded single set; the concurrent env-read race is accepted in tests.
        unsafe { std::env::set_var("GROK_HOME", &dir) };
        let memo = xai_grok_config::grok_home();
        assert!(
            memo.starts_with(&dir),
            "grok-home memo was warmed before test isolation: {}",
            memo.display()
        );
    });
}

pub mod acp_transport;
mod auth_status;
mod call_result;
pub mod credentials;
pub mod elicitation;
mod generation;
pub mod liveness;
pub mod mcp_http_client;
pub mod oauth;
pub mod oauth_config;
pub mod owned_clients;
pub mod servers;
pub mod shared_mcp_state;
mod tool_name;
pub mod wire;
