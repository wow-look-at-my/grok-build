//! Prompt metadata types shared between the CLI client and the cli-chat-proxy server.
use serde::{Deserialize, Serialize};
/// Schema version for the GCS metadata format.
pub const GCS_SCHEMA_VERSION: &str = "v1.24";
/// OS-level sandbox state for a trace turn (local `xai-grok-sandbox`, not cloud sandbox).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalSandboxTelemetry {
    /// Resolved profile at process startup (e.g. "off", "workspace", "strict").
    pub profile: String,
    /// Whether kernel-level enforcement is active for this process.
    pub applied: bool,
}
/// Metadata about a prompt turn, uploaded as JSON for tracing/debugging.
///
/// Path format: `{session_id}/turn_{N}/metadata.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PromptMetadata {
    /// Schema version for this metadata format
    pub schema_version: String,
    /// Session id (UUIDv7) for this trace
    pub session_id: String,
    /// Monotonic turn number within the session
    pub turn_number: u64,
    /// Request id for this prompt (uuid v4 we generate per prompt)
    pub request_id: String,
    /// Subagent attempt id when this trace belongs to a child activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    /// Timestamp at the start of prompt handling (UTC RFC3339)
    pub turn_started_at: String,
    /// Git repo root (if the session cwd is inside a git repository).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    /// Git remote URL (origin) for the repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    /// How workspace files were collected: "git", "project", or "non_project".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_type: Option<String>,
    /// User ID from authentication
    pub user_id: Option<String>,
    /// User email from authentication (may be None)
    pub user_email: Option<String>,
    /// Team ID from OAuth authentication (may be None for personal accounts)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    /// Client source identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_source: Option<String>,
    /// Client (TUI) version string, e.g., "0.1.70 (c28a985a1f1)" This is sent by the TUI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    /// The model being used for this session
    pub model: String,
    /// Reasoning effort the turn was sampled with (e.g. "low", "medium", "high", "xhigh").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Experiment ID when the model was overridden via experiment routing. Currently unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
    /// Host OS where the agent is running (e.g., "macos", "linux")
    pub host_os: String,
    /// Host architecture (e.g., "x86_64", "aarch64")
    pub host_arch: String,
    /// Whether the user's prompt contains at least one image attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_has_image: Option<bool>,
    /// Whether the prompt was truncated. When `Some(true)`, the full text is at `full_prompt.txt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_was_truncated: Option<bool>,
    /// Whether the prompt was sent in verbatim mode (skipping `<user_query>` wrapping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_verbatim: Option<bool>,
    /// Current working directory of the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The agent type / harness name for this session (e.g. "grok-build", "opencode").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Version of the grok-shell agent binary that handled this turn (`xai_grok_version::version()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_version: Option<String>,
    /// Resolved OS sandbox profile and whether enforcement is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<LocalSandboxTelemetry>,
}
/// Always-present fields for [`PromptMetadata::new`].
///
/// Name required fields explicitly; optional fields may use `..Default::default()`.
#[derive(Debug, Clone, Default)]
pub struct PromptMetadataParams {
    pub schema_version: String,
    pub session_id: String,
    pub turn_number: u64,
    pub request_id: String,
    pub attempt_id: Option<String>,
    pub turn_started_at: String,
    pub repo_root: Option<String>,
    pub remote_url: Option<String>,
    pub workspace_type: Option<String>,
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    pub team_id: Option<String>,
    pub client_source: Option<String>,
    pub client_version: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub experiment_id: Option<String>,
    pub host_os: String,
    pub host_arch: String,
    pub prompt_has_image: Option<bool>,
    pub prompt_was_truncated: Option<bool>,
    pub prompt_verbatim: Option<bool>,
    pub cwd: Option<String>,
    pub agent_type: Option<String>,
    pub shell_version: Option<String>,
    pub sandbox: Option<LocalSandboxTelemetry>,
}
impl PromptMetadata {
    /// Complete constructor. Optional collection fields are defaulted when
    /// compiled in, so callers never name them in a struct literal.
    #[must_use]
    pub fn new(params: PromptMetadataParams) -> Self {
        Self {
            schema_version: params.schema_version,
            session_id: params.session_id,
            turn_number: params.turn_number,
            request_id: params.request_id,
            attempt_id: params.attempt_id,
            turn_started_at: params.turn_started_at,
            repo_root: params.repo_root,
            remote_url: params.remote_url,
            workspace_type: params.workspace_type,
            user_id: params.user_id,
            user_email: params.user_email,
            team_id: params.team_id,
            client_source: params.client_source,
            client_version: params.client_version,
            model: params.model,
            reasoning_effort: params.reasoning_effort,
            experiment_id: params.experiment_id,
            host_os: params.host_os,
            host_arch: params.host_arch,
            prompt_has_image: params.prompt_has_image,
            prompt_was_truncated: params.prompt_was_truncated,
            prompt_verbatim: params.prompt_verbatim,
            cwd: params.cwd,
            agent_type: params.agent_type,
            shell_version: params.shell_version,
            sandbox: params.sandbox,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Minimal JSON matching the pre-prompt-content schema fields.
    fn minimal_json() -> &'static str {
        r#"{
            "schema_version": "v1.23",
            "session_id": "abc",
            "turn_number": 1,
            "request_id": "req-1",
            "turn_started_at": "2025-01-01T00:00:00Z",
            "user_id": null,
            "user_email": null,
            "model": "grok-3",
            "host_os": "linux",
            "host_arch": "x86_64"
        }"#
    }
    #[test]
    fn missing_fields_deserialize_to_none_not_false() {
        let meta: PromptMetadata = serde_json::from_str(minimal_json()).unwrap();
        assert_eq!(meta.prompt_has_image, None);
        assert_eq!(meta.prompt_was_truncated, None);
        assert_eq!(meta.cwd, None);
        assert_eq!(meta.team_id, None);
    }
    #[test]
    fn explicit_false_deserializes_to_some_false() {
        let json = r#"{
            "schema_version": "v1.23",
            "session_id": "abc",
            "turn_number": 1,
            "request_id": "req-1",
            "turn_started_at": "2025-01-01T00:00:00Z",
            "user_id": null,
            "user_email": null,
            "model": "grok-3",
            "host_os": "linux",
            "host_arch": "x86_64",
            "prompt_has_image": false,
            "prompt_was_truncated": false
        }"#;
        let meta: PromptMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.prompt_has_image, Some(false));
        assert_eq!(meta.prompt_was_truncated, Some(false));
    }
    #[test]
    fn none_fields_are_omitted_from_serialization() {
        let meta: PromptMetadata = serde_json::from_str(minimal_json()).unwrap();
        let serialized = serde_json::to_string(&meta).unwrap();
        assert!(!serialized.contains("prompt_has_image"));
        assert!(!serialized.contains("prompt_was_truncated"));
        assert!(!serialized.contains("cwd"));
        assert!(!serialized.contains("team_id"));
        assert!(!serialized.contains("\"prompt\""));
        assert!(!serialized.contains("full_prompt"));
        assert!(!serialized.contains("truncated_prompt_local_path"));
    }
    #[test]
    fn some_fields_are_included_in_serialization() {
        let mut meta: PromptMetadata = serde_json::from_str(minimal_json()).unwrap();
        meta.prompt_has_image = Some(false);
        meta.prompt_was_truncated = Some(true);
        let serialized = serde_json::to_string(&meta).unwrap();
        assert!(serialized.contains("\"prompt_has_image\":false"));
        assert!(serialized.contains("\"prompt_was_truncated\":true"));
    }
    #[test]
    fn cwd_round_trips() {
        let mut meta: PromptMetadata = serde_json::from_str(minimal_json()).unwrap();
        meta.cwd = Some("/root/code/xai".into());
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: PromptMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.cwd.as_deref(), Some("/root/code/xai"));
    }
    #[test]
    fn sandbox_round_trips() {
        let mut meta: PromptMetadata = serde_json::from_str(minimal_json()).unwrap();
        meta.sandbox = Some(LocalSandboxTelemetry {
            profile: "strict".into(),
            applied: true,
        });
        meta.attempt_id = Some("at1.abc".into());
        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: PromptMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.sandbox,
            Some(LocalSandboxTelemetry {
                profile: "strict".into(),
                applied: true,
            })
        );
        assert_eq!(deserialized.attempt_id.as_deref(), Some("at1.abc"));
    }
    #[test]
    fn new_defaults_optional_collection_fields() {
        let meta = PromptMetadata::new(PromptMetadataParams {
            schema_version: GCS_SCHEMA_VERSION.into(),
            session_id: "abc".into(),
            turn_number: 1,
            request_id: "req-1".into(),
            turn_started_at: "2025-01-01T00:00:00Z".into(),
            model: "grok-3".into(),
            host_os: "linux".into(),
            host_arch: "x86_64".into(),
            ..Default::default()
        });
        assert_eq!(meta.session_id, "abc");
        assert_eq!(meta.model, "grok-3");
    }
}
