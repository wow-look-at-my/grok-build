//! Each identifier is a newtype wrapper around `String`.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub(crate) String);

impl SessionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Unique tool call identifier within a session.
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub(crate) String);

impl ToolCallId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ToolCallId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolCallId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Unique hunk identifier produced by the hunk tracker.
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HunkId(pub(crate) String);

impl HunkId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for HunkId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for HunkId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_serializes_transparently() {
        let id = SessionId::new("sess-123");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"sess-123\"");
        let back: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn tool_call_id_serializes_transparently() {
        let id = ToolCallId::new("call-abc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"call-abc\"");
        let back: ToolCallId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn hunk_id_serializes_transparently() {
        let id = HunkId::new("hunk-xyz");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"hunk-xyz\"");
        let back: HunkId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
