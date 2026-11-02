//! Subagent metadata records (`meta.json`, `subagent.json`) and the cancellation message a child turn reports.
use xai_grok_session_events::types::CancellationCategory;
pub fn cancellation_error_message(
    category: Option<xai_grok_session_events::types::CancellationCategory>,
    context: Option<&crate::session::commands::CancellationContext>,
) -> String {
    let detail = context.and_then(|ctx| {
        let tool = ctx.tool_name.as_deref();
        let reason = ctx.reason.as_deref();
        let hook = ctx.hook_name.as_deref();
        match (tool, reason, hook) {
            (Some(t), Some(r), Some(h)) => Some(format!("{r} for tool `{t}` (hook: {h})")),
            (Some(t), Some(r), None) => Some(format!("{r} for tool `{t}`")),
            (Some(t), None, _) => Some(format!("tool `{t}`")),
            _ => None,
        }
    });
    match (category, &detail) {
        (Some(CancellationCategory::PermissionRejected), Some(d)) => {
            format!("Subagent turn was cancelled: user rejected permission — {d}")
        }
        (Some(CancellationCategory::PermissionRejected), None) => {
            "Subagent turn was cancelled: user rejected a permission prompt".to_string()
        }
        (Some(CancellationCategory::PermissionCancelled), _) => {
            "Subagent turn was cancelled: user cancelled a permission prompt".to_string()
        }
        (Some(CancellationCategory::HookDenied), Some(d)) => {
            format!("Subagent turn was cancelled: hook denied — {d}")
        }
        (Some(CancellationCategory::HookDenied), None) => {
            "Subagent turn was cancelled: blocked by a hook".to_string()
        }
        (Some(CancellationCategory::MidTurnAbort), _) => {
            "Subagent turn was cancelled: aborted mid-turn".to_string()
        }
        _ => "Subagent turn was cancelled".to_string(),
    }
}
/// Metadata stored as `meta.json` in the child session directory.
/// Links the child session back to its parent.
/// For the GCS-persisted artifact (`subagent.json`), see [`SubagentSessionMetadata`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentMeta {
    pub subagent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub subagent_type: String,
    pub description: String,
    pub prompt: String,
    /// "running" | "completed" | "failed" | "cancelled"
    pub status: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Effective context source after bootstrap: "new" or "resumed".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_source: Option<String>,
    /// True only for a summarized (normalized) fork; false for verbatim mirror-forks, resume, and new sessions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub context_normalized: bool,
    /// Error message if fork-copy failed and fell back to fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_copy_error: Option<String>,
    /// Named persona applied to this subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// ID of the source subagent this session was resumed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    /// Effective cwd used by the child session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_cwd: Option<String>,
    /// Worktree path if the child used `isolation=worktree`. Persisted for durable `resume_from` reconstruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// Durable git ref holding a snapshot of the child's worktree working state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    /// Effective model ID used by the child session. Persisted for durable `resume_from` identity validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_model_id: Option<String>,
}
/// Canonical subagent metadata for GCS persistence (`subagent.json`).
/// Uploaded to `{session_id}/subagent.json` in GCS and optionally mirrored locally.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSessionMetadata {
    pub schema_version: u32,
    pub session_id: String,
    pub session_kind: String,
    pub subagent_id: String,
    pub child_session_id: String,
    pub parent_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_prompt_id: Option<String>,
    pub subagent_type: String,
    /// Human-readable spawn description: the task tool's `description` argument.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    #[serde(default)]
    pub context_normalized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation_mode: Option<String>,
    #[serde(default)]
    pub depth: u32,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_copy_error: Option<String>,
    /// ID of the source subagent this session was resumed from (`resume_from`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}
impl SubagentSessionMetadata {
    pub const SCHEMA_VERSION: u32 = 1;
    #[allow(clippy::too_many_arguments)]
    pub fn from_meta(
        meta: &SubagentMeta,
        model_id: Option<&str>,
        cwd: Option<&str>,
        worktree_path: Option<&str>,
        isolation_mode: Option<&str>,
        capability_mode: Option<&str>,
        reasoning_effort: Option<&str>,
        role: Option<&str>,
        parent_prompt_id: Option<&str>,
        depth: u32,
    ) -> Self {
        let session_kind = if meta.resumed_from.is_some() {
            "subagent_resume"
        } else {
            "subagent"
        };
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: meta.child_session_id.clone(),
            session_kind: session_kind.to_string(),
            subagent_id: meta.subagent_id.clone(),
            child_session_id: meta.child_session_id.clone(),
            parent_session_id: meta.parent_session_id.clone(),
            parent_prompt_id: parent_prompt_id.map(str::to_string),
            subagent_type: meta.subagent_type.clone(),
            description: meta.description.clone(),
            role: role.map(str::to_string),
            persona: meta.persona.clone(),
            context_normalized: meta.context_normalized,
            capability_mode: capability_mode.map(str::to_string),
            reasoning_effort: reasoning_effort.map(str::to_string),
            model_id: model_id.map(str::to_string),
            cwd: cwd.map(str::to_string),
            worktree_path: worktree_path.map(str::to_string),
            isolation_mode: isolation_mode.map(str::to_string),
            depth,
            started_at: meta.started_at.to_rfc3339(),
            completed_at: meta.completed_at.map(|t| t.to_rfc3339()),
            status: meta.status.clone(),
            duration_ms: meta.duration_ms,
            tool_calls: meta.tool_calls,
            turns: meta.turns,
            error: meta.error.clone(),
            fork_copy_error: meta.fork_copy_error.clone(),
            resumed_from: meta.resumed_from.clone(),
        }
    }
}
