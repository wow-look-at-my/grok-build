//! MCP server re-exports + shell-side wrappers for timeout override resolution.

pub use xai_grok_mcp::servers::{
    AcpServerEntry, HttpConfig, MCP_TOOL_NAME_DELIMITER, McpClient, McpClientTimeoutOverrides,
    McpConfigDiff, McpError, McpInitStrategy, McpMetaConfigMap, McpServerMetaConfig, McpServerName,
    McpService, McpSpawnCtx, McpState, McpTool, McpToolRegistration, OauthInteractivity,
    SharedMcpPool, mcp_server_name, mcp_target_str, mcp_transport_str, parse_mcp_meta_config,
    parse_mcp_tool_name, sanitize_descriptor_segment, validate_tool_name,
};

use std::collections::HashMap;
use std::path::Path;

use agent_client_protocol as acp;
use xai_grok_mcp::oauth_config::{McpOAuthConfig, McpOAuthConfigMap};
use xai_grok_mcp::servers as inner;

fn resolve_overrides(
    server_name: &str,
    cwd: Option<&Path>,
) -> Option<inner::McpClientTimeoutOverrides> {
    let config = match cwd {
        Some(cwd) => crate::util::config::get_mcp_server_config_with_project(server_name, cwd),
        None => crate::util::config::get_mcp_server_config(server_name),
    };
    // Fall back to the globally-resolved startup timeout so servers without a
    // per-server `startup_timeout_sec` (e.g. `~/.claude.json` imports) still get it.
    let global_startup = crate::util::config::resolved_mcp_startup_timeout_secs();
    Some(inner::McpClientTimeoutOverrides {
        startup_timeout_sec: config
            .as_ref()
            .and_then(|c| c.startup_timeout_sec)
            .or(Some(global_startup)),
        tool_timeout_sec: config.as_ref().and_then(|c| c.tool_timeout_sec),
        tool_timeouts: config.as_ref().and_then(|c| c.tool_timeouts.clone()),
        expose_image_base64: config.as_ref().and_then(|c| c.expose_image_base64),
    })
}

/// Build the config-resolved event data from a list of MCP server configs.
pub fn build_config_resolved_event(
    configs: &[acp::McpServer],
    cwd: &Path,
) -> xai_file_utils::events::Event {
    let disabled: Vec<String> = crate::util::config::disabled_mcp_server_names(cwd)
        .into_iter()
        .collect();
    let servers = configs
        .iter()
        .map(|c| xai_file_utils::events::McpConfigServer {
            name: inner::mcp_server_name(c).to_string(),
            transport: inner::mcp_transport_str(c).to_string(),
            source: if inner::mcp_server_name(c)
                .starts_with(crate::session::managed_mcp::MANAGED_MCP_PREFIX)
            {
                "managed"
            } else {
                "local"
            }
            .to_string(),
        })
        .collect();
    xai_file_utils::events::Event::McpConfigResolved { servers, disabled }
}

/// Outcome of [`wait_until_mcp_init_settles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpInitWait {
    /// Every server finished; tools are registered.
    Initialized,
    /// Nothing is in flight, and it never completed — the caller owns starting
    /// (or restarting) initialization.
    NotInitializing,
    /// The budget elapsed with initialization still in flight.
    TimedOut,
}

