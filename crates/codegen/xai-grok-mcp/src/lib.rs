#![allow(clippy::cast_possible_truncation)] // Hits predate the gate
// lib.rs holds the lib code.

#![allow(clippy::expect_used)]
#![allow(clippy::string_slice)] // Hits predate the gate
#![allow(clippy::unwrap_used)] // Hits predate the gate
//! Two responsibilities:
//!
//! - **Wraps `rmcp`.** rmcp's HTTP goes through `xai_grok_net` ([`net_http_client`]).
//!   Consumers reach `rmcp` model types through this namespace
//!   (`xai_grok_mcp::rmcp::*`).
//!
//! 2. **Owns MCP-specific integration code**:
//!    - [`credentials`]: on-disk `$GROK_HOME/mcp_credentials.json` store and the rmcp `CredentialStore` adapter.
//!    - `auth_status`: decides auth for HTTP servers from what is on disk.
//!    - [`oauth`]: browser-based OAuth flow with cross-process and in-process dedup.
//!    - [`oauth_config`]: BYO OAuth config types parsed out of `config.toml`.
//!    - [`servers`]: MCP transport layer (rmcp's `StreamableHttpClientTransport` and `TokioChildProcess`).
//!      It also owns client lifecycle, tool invocation, error classification, and managed-MCP refresh.
//!    - [`mcp_http_client`]: backoff wrapper around the HTTP client handed to rmcp's streamable-HTTP transport.
//!      It works around rmcp's zero-backoff SSE reconnect loop.

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
pub mod net_http_client;
pub mod oauth;
pub mod oauth_config;
pub mod owned_clients;
pub mod servers;
pub mod shared_mcp_state;
mod tool_name;
pub mod wire;
