//! Object-safe `ToolRegistry` trait shared by every storage plane.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use xai_tool_protocol::{
    ConnectionId, RegistrationOutcome, ServerId, SessionId, ToolDefinitionMode, ToolId,
    ToolRegistration, ToolServerRegistration, UserId,
};
use xai_tool_runtime::{SearchSnapshot, ServerSummary};
use xai_tool_types::ToolDescription;

use crate::resolver::ResolvedTool;

/// Outcome of a single [`ToolRegistry::bind_tool_session`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSessionBindOutcome {
    /// Added to the tool's session set.
    Bound,
    /// Session id was already in the tool's session set; no-op.
    AlreadyBound,
    /// No tool with the given id is registered against this connection.
    UnknownTool,
    /// Cross-connection conflict: another connection already holds the `(session_id, tool_id)` reverse-index slot.
    Conflict,
}

/// Outcome of a single [`ToolRegistry::unbind_tool_session`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSessionUnbindOutcome {
    /// Removed from the tool's session set.
    Unbound,
    /// Session id was not in the tool's session set; no-op.
    NotBound,
    /// No tool with the given id is registered against this connection.
    UnknownTool,
}

/// Aggregated summary of a connection-scoped cleanup pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionCleanupReport {
    /// Number of distinct `(connection, tool_id)` records dropped.
    pub tools_dropped: usize,
    /// Number of reverse-index `(session_id, tool_id)` rows cleaned up across every session the dropped tools were bound.
    pub session_bindings_cleared: usize,
}

/// Aggregated summary of a session-scoped cleanup pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCleanupReport {
    /// Number of tools whose session set lost the unregistered session id.
    pub tools_touched: usize,
    /// Number of tools whose session set became empty after the unregistration.
    pub tools_left_orphaned: usize,
}

/// Backend-agnostic registry of tools available within a router.
///
/// Methods are split into mutating (`async fn` — registration changes may
/// touch shared state and require coordination) and read-only views
/// (synchronous — implementations should answer from a consistent snapshot
/// without awaiting). The split mirrors how callers use the registry: the
/// hot path is the `find_tool` / `list_tools` view; mutations happen on the
/// rarer registration boundary.
#[async_trait]
pub trait ToolRegistry: Send + Sync + std::fmt::Debug {
    /// Register a single tool against `connection_id`.
    async fn register_tool(
        &self,
        connection_id: ConnectionId,
        reg: ToolRegistration,
    ) -> RegistrationOutcome;

    /// Register a multi-tool batch from a single tool server against
    /// `connection_id`.
    async fn register_server(
        &self,
        connection_id: ConnectionId,
        reg: ToolServerRegistration,
    ) -> Vec<RegistrationOutcome>;

    /// Drop the tool registered under `(connection_id, tool_id)`.
    async fn unregister_tool(&self, connection_id: &ConnectionId, tool: &ToolId) -> bool;

    /// Drop every tool registered by `connection_id` under `server_id`. Returns the number of entries removed.
    async fn unregister_server(&self, connection_id: &ConnectionId, server: &ServerId) -> usize;

    /// Add `session_id` to the per-tool session set of `(connection_id,
    /// tool_id)`.
    async fn bind_tool_session(
        &self,
        connection_id: &ConnectionId,
        tool: &ToolId,
        session_id: &SessionId,
    ) -> ToolSessionBindOutcome;

    /// Remove `session_id` from the per-tool session set of
    /// `(connection_id, tool_id)`. Does not unregister the tool itself.
    async fn unbind_tool_session(
        &self,
        connection_id: &ConnectionId,
        tool: &ToolId,
        session_id: &SessionId,
    ) -> ToolSessionUnbindOutcome;

    /// Drop every tool registered by `connection_id`. Used by the WebSocket transport on disconnect cleanup.
    async fn drop_connection(&self, connection_id: &ConnectionId) -> ConnectionCleanupReport;

    /// Look up the active resolution for `(session, tool)`.
    fn find_tool(&self, session: &SessionId, tool: &ToolId) -> Option<ResolvedTool>;

    /// Enumerate every active tool description for `session`, filtered by the requested presentation `mode`.
    fn list_tools(&self, session: &SessionId, mode: &ToolDefinitionMode) -> Vec<ToolDescription>;

    /// Enumerate active server summaries for `session`. Useful for rendering connected-integrations system reminders.
    fn list_servers(&self, session: &SessionId) -> Vec<ServerSummary>;

    /// Run a search query against the registry's index for `session`.
    fn search(&self, session: &SessionId, query: &str, limit: usize) -> SearchSnapshot;

    /// Drop the binding to `session` from every tool that has it.
    async fn unregister_session(&self, session: &SessionId) -> SessionCleanupReport;

    /// Helper: set of session ids bound to `(connection_id, tool_id)`.
    fn tool_sessions(&self, connection_id: &ConnectionId, tool: &ToolId) -> HashSet<SessionId>;

    /// All servers registered by this user across all connections.
    fn list_servers_for_user(&self, user_id: &UserId) -> Vec<ServerRecord>;

    /// Look up a server by its connection ID.
    fn get_server_record(&self, connection_id: &ConnectionId) -> Option<ServerRecord>;

    /// Look up only a server's id by its connection ID.
    fn get_server_id(&self, connection_id: &ConnectionId) -> Option<ServerId> {
        self.get_server_record(connection_id)
            .map(|record| record.server_id)
    }
}

/// Bind-time policy keys on the stamped [`ServerRecord::host_kind`].
pub use xai_tool_protocol::HostKind;

/// Server identity captured at `register_server` time.
#[derive(Debug, Clone)]
pub struct ServerRecord {
    pub connection_id: ConnectionId,
    pub user_id: UserId,
    pub server_id: ServerId,
    pub description: String,
    pub metadata: serde_json::Value,
    pub registered_at: chrono::DateTime<chrono::Utc>,
    /// Monotonic registration stamp ([`next_registration_seq`]) — the stale-vs-revived discriminator for newest-wins.
    pub registration_seq: u64,
    /// Resolved from the credential's minter at upgrade; `None` for a minter the hub does not know.
    pub host_kind: Option<HostKind>,
}

/// Process-global hybrid logical clock: per-process strictly-increasing (no ties, immune to NTP step-back) and epoch-seeded.
static REGISTRATION_CLOCK: AtomicU64 = AtomicU64::new(0);

/// Bits reserved below the wall-clock milliseconds in a registration seq: the per-process HLC bump space.
pub const REGISTRATION_SEQ_SHIFT: u32 = 10;

/// Encode a wall-clock millisecond reading as a registration seq (before
/// the HLC bump applied by [`next_registration_seq`]).
pub fn seq_from_wall_ms(wall_ms: u64) -> u64 {
    wall_ms << REGISTRATION_SEQ_SHIFT
}

/// Decode the wall-clock milliseconds a registration seq was issued at
/// (inverse of [`seq_from_wall_ms`], dropping the HLC bump bits).
pub fn seq_wall_ms(seq: u64) -> u64 {
    seq >> REGISTRATION_SEQ_SHIFT
}

/// Issue the next monotonic registration stamp. See [`REGISTRATION_CLOCK`].
pub fn next_registration_seq() -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let candidate = seq_from_wall_ms(now_ms);
    let mut prev = REGISTRATION_CLOCK.load(Ordering::Relaxed);
    loop {
        let next = candidate.max(prev + 1);
        match REGISTRATION_CLOCK.compare_exchange_weak(
            prev,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}
