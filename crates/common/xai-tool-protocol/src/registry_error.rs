//! Serializable registry-level errors.

use serde::{Deserialize, Serialize};

use crate::{ServerId, SessionId, ToolId};

#[derive(thiserror::Error, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RegistryError {
    /// A different connection already owns this `(session, tool)`.
    #[serde(rename = "tool_already_registered")]
    #[error("tool already registered: {tool_id}")]
    AlreadyRegistered { tool_id: ToolId },

    /// The registration's session does not match the connection's bound
    /// session.
    #[error("session mismatch: token session={token_session}, registration session={reg_session}")]
    SessionMismatch {
        token_session: SessionId,
        reg_session: SessionId,
    },

    /// `server_id` collides with an active server in this session owned by a different connection.
    #[error("server_id {server_id} collides with an active server in this session")]
    ServerIdCollision { server_id: ServerId },

    /// `server_id` is already in use on this connection by an earlier registration with a different tool set.
    #[error("server_id {server_id} already owned by an earlier registration on this connection")]
    ServerIdInUse { server_id: ServerId },

    /// Description failed structural validation.
    #[error("invalid description: {message}")]
    InvalidDescription { message: String },

    /// `if_match_generation` precondition failed.
    #[error("stale generation: expected={expected}, actual={actual}")]
    StaleGeneration { expected: u64, actual: u64 },
}
