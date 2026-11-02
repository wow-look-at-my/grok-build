//! Types the session actor hands back through `SessionCommand` replies, so the command layer can name them.
use crate::sampling::SamplingError;

/// Worker rounds a refuted goal may run without re-running verification before the continuation directive escalates to a forceful "re-verify now" block.
pub const GOAL_REVERIFY_AFTER_DEFAULT: u32 = 8;

/// Bumped when a turn is promoted. A report carrying an older epoch is refused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TurnEpoch(pub u64);

/// What a mention spawned.
#[derive(Debug, Clone)]
pub struct ModelMentionSpawn {
    pub subagent_id: String,
    pub model_id: String,
}

/// What a `/todo` run put on the list.
#[derive(Debug, Clone)]
pub struct TodoCaptureOutcome {
    /// Contents of the appended items, in the order they were added.
    pub added: Vec<String>,
    /// Read-only tool calls the capture agent spent getting there.
    pub tools_used: usize,
    /// The "the user assigned this" text to start a turn with, when no turn was running and nothing was put in front of the agent.
    pub wake_reminder: Option<String>,
}

/// Failure surface of a `/todo` capture run. Typed to the ACP boundary so
/// model errors keep the canonical mapping (rate limits, auth) instead of a
/// flattened string, same as `SideQuestionError`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TodoCaptureError {
    #[error("todo capture model call failed: {0}")]
    Sampling(#[from] SamplingError),
    #[error("failed to prepare client: {0}")]
    PrepareClient(String),
    #[error(
        "/todo needs a task-list tool whose items carry ids (`todo_write`); this session's is `{0}`, which identifies items by their text alone, so a capture cannot address the one it adds"
    )]
    UnsupportedTodoTool(String),
    #[error(
        "the capture agent finished without adding a todo ({reason}). Transcript: {transcript}"
    )]
    NothingAdded { reason: String, transcript: String },
    #[error("adding the todo failed: {0}")]
    TodoWriteFailed(String),
}

/// Serialized onto `streaming_partial.json` so trace inspection can tell
/// whether the abort interrupted thinking, response text, or a tool call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePhase {
    /// Stream opened (`StreamStarted`) but no channel token observed yet.
    #[default]
    Pending,
    /// Reasoning ("thinking") tokens were the most recent output.
    Reasoning,
    /// Visible response-text tokens were the most recent output.
    ResponseText,
    /// The model had begun emitting a tool call (`ToolCallDelta`).
    ToolCall,
}

/// Stored separately from `SessionHandle` (which derives `Clone`) because `JoinHandle` is not `Clone`.
pub struct SessionThread {
    join_handle: std::thread::JoinHandle<()>,
}
impl SessionThread {
    pub fn new(handle: std::thread::JoinHandle<()>) -> Self {
        Self {
            join_handle: handle,
        }
    }
    pub fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_handle(handle: std::thread::JoinHandle<()>) -> Self {
        Self::new(handle)
    }
}
