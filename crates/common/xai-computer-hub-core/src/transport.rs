//! Object-safe `Transport` trait plus the `Principal` value carried across authorize/call boundaries.

use async_trait::async_trait;
use serde_json::Value;

use xai_tool_protocol::{SessionId, ToolId, UserId};
use xai_tool_runtime::{ToolCallContext, ToolError, ToolStream, TypedToolOutput};

pub use xai_tool_protocol::TransportKind;

/// Authenticated identity bound to a transport at handshake time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// Authenticated user identity.
    pub user_id: UserId,

    /// Sessions this principal is authorised to act on.
    pub session_ids: Vec<SessionId>,

    /// OAuth-style scopes granted to this principal, e.g. `"tool.invoke"`.
    pub scopes: Vec<String>,

    /// Token audiences claimed by the credential, e.g. the router's expected `aud` values.
    pub audiences: Vec<String>,
}

impl Principal {
    /// Build a principal for `user_id` with no sessions, scopes, or
    /// audiences. Use the `with_*` builders to populate the rest.
    pub fn new(user_id: UserId) -> Self {
        Self {
            user_id,
            session_ids: Vec::new(),
            scopes: Vec::new(),
            audiences: Vec::new(),
        }
    }

    /// Append `session_id` to the authorised set.
    pub fn with_session(mut self, session_id: SessionId) -> Self {
        self.session_ids.push(session_id);
        self
    }

    /// Append `scope` to the granted scopes.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scopes.push(scope.into());
        self
    }

    /// Append `aud` to the token's audience list.
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self {
        self.audiences.push(aud.into());
        self
    }

    /// Whether `scope` is present in the granted scopes.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Whether `session_id` is in the principal's authorised session set.
    pub fn authorizes_session(&self, session_id: &SessionId) -> bool {
        self.session_ids.iter().any(|s| s == session_id)
    }
}

/// Object-safe transport for dispatching tool calls.
#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Whether the underlying transport is local (in-process) or remote (forwarded over a connection).
    fn kind(&self) -> TransportKind;

    /// One-time authorisation handshake.
    async fn authorize(&self) -> Result<Principal, ToolError>;

    /// Dispatch a tool call.
    async fn call(
        &self,
        tool_id: ToolId,
        args: Value,
        ctx: ToolCallContext,
    ) -> ToolStream<TypedToolOutput>;
}
