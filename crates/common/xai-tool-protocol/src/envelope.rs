
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{FrameSeq, IdError, RequestId, SessionId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonRpcVersion;

impl JsonRpcVersion {
    pub const VERSION: &'static str = "2.0";
}

impl fmt::Display for JsonRpcVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::VERSION)
    }
}

impl Serialize for JsonRpcVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::VERSION)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = JsonRpcVersion;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "the literal string \"{}\"", JsonRpcVersion::VERSION)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == JsonRpcVersion::VERSION {
                    Ok(JsonRpcVersion)
                } else {
                    Err(E::custom(format!(
                        "expected jsonrpc \"{}\", got {v:?}",
                        JsonRpcVersion::VERSION
                    )))
                }
            }
        }
        deserializer.deserialize_str(V)
    }
}

/// Per the spec the `id` MAY be a string, a number, or null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    String(String),
    Number(i64),
}

impl JsonRpcId {
    pub fn new_string(s: impl Into<String>) -> Self {
        Self::String(s.into())
    }

    /// Build a fresh UUID v7-backed id.
    pub fn new_uuid_v7() -> Self {
        Self::String(uuid::Uuid::now_v7().to_string())
    }

    pub fn from_request_id(id: &RequestId) -> Self {
        Self::String(id.as_str().to_owned())
    }

    /// Project to a [`RequestId`]. Numeric ids are stringified. Returns
    /// an error if the resulting string would be empty.
    pub fn as_request_id(&self) -> Result<RequestId, IdError> {
        match self {
            Self::String(s) => RequestId::new(s.as_str()),
            Self::Number(n) => RequestId::new(n.to_string()),
        }
    }
}

impl fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Number(n) => write!(f, "{n}"),
        }
    }
}

///
/// Generic over `params` so callers can pin a concrete schema (e.g.
/// [`crate::frames::ToolCallParams`]) without losing the envelope's
/// invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest<P = serde_json::Value> {
    pub jsonrpc: JsonRpcVersion,
    pub id: JsonRpcId,
    /// Grok extension: routing/sanity-check session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub method: String,
    pub params: P,
}

///
/// No `id` (notifications do not produce a response). `seq` is an
/// optional per-connection monotonic counter so receivers can dedup and
/// detect drops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification<P = serde_json::Value> {
    pub jsonrpc: JsonRpcVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<FrameSeq>,
    pub method: String,
    pub params: P,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Per the spec exactly one of `result` / `error` is present.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcResponse<R = serde_json::Value> {
    pub jsonrpc: JsonRpcVersion,
    pub id: JsonRpcId,
    pub session_id: Option<SessionId>,
    pub outcome: ResponseOutcome<R>,
}

/// Either a `result` payload (success) or a [`JsonRpcError`] (failure).
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseOutcome<R> {
    Result(R),
    Error(JsonRpcError),
}

impl<R: Serialize> Serialize for JsonRpcResponse<R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut len = 3;
        if self.session_id.is_some() {
            len += 1;
        }
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("jsonrpc", &self.jsonrpc)?;
        map.serialize_entry("id", &self.id)?;
        if let Some(sid) = &self.session_id {
            map.serialize_entry("session_id", sid)?;
        }
        match &self.outcome {
            ResponseOutcome::Result(r) => map.serialize_entry("result", r)?,
            ResponseOutcome::Error(e) => map.serialize_entry("error", e)?,
        }
        map.end()
    }
}

impl<'de, R: Deserialize<'de>> Deserialize<'de> for JsonRpcResponse<R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `Option<...>` deserialises to `None` when missing without
        // `#[serde(default)]`.
        #[derive(Deserialize)]
        struct Flat<R> {
            jsonrpc: JsonRpcVersion,
            id: JsonRpcId,
            session_id: Option<SessionId>,
            result: Option<R>,
            error: Option<JsonRpcError>,
        }

        let flat = Flat::<R>::deserialize(deserializer)?;
        let outcome = match (flat.result, flat.error) {
            (Some(r), None) => ResponseOutcome::Result(r),
            (None, Some(e)) => ResponseOutcome::Error(e),
            (Some(_), Some(_)) => {
                return Err(de::Error::custom(
                    "JSON-RPC response must contain `result` XOR `error`, got both",
                ));
            }
            (None, None) => {
                return Err(de::Error::custom(
                    "JSON-RPC response must contain `result` or `error`",
                ));
            }
        };
        Ok(Self {
            jsonrpc: flat.jsonrpc,
            id: flat.id,
            session_id: flat.session_id,
            outcome,
        })
    }
}

impl<R> JsonRpcResponse<R> {
    pub fn ok(id: JsonRpcId, result: R) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            session_id: None,
            outcome: ResponseOutcome::Result(result),
        }
    }

    pub fn err(id: JsonRpcId, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            session_id: None,
            outcome: ResponseOutcome::Error(error),
        }
    }

    pub fn with_session(mut self, sid: SessionId) -> Self {
        self.session_id = Some(sid);
        self
    }
}
