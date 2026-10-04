//! Identifier newtypes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// Errors produced by id constructors and validators.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    #[error("identifier must not be empty")]
    Empty,
    #[error("identifier {value:?} has invalid format")]
    InvalidFormat { value: String },
    #[error("identifier {value:?} uses a reserved prefix")]
    ReservedPrefix { value: String },
}

#[inline]
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_valid_segment(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_id_char)
}

fn ensure_non_empty(s: &str) -> Result<(), IdError> {
    if s.is_empty() {
        Err(IdError::Empty)
    } else {
        Ok(())
    }
}

/// Generate a string-backed opaque id newtype.
///
/// Emits `new`, `as_str`, `into_inner`, `AsRef<str>`, `Display`, `FromStr`,
/// `TryFrom<String>`, and a validating `Deserialize` (which routes through
/// `Self::new`). `Serialize` is derived transparently.
///
/// An optional `extra_validator = $path` clause accepts a
/// `fn(&str) -> Result<(), IdError>` that runs after the empty-string
/// check.
macro_rules! opaque_id {
    ($(#[$meta:meta])* $name:ident $(, extra_validator = $validator:path)?) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct, validating the id's invariants.
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                ensure_non_empty(&value)?;
                $($validator(&value)?;)?
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::new(raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

/// Prefix reserved for hub-internal session-owner keys (bot-relay rotate lock).
pub const HUB_RESERVED_SESSION_PREFIX: &str = "__hub:";

fn validate_session_id(s: &str) -> Result<(), IdError> {
    if s.starts_with(HUB_RESERVED_SESSION_PREFIX) {
        return Err(IdError::ReservedPrefix {
            value: s.to_owned(),
        });
    }
    Ok(())
}

opaque_id!(
    /// Session identifier. Service-issued or carried from a JWT claim.
    SessionId,
    extra_validator = validate_session_id
);
opaque_id!(
    /// User identifier (the JWT `sub` claim).
    UserId
);
opaque_id!(
    /// Per-connection identifier issued by the computer hub.
    ConnectionId
);
opaque_id!(
    /// JSON-RPC request id as it appears on the wire.
    RequestId
);
opaque_id!(
    /// End-to-end identifier for a single tool invocation. SDKs SHOULD use UUID v7 (see [`ToolCallId::new_v7`]).
    ToolCallId
);

impl ToolCallId {
    /// Generate a fresh UUID v7-backed `ToolCallId`.
    pub fn new_v7() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}

const SERVER_ID_RESERVED_PREFIX: &str = "auto:";

fn validate_server_id(s: &str) -> Result<(), IdError> {
    if s.starts_with(SERVER_ID_RESERVED_PREFIX) {
        return Err(IdError::ReservedPrefix {
            value: s.to_owned(),
        });
    }
    Ok(())
}

opaque_id!(
    /// Server identifier.
    ServerId,
    extra_validator = validate_server_id
);

impl ServerId {
    /// Synthesise the deterministic computer-hub-side id for a single-tool
    /// `register_tool` that omits `server_id`. Bypasses [`ServerId::new`]'s
    /// reserved-prefix check.
    pub fn synthesize_for_tool(
        #[allow(unused_variables)] connection_id: &ConnectionId,
        tool_id: &ToolId,
    ) -> Self {
        Self(format!("{SERVER_ID_RESERVED_PREFIX}tool:{tool_id}"))
    }
}

fn validate_tool_id(s: &str) -> Result<(), IdError> {
    if !is_well_formed_tool_id(s) {
        return Err(IdError::InvalidFormat {
            value: s.to_owned(),
        });
    }
    Ok(())
}

fn is_well_formed_tool_id(s: &str) -> bool {
    let mut parts = s.splitn(3, ':');
    let Some(first) = parts.next() else {
        return false;
    };
    match (parts.next(), parts.next()) {
        (None, _) => is_valid_segment(first),
        (Some(second), None) => is_valid_segment(first) && is_valid_segment(second),
        _ => false,
    }
}

opaque_id!(
    /// Tool identifier. Format: `{namespace}:{name}` or `{name}`. Each segment must match `[a-zA-Z0-9_-]+`.
    ToolId,
    extra_validator = validate_tool_id
);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FrameSeq(u64);

impl FrameSeq {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FrameSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<u64> for FrameSeq {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
