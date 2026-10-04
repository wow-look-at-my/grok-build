//! Per-session event log (`events.jsonl`).

#![deny(clippy::indexing_slicing)]

pub mod log;
pub mod tracker;
pub mod types;

pub use log::EventWriter;
pub use tracker::EventTracker;
pub use types::{
    CancellationCategory, EVENT_SCHEMA_VERSION, Event, McpConfigServer, McpErrorCategory,
    PermissionDecision, Phase, SessionRelationship, ToolCompletedSource, ToolOutcome,
    TurnOutcomeLabel,
};
