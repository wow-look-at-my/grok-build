#![allow(clippy::cast_lossless)]
#![deny(clippy::indexing_slicing)]

mod file_acceleration;
pub use file_acceleration::*;
mod flags;
pub use flags::*;
mod registry;
pub use registry::*;
mod memory;
pub use memory::*;
mod mcp;
pub use mcp::*;
mod permission;
pub use permission::*;
mod auth_provider;
pub use auth_provider::*;
mod privacy;
pub mod retry_budget;
pub use privacy::{PrivacyMode, SharedPrivacyMode};
mod sandbox;
pub use sandbox::*;

use serde::{Deserialize, Serialize};

pub use xai_grok_config::{
    CampaignOverride, ConsentGate, ContextualHintsRemote, DisplayRefreshSettings,
    DoomLoopRecoverySettings, GoalRoleModel, LongReasoningReminderSettings, RemoteRequestEncoding,
    RemoteSettings, WorktreeAutoGcSettings, WorktreeKindMaxAge, deserialize_tolerant,
};

/// Advanced knobs for the output-rate floor: the `[output_rate_floor]` TOML
/// table. Every field is `Option` so a partial table never fails the parse
/// and each key falls through to the client default on its own.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(default)]
pub struct OutputRateFloorSettings {
    /// Trailing window the rate is measured over, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<u64>,
    /// Reissue budget per model call. `-1` or `"unlimited"` never runs out.
    #[serde(with = "retry_budget", skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "retry_budget::json_schema")]
    pub max_retries: Option<u32>,
}
