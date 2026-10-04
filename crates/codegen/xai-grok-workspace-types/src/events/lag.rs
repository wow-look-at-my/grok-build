//! `EventLag` is a wire-format payload.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Backpressure signal when the event-bus subscriber lags and events are
/// dropped. Tagged with `tag = "type"` like every wire enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EventLag {
    /// The consumer fell behind by `n` events.
    #[error("lagged by {0} events")]
    Lagged(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape_uses_type_tag_with_data_payload() {
        let lag = EventLag::Lagged(3);
        let json = serde_json::to_string(&lag).unwrap();
        assert_eq!(json, r#"{"type":"lagged","data":3}"#);
    }
}
