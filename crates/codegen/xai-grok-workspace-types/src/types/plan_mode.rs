//! Plan-mode transition shapes used by [`ToolChunk::NeedPlanModeChange`](crate::chunks::ToolChunk::NeedPlanModeChange).

use serde::{Deserialize, Serialize};

/// Direction of a plan-mode transition the tool wants to make.
/// Adjacent tagging matches every other wire enum; see "# Wire format".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PlanModeTransition {
    /// Tool wants to enter plan mode.
    Enter {
        /// Optional initial plan text to seed the UI preview.
        #[serde(default)]
        plan: Option<String>,
    },
    /// Tool wants to exit plan mode and resume normal operation.
    /// `final_plan` is what the model will execute; the UI may render it for review.
    Exit {
        /// Optional final plan text the model will execute on exit.
        #[serde(default)]
        final_plan: Option<String>,
    },
}

/// User's decision on a proposed plan-mode transition. `Defer` is "not right
/// now", not `Reject`; the model may re-propose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum PlanModeDecision {
    /// Approve the transition; the tool applies the change.
    Approve,
    /// Reject the transition; the tool emits `Err(WorkspaceError::Permission { .. })`.
    Reject {
        /// Optional user-provided context for the model.
        #[serde(default)]
        feedback: Option<String>,
    },
    /// Defer the transition; the tool emits a non-error `Final` indicating no change was made.
    Defer,
}
