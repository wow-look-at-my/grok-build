//! The wire-side request envelope.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::metadata::Metadata;

/// Wire-side request envelope.
/// Cancellation and the in-process extensions map live on the runtime envelope, not here; see this module's doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMessage<T> {
    /// The typed request payload (one of the `*Request` enums).
    pub message: T,

    /// String-keyed metadata for the call (auth tokens, trace context, session id, ...).
    #[serde(default)]
    pub metadata: Metadata,

    /// Optional absolute deadline for the call, UTC, encoded as ISO-8601.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
}

impl<T> RequestMessage<T> {
    /// Construct a new request with empty metadata and no deadline.
    pub fn new(message: T) -> Self {
        Self {
            message,
            metadata: Metadata::default(),
            deadline: None,
        }
    }

    /// Builder: attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder: set the absolute deadline.
    #[must_use]
    pub fn with_deadline(mut self, deadline: DateTime<Utc>) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Map the inner payload while preserving metadata and deadline.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> RequestMessage<U> {
        RequestMessage {
            message: f(self.message),
            metadata: self.metadata,
            deadline: self.deadline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omits_deadline_when_none() {
        let req = RequestMessage::new(42_u32);
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("deadline"), "got {json}");
    }

    #[test]
    fn round_trips_with_deadline() {
        let when = chrono::Utc::now();
        let req = RequestMessage::new(42_u32).with_deadline(when);
        let json = serde_json::to_string(&req).unwrap();
        let back: RequestMessage<u32> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn map_preserves_metadata_and_deadline() {
        let when = chrono::Utc::now();
        let mut meta = Metadata::default();
        meta.insert("k", "v");
        let req = RequestMessage::new(1_u32)
            .with_metadata(meta.clone())
            .with_deadline(when);
        let mapped = req.map(|n| n.to_string());
        assert_eq!(mapped.message, "1");
        assert_eq!(mapped.metadata, meta);
        assert_eq!(mapped.deadline, Some(when));
    }
}