/// Poll `mcp_state` until initialization settles, giving up after `budget`.
///
/// The budget is the point of this function. A server that keeps failing gets
/// re-initialized, which puts the state back in flight, so a poll with no
/// deadline never returns — and its caller is a prompt waiting to run.
pub async fn wait_until_mcp_init_settles(
    mcp_state: &tokio::sync::Mutex<inner::McpState>,
    budget: std::time::Duration,
) -> McpInitWait {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        {
            let state = mcp_state.lock().await;
            if state.is_initialized() {
                return McpInitWait::Initialized;
            }
            if !state.is_initializing() {
                return McpInitWait::NotInitializing;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return McpInitWait::TimedOut;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

pub async fn start_mcp_server(
    mcp_server: acp::McpServer,
    cwd: Option<&Path>,
    meta_config: Option<&inner::McpServerMetaConfig>,
    byo_config: Option<&McpOAuthConfig>,
    ctx: &inner::McpSpawnCtx<'_>,
) -> Result<inner::McpClient, inner::McpError> {
    let overrides = resolve_overrides(inner::mcp_server_name(&mcp_server), cwd);
    inner::start_mcp_server(mcp_server, overrides.as_ref(), meta_config, byo_config, ctx).await
}

/// Build all pending MCP clients for one init pass as a single merged list: config-declared
/// servers (HTTP/stdio, spawned lock-free via [`start_mcp_servers`]) and SDK in-process
/// servers (built under a brief lock via `McpState::build_pending_acp_clients`). SDK clients
/// never fail to build, so they enter as `Ok`. One entry point so the init batch doesn't
/// invoke two builders.
pub async fn build_pending_clients(
    mcp_state: &tokio::sync::Mutex<inner::McpState>,
    configs_to_start: Vec<acp::McpServer>,
    cwd: Option<&Path>,
    meta_config_map: &inner::McpMetaConfigMap,
    oauth_config_map: &McpOAuthConfigMap,
    ctx: &inner::McpSpawnCtx<'_>,
) -> Vec<Result<inner::McpClient, inner::McpError>> {
    let mut results = start_mcp_servers(
        configs_to_start,
        cwd,
        meta_config_map,
        oauth_config_map,
        ctx,
    )
    .await;
    // Re-resolve SDK (ACP) config.toml overrides for THIS init, matching HTTP/stdio, so a
    // mid-session config change applies on the next init (resolved outside the lock — it
    // reads config.toml — then handed to the pure, under-lock builder).
    let acp_overrides: HashMap<String, inner::McpClientTimeoutOverrides> = {
        let names = mcp_state.lock().await.pending_acp_server_names();
        names
            .iter()
            .filter_map(|name| resolve_overrides(name, cwd).map(|o| (name.clone(), o)))
            .collect()
    };
    // Brief lock, no `.await` held: the SDK clients are built synchronously (pure).
    let acp_clients = mcp_state
        .lock()
        .await
        .build_pending_acp_clients(&acp_overrides);
    results.extend(acp_clients.into_iter().map(Ok));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str) -> acp::McpServer {
        acp::McpServer::Stdio(acp::McpServerStdio::new(
            name,
            std::path::PathBuf::from("uvx"),
        ))
    }

    /// A server that never finishes handshaking must not hold a caller
    /// forever. This is the failure an operator saw as a session that stopped
    /// answering prompts entirely and only came back on restart.
    #[tokio::test(start_paused = true)]
    async fn a_wedged_init_gives_up_instead_of_waiting_forever() {
        let mut state = inner::McpState::new(vec![stdio("kagi")]);
        assert!(state.try_start_init());
        state.mark_servers_initializing(["kagi".to_string()]);
        let state = tokio::sync::Mutex::new(state);

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(600),
            wait_until_mcp_init_settles(&state, std::time::Duration::from_secs(30)),
        )
        .await
        .expect("the wait must return on its own, not run until the test's own timeout");

        assert_eq!(outcome, McpInitWait::TimedOut);
    }

    /// The budget only bounds a wait that would otherwise never end: a server
    /// that finishes is still waited for.
    #[tokio::test(start_paused = true)]
    async fn a_server_that_finishes_is_waited_for() {
        let mut state = inner::McpState::new(vec![stdio("kagi")]);
        assert!(state.try_start_init());
        state.mark_servers_initializing(["kagi".to_string()]);
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(state));

        let settler = std::sync::Arc::clone(&state);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let mut state = settler.lock().await;
            state.mark_server_ready("kagi");
            state.finish_init();
        });

        assert_eq!(
            wait_until_mcp_init_settles(&state, std::time::Duration::from_secs(30)).await,
            McpInitWait::Initialized,
        );
    }

    /// Nothing in flight and never completed is the caller's cue to start
    /// initialization, not to wait out the budget.
    #[tokio::test(start_paused = true)]
    async fn an_unstarted_init_returns_immediately() {
        let state = tokio::sync::Mutex::new(inner::McpState::new(vec![stdio("kagi")]));

        assert_eq!(
            wait_until_mcp_init_settles(&state, std::time::Duration::from_secs(30)).await,
            McpInitWait::NotInitializing,
        );
    }
}

pub async fn start_mcp_servers(
    mcp_servers: Vec<acp::McpServer>,
    cwd: Option<&Path>,
    meta_config_map: &inner::McpMetaConfigMap,
    oauth_config_map: &McpOAuthConfigMap,
    ctx: &inner::McpSpawnCtx<'_>,
) -> Vec<Result<inner::McpClient, inner::McpError>> {
    let overrides_map: HashMap<String, inner::McpClientTimeoutOverrides> = mcp_servers
        .iter()
        .filter_map(|s| {
            let name = inner::mcp_server_name(s);
            resolve_overrides(name, cwd).map(|o| (name.to_string(), o))
        })
        .collect();
    inner::start_mcp_servers(
        mcp_servers,
        &overrides_map,
        meta_config_map,
        oauth_config_map,
        ctx,
    )
    .await
}
