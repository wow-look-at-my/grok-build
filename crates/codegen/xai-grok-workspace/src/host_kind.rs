//! Which host runs a workspace server, and what its credential lets the server do.

use xai_computer_hub_sdk::SharedAuthProvider;
use xai_grok_tools::registry::types::ToolServerConfig;

use crate::session::tool_config::WorkspaceSessionContextFactory;

/// The host running a workspace server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceHostKind {
    /// A hosted sandbox.
    Sandbox,
    /// A daemon on a user's or remote machine, whoever started it (Grok Desktop, the `grok` CLI, or by hand).
    #[default]
    Daemon,
}

impl WorkspaceHostKind {
    /// The `host_kind` string for registration metadata (`xai_tool_protocol::HOST_KIND_*`).
    pub fn as_wire_str(self) -> &'static str {
        match self {
            WorkspaceHostKind::Sandbox => xai_tool_protocol::HOST_KIND_SANDBOX,
            WorkspaceHostKind::Daemon => xai_tool_protocol::HOST_KIND_DAEMON,
        }
    }

    /// Whether the host's credential only serves the hub: every host but the
    /// sandbox.
    pub fn is_hub_only(self) -> bool {
        match self {
            WorkspaceHostKind::Sandbox => false,
            WorkspaceHostKind::Daemon => true,
        }
    }

    /// Whether the server streams `FsChanged` for its root.
    pub fn streams_fs_changes(self) -> bool {
        match self {
            WorkspaceHostKind::Sandbox => false,
            WorkspaceHostKind::Daemon => true,
        }
    }

    /// The catalog a server on this host advertises to the hub, and serves when a bind names no
    /// toolset and the host lets it: the sandbox's, minus the API-backed tools on a hub-only host.
    pub fn default_toolset(self) -> ToolServerConfig {
        let mut catalog = xai_grok_agent::workspace_grok_build_toolset();
        if self.is_hub_only() {
            let api_backed = xai_grok_agent::api_backed_tool_ids();
            catalog.tools.retain(|tool| !api_backed.contains(&tool.id));
        }
        catalog
    }

    /// The session-context factory for this host: only one whose credential reaches the API hands
    /// `auth` to the gen and search tool configs and to the session `Resources`.
    pub(crate) fn session_context_factory(
        self,
        auth: SharedAuthProvider,
        api_base_url: String,
    ) -> WorkspaceSessionContextFactory {
        if self.is_hub_only() {
            WorkspaceSessionContextFactory::hub_only()
        } else {
            WorkspaceSessionContextFactory::with_auth(auth, api_base_url)
        }
    }
}

#[cfg(test)]
#[path = "host_kind_tests.rs"]
mod tests;
