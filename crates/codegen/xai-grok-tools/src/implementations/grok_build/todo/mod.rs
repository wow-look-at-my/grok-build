//! TodoWrite — new-architecture implementation.
//!
//! Reuses the core logic (`validate_no_duplicate_ids`, `apply_replace`,
//! `apply_merge`, `summarize_todo_state`) from the old `implementations::todo`
//! module. State is stored as `State<TodoState>` in Resources instead of
//! `ToolState.todo_state`.

use std::fmt::Write;

use crate::implementations::grok_build::task::backend::SubagentBackendResource;
use crate::implementations::grok_build::task::effective_max_subagent_depth;
use crate::implementations::grok_build::task::types::{
    CurrentPromptIdResource, SessionIdResource, SpawnRootSpan, SubagentDepthCounter, SubagentOwner,
    SubagentRequest, SubagentRuntimeOverrides, SubagentValidateTypeOutcome,
};
use crate::types::output::{TodoWriteOutput, TodoWriteSuccess};
use crate::types::requirements::{Expr, ToolRequirement};
#[allow(unused_imports)]
use crate::types::resources::{SharedResources, State};
use crate::types::tool::{ToolKind, ToolNamespace};

#[derive(thiserror::Error, Debug)]
pub enum TodoError {
    #[error("Missing Todo content in mode: {0}")]
    MissingTodoContent(String),

    #[error("Missing Todo ID in mode: {0}")]
    MissingTodoID(String),

    #[error("Duplicate Todo ID in response: {0}")]
    DuplicateTodoID(String),
}

pub(crate) fn validate_no_duplicate_ids(updates: &[TodoUpdate]) -> Result<(), TodoError> {
    use std::collections::HashSet;
    let mut seen = HashSet::with_capacity(updates.len());
    if let Some(dup) = updates.iter().map(|u| &u.id).find(|id| !seen.insert(*id)) {
        return Err(TodoError::DuplicateTodoID(dup.to_owned()));
    }
    Ok(())
}

/// How many items may be `in_progress` at once.
const DEFAULT_MAX_IN_PROGRESS: usize = 5;

/// The env var that overrides [`DEFAULT_MAX_IN_PROGRESS`].
const MAX_IN_PROGRESS_VAR: &str = "GROK_TODO_MAX_IN_PROGRESS";

/// Resolve the cap from the raw env value. A value that is not a positive
/// number keeps the default and says so.
fn max_in_progress_cap(raw: Option<&str>) -> usize {
    let Some(raw) = raw else {
        return DEFAULT_MAX_IN_PROGRESS;
    };
    match raw.trim().parse::<usize>() {
        Ok(n) if n > 0 => n,
        _ => {
            tracing::warn!(
                value = %raw,
                "{MAX_IN_PROGRESS_VAR} is not a positive number; using the default"
            );
            DEFAULT_MAX_IN_PROGRESS
        }
    }
}

/// The message a write is refused with when it would leave more than `cap`
/// items `in_progress`, or `None` when the write is within the cap.
fn in_progress_cap_violation(state: &TodoState, cap: usize) -> Option<String> {
    let running = state
        .todo_items()
        .filter(|item| item.status == TodoStatus::InProgress)
        .count();
    (running > cap).then(|| {
        format!(
            "{running} items would be in progress at once, over the cap of {cap}. \
             Keep what you are working on now in progress and leave the rest pending."
        )
    })
}

/// Every write is a merge: updates are folded into the existing state.
/// - **Existing items**: `content` is optional — if omitted the previous
///   value is kept. This lets the model mark an item from `in_progress` →
///   `completed` without echoing the content back.
/// - **New items** (id not yet in state): if `content` is omitted the `id`
///   is used as a fallback so the tool never errors on a merge call. This
///   makes the tool resilient to state being lost between calls.
///
/// `prepend` puts new items at the FRONT of the list, in the order given.
/// Existing items keep their place either way, so a prepend still cannot
/// reorder or rewrite work already on the list.
///
/// An id already on the list is never dropped by a write that omits it. An
/// item leaves the actionable set only by becoming `Completed` or
/// `Cancelled`, which is a status the caller has to ask for by id.
pub(crate) fn apply_merge(
    state: &mut TodoState,
    updates: &[TodoUpdate],
    prepend: bool,
) -> Result<(), TodoError> {
    let mut front = 0usize;
    for u in updates {
        if state.update_with_verification(
            &u.id,
            u.content.as_deref(),
            u.status,
            u.verification.as_deref(),
        ) {
            // Existing item – partial update succeeded, content was optional.
            continue;
        }
        let content = if u.has_no_content() {
            u.id.clone()
        } else {
            u.content.clone().unwrap()
        };
        let status = u.status.unwrap_or(TodoStatus::Pending);
        let item = TodoItem {
            content,
            priority: TodoPriority::default(),
            status,
            meta: None,
            verification: u.verification.clone().filter(|v| !v.trim().is_empty()),
            verification_passed: false,
        };
        if prepend {
            state.insert_at(front, u.id.clone(), item);
            front += 1;
        } else {
            state.push(u.id.clone(), item);
        }
    }
    Ok(())
}

/// A finished item's echo is cut to this many characters of its first line.
const FINISHED_ITEM_ECHO_CHARS: usize = 100;

/// Every write echoes the whole list, and the list never shrinks. So a
/// finished item echoes only its first line. The state keeps the full text.
pub(crate) fn summarize_todo_state(state: &TodoState) -> String {
    if state.is_empty() {
        "No tasks currently tracked.".into()
    } else {
        let mut out = String::new();
        for (id, t) in state.todo_items_with_ids() {
            let finished = matches!(t.status, TodoStatus::Completed | TodoStatus::Cancelled);
            if finished {
                let first = t.content.lines().next().unwrap_or("");
                let mut head: String = first.chars().take(FINISHED_ITEM_ECHO_CHARS).collect();
                if head.len() < t.content.trim_end().len() {
                    head.push('…');
                }
                writeln!(&mut out, "- {} {id}: {head}", t.status.tag()).ok();
            } else {
                writeln!(&mut out, "- {} {id}: {}", t.status.tag(), t.content).ok();
            }
        }
        out
    }
}

/// Subagent type a todo verifier runs as.
pub const VERIFIER_SUBAGENT_TYPE: &str = "general-purpose";

/// The line a verifier subagent ends its reply with to report its verdict.
pub const VERIFIER_PASS_MARKER: &str = "VERIFIER_RESULT: PASS";
/// The line a verifier subagent ends its reply with when the condition is not met.
pub const VERIFIER_FAIL_MARKER: &str = "VERIFIER_RESULT: FAIL";

/// A verifier subagent's verdict, read from its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationVerdict {
    Passed,
    Failed(String),
    /// The verifier returned no verdict line.
    Unreported,
}

/// Read the LAST verdict line from a verifier's output. A PASS after a FAIL is
/// the verifier correcting itself, which is why the last one wins.
pub fn parse_verification_verdict(output: &str) -> VerificationVerdict {
    for line in output.lines().rev() {
        let line = line.trim();
        if line.starts_with(VERIFIER_PASS_MARKER) {
            return VerificationVerdict::Passed;
        }
        if let Some(rest) = line.strip_prefix(VERIFIER_FAIL_MARKER) {
            let reason = rest.trim_start_matches([':', '-', ' ']).trim();
            return VerificationVerdict::Failed(if reason.is_empty() {
                "verifier reported failure".to_string()
            } else {
                reason.to_string()
            });
        }
    }
    VerificationVerdict::Unreported
}

/// Build the prompt handed to a fresh verifier subagent. It carries the item,
/// the condition and whatever context the triggering call supplied.
pub fn verifier_prompt(content: &str, condition: &str, context: Option<&str>) -> String {
    let mut prompt = String::new();
    writeln!(
        &mut prompt,
        "You are an independent verifier. Decide whether the verification condition below is \
         satisfied by the current state of the workspace. Gather your own evidence; assume nothing \
         about whether the work was done, and do not change the workspace to make the condition \
         pass."
    )
    .ok();
    writeln!(&mut prompt).ok();
    writeln!(&mut prompt, "Todo item: {content}").ok();
    writeln!(&mut prompt, "Verification condition: {condition}").ok();
    if let Some(context) = context.map(str::trim).filter(|c| !c.is_empty()) {
        writeln!(&mut prompt).ok();
        writeln!(&mut prompt, "Context supplied by the caller:").ok();
        writeln!(&mut prompt, "{context}").ok();
    }
    writeln!(&mut prompt).ok();
    writeln!(
        &mut prompt,
        "End your reply with a single final line: `{VERIFIER_PASS_MARKER}` if the condition is \
         satisfied, or `{VERIFIER_FAIL_MARKER}: <reason>` if it is not."
    )
    .ok();
    prompt
}

/// A completion this call requested whose verifier has not passed yet.
struct PendingVerification {
    id: TodoId,
    content: String,
    condition: String,
    context: Option<String>,
}

/// Outcome of one verifier run.
struct VerificationOutcome {
    id: TodoId,
    passed: bool,
    detail: String,
}

impl VerificationOutcome {
    fn blocked(id: &TodoId, detail: String) -> Self {
        Self {
            id: id.clone(),
            passed: false,
            detail,
        }
    }
}

/// Run one verifier prompt in its own fresh subagent and translate the result
/// into a verdict. Every failure mode is a block, so an item can never be
/// completed on a verifier that did not run.
async fn run_verifier(
    ctx: &xai_tool_runtime::ToolCallContext,
    backend: Option<&SubagentBackendResource>,
    parent_session_id: &str,
    parent_prompt_id: Option<String>,
    depth: u32,
    max_depth: u32,
    pending: &PendingVerification,
) -> VerificationOutcome {
    let Some(backend) = backend else {
        return VerificationOutcome::blocked(
            &pending.id,
            "no subagent support is available in this session".to_string(),
        );
    };
    if depth >= max_depth {
        return VerificationOutcome::blocked(
            &pending.id,
            format!("subagent depth limit reached ({depth}/{max_depth})"),
        );
    }
    match backend
        .backend()
        .validate_type(VERIFIER_SUBAGENT_TYPE, parent_session_id)
        .await
    {
        SubagentValidateTypeOutcome::Ok => {}
        _ => {
            return VerificationOutcome::blocked(
                &pending.id,
                format!("subagent type '{VERIFIER_SUBAGENT_TYPE}' is unavailable"),
            );
        }
    }

    let child_id = uuid::Uuid::now_v7().to_string();
    let span = tracing::info_span!(parent: None, "todo.verify", todo_id = %pending.id);
    span.follows_from(tracing::Span::current().id());
    let request = SubagentRequest {
        id: child_id,
        prompt: verifier_prompt(
            &pending.content,
            &pending.condition,
            pending.context.as_deref(),
        ),
        description: format!(
            "verify todo: {}",
            pending.content.lines().next().unwrap_or("").trim()
        ),
        subagent_type: VERIFIER_SUBAGENT_TYPE.to_string(),
        parent_session_id: parent_session_id.to_string(),
        parent_prompt_id,
        // Fresh subagent: no resumed transcript and no forked conversation.
        resume_from: None,
        cwd: None,
        runtime_overrides: SubagentRuntimeOverrides::default(),
        run_in_background: false,
        // Harness-internal: the verdict returns through this tool call.
        surface_completion: false,
        await_to_completion: false,
        fork_context: false,
        owner: SubagentOwner::Task,
        cancel_token: tokio_util::sync::CancellationToken::new(),
        spawn_root: SpawnRootSpan::new(span),
        tool_call_id: Some(ctx.call_id.as_str().to_owned()),
    };

    let result = match backend.backend().spawn(request, None).await {
        Ok(result) => result,
        Err(error) => {
            return VerificationOutcome::blocked(
                &pending.id,
                format!("verifier could not be spawned: {error}"),
            );
        }
    };
    if result.cancelled {
        return VerificationOutcome::blocked(&pending.id, "verifier was cancelled".to_string());
    }
    if result.backgrounded {
        return VerificationOutcome::blocked(
            &pending.id,
            "verifier did not finish within the foreground budget".to_string(),
        );
    }
    if !result.success {
        return VerificationOutcome::blocked(
            &pending.id,
            format!(
                "verifier failed: {}",
                result.error.unwrap_or_else(|| "unknown error".to_string())
            ),
        );
    }
    match parse_verification_verdict(&result.output) {
        VerificationVerdict::Passed => VerificationOutcome {
            id: pending.id.clone(),
            passed: true,
            detail: "verification passed".to_string(),
        },
        VerificationVerdict::Failed(reason) => {
            VerificationOutcome::blocked(&pending.id, format!("verification failed: {reason}"))
        }
        VerificationVerdict::Unreported => VerificationOutcome::blocked(
            &pending.id,
            "verifier did not report a verdict".to_string(),
        ),
    }
}

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type TodoId = String;

// diff from acp: default to medium
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub const fn tag(&self) -> &str {
        match self {
            Self::Pending => "[pending]",
            Self::InProgress => "[in_progress]",
            Self::Completed => "[completed]",
            Self::Cancelled => "[cancelled]",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    #[serde(default)]
    pub priority: TodoPriority,
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    /// Optional verification prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    /// Whether the current `verification` prompt has passed a verifier run.
    #[serde(default)]
    pub verification_passed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoState {
    todos: IndexMap<TodoId, TodoItem>,
}

crate::register_resource!("grok_build", "Todo", TodoState);

impl TodoState {
    pub fn push(&mut self, id: TodoId, todo: TodoItem) {
        self.todos.insert(id, todo);
    }

    /// Insert at `index`, shifting everything from there on down the list.
    /// An id already in the map keeps its own position and is only updated,
    /// so this can add an item but never move one.
    pub fn insert_at(&mut self, index: usize, id: TodoId, todo: TodoItem) {
        if self.todos.contains_key(&id) {
            self.todos.insert(id, todo);
            return;
        }
        self.todos
            .shift_insert(index.min(self.todos.len()), id, todo);
    }

    // There is deliberately no `clear`, and no remove of any shape. The list
    // is the user's, and an item on it can only be re-worded or moved to
    // `Completed`/`Cancelled` by id. Dropping one silently is what a wholesale
    // rewrite used to do, and nothing may be able to do it again.

    pub fn update(
        &mut self,
        id: &TodoId,
        content: Option<&str>,
        status: Option<TodoStatus>,
    ) -> bool {
        self.update_with_verification(id, content, status, None)
    }

    /// `update` plus an optional new verification prompt. A changed condition
    /// clears `verification_passed`: the new condition has not been verified.
    pub fn update_with_verification(
        &mut self,
        id: &TodoId,
        content: Option<&str>,
        status: Option<TodoStatus>,
        verification: Option<&str>,
    ) -> bool {
        let Some(todo) = self.todos.get_mut(id) else {
            return false;
        };
        if let Some(content) = content
            && !content.is_empty()
        {
            todo.content = content.into();
        }
        if let Some(verification) = verification {
            let verification = verification.trim();
            if !verification.is_empty() && todo.verification.as_deref() != Some(verification) {
                todo.verification = Some(verification.to_string());
                todo.verification_passed = false;
            }
        }
        if let Some(status) = status {
            todo.status = status;
        }
        true
    }

    /// Borrow an item by id.
    pub fn item(&self, id: &TodoId) -> Option<&TodoItem> {
        self.todos.get(id)
    }

    /// Record the outcome of a verifier run for an item.
    pub fn set_verification_passed(&mut self, id: &TodoId, passed: bool) -> bool {
        let Some(todo) = self.todos.get_mut(id) else {
            return false;
        };
        todo.verification_passed = passed;
        true
    }

    /// Change an item's priority without touching its text or status.
    pub fn set_priority(&mut self, id: &TodoId, priority: TodoPriority) -> bool {
        let Some(todo) = self.todos.get_mut(id) else {
            return false;
        };
        todo.priority = priority;
        true
    }

    /// The id of the first item whose text is exactly `content`.
    ///
    /// For a caller whose items carry no id of their own, the text is the only
    /// identity they have.
    pub fn id_with_content(&self, content: &str) -> Option<TodoId> {
        self.todos
            .iter()
            .find(|(_, todo)| todo.content == content)
            .map(|(id, _)| id.clone())
    }

    /// An id no item is using, for appending to a list whose caller supplies
    /// none. Counts up past the numeric ids already present.
    pub fn next_free_numeric_id(&self) -> TodoId {
        let highest = self
            .todos
            .keys()
            .filter_map(|id| id.parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        format!("{}", highest + 1)
    }

    pub fn todo_items(&self) -> impl Iterator<Item = &TodoItem> + '_ {
        self.todos.values()
    }

    pub fn todo_items_with_ids(&self) -> impl Iterator<Item = (&TodoId, &TodoItem)> + '_ {
        self.todos.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
    }

    pub fn has_id(&self, id: &str) -> bool {
        self.todos.contains_key(id)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoUpdate {
    #[schemars(description = "Unique identifier for the todo item")]
    pub id: String,

    #[schemars(description = "The description/content of the todo item")]
    pub content: Option<String>,

    #[schemars(
        description = "The status of the todo item: pending, in_progress, completed, or cancelled"
    )]
    pub status: Option<TodoStatus>,

    #[schemars(
        description = "Optional verification prompt for this item. When set, marking the item `completed` runs an independent verifier subagent against this condition first; the item cannot be completed until that verifier reports it satisfied. Omit to leave the existing condition unchanged, or send a new prompt to replace it (which re-arms verification)."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,

    #[schemars(
        description = "Context handed to the verifier when completing this item: what to check, where the evidence is, anything the verifier cannot infer. Only read when this call marks the item `completed`."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_context: Option<String>,
}

impl TodoUpdate {
    /// True when the update carries no meaningful content (None or empty string).
    fn has_no_content(&self) -> bool {
        self.content.as_deref().is_none_or(str::is_empty)
    }
}

const fn default_merge() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoWriteInput {
    /// When true (the default), merge the provided todos into the existing
    /// list by id (partial updates are allowed — leave unchanged fields
    /// undefined). When explicitly set to false, the provided todos replace
    /// the existing list entirely.
    /// Accepted for wire compatibility and ignored: every write merges.
    ///
    /// Absent from the advertised schema because both values now behave the
    /// same. It used to select a wholesale replace, which cleared the list and
    /// dropped every item the call did not resend — the one way the tool could
    /// destroy work the user put there.
    #[serde(
        default = "default_merge",
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    #[schemars(skip)]
    pub merge: bool,

    #[schemars(
        description = "Todo items to add or update, matched by id. Send only what you are changing; items you omit are left untouched. To flip status without changing the text, send just id + status."
    )]
    pub todos: Vec<TodoUpdate>,

    /// Put new merged items at the front of the list instead of the end.
    ///
    /// Deliberately absent from the advertised schema: the only caller is the
    /// `/TODO` capture path, and a knob the model can reach would let it
    /// reorder the list the user is watching. Keeping it out also leaves the
    /// serialized tool list byte-identical, so the conversation's prompt cache
    /// survives this field.
    #[serde(default)]
    #[schemars(skip)]
    pub prepend: bool,
}

/// New-architecture `TodoWrite` tool. State: `State<TodoState>` — persisted across calls via
/// Resources serde. Params: `()` — no per-tool configuration.
#[derive(Debug, Default)]
pub struct TodoWriteTool;

impl crate::types::tool_metadata::ToolMetadata for TodoWriteTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Plan
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Create and manage a structured task list. The user sees this list live — it is your primary way to show progress.

Add as many items as the work needs — a small task may be one or two, a large one many more. Do not pad a small job into a fake checklist, and do not crush a large job into a handful of vague items. Skip for trivial single-step work. Check items off as you go; keep roughly one in_progress.

Writes merge by id, so send only the items you are changing. An item you leave out is kept exactly as it was: there is no way to remove one. Work leaves the list by status only — completed when it is done, cancelled when it will not be done. Reword an item by sending its id with new content.

An item may carry a `verification` prompt. Marking such an item `completed` runs an independent verifier subagent against that condition first, passing the `verificationContext` the completion call supplies. The item is marked done only if the verifier reports the condition satisfied; otherwise it stays open and the failure is returned to you.

Every call returns the whole list with each item's id. Items can appear that you did not write (the user and the goal planner add them), so call with an empty `todos` array to read the current list and its ids before you update them."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for TodoWriteTool {
    type Args = TodoWriteInput;
    type Output = TodoWriteOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("todo_write").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "todo_write",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "new_tool.todo_write",
        skip_all,
        fields(merge = %input.merge, todo_count = input.todos.len())
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: TodoWriteInput,
    ) -> Result<TodoWriteOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        // Validate IDs upfront — return an error-as-output variant so the
        // Python side can distinguish this from infra errors.
        if let Err(TodoError::DuplicateTodoID(id)) = validate_no_duplicate_ids(&input.todos) {
            return Ok(TodoWriteOutput::DuplicateId(format!(
                "Duplicate todo ID in request: \"{id}\". Each todo item must have a unique ID."
            )));
        }

        // Items this call marks `completed` whose verifier has not passed run
        // an independent verifier subagent BEFORE the completion stands. The
        // merge is applied first so a condition sent on the same call is the
        // one verified; a failed verifier then reverts that item's status.
        let (
            pending_verifications,
            previous_statuses,
            backend,
            parent_session_id,
            parent_prompt_id,
            depth,
            max_depth,
        ) = {
            let mut res = resources.lock().await;
            let todo_state = res.get_or_default::<State<TodoState>>();

            let mut previous_statuses: std::collections::HashMap<TodoId, Option<TodoStatus>> =
                std::collections::HashMap::new();
            for update in &input.todos {
                if update.status == Some(TodoStatus::Completed) {
                    previous_statuses
                        .entry(update.id.clone())
                        .or_insert_with(|| todo_state.0.item(&update.id).map(|item| item.status));
                }
            }

            // Refuse a write that would put more items in progress than the cap allows, before any of it lands.
            let cap = max_in_progress_cap(std::env::var(MAX_IN_PROGRESS_VAR).ok().as_deref());
            let mut projected = todo_state.0.clone();
            apply_merge(&mut projected, &input.todos, input.prepend)?;
            if let Some(message) = in_progress_cap_violation(&projected, cap) {
                return Ok(TodoWriteOutput::TooManyInProgress(message));
            }

            // Always a merge. The list belongs to the user, so a write adds
            // and updates by id and never drops what it leaves out.
            apply_merge(&mut todo_state.0, &input.todos, input.prepend)?;

            let mut pending = Vec::new();
            for update in &input.todos {
                if update.status != Some(TodoStatus::Completed) {
                    continue;
                }
                let Some(item) = todo_state.0.item(&update.id) else {
                    continue;
                };
                if item.verification_passed {
                    continue;
                }
                let Some(condition) = item
                    .verification
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                else {
                    continue;
                };
                pending.push(PendingVerification {
                    id: update.id.clone(),
                    content: item.content.clone(),
                    condition: condition.to_string(),
                    context: update.verification_context.clone(),
                });
            }

            let backend = res.get::<SubagentBackendResource>().cloned();
            let parent_session_id = res
                .get::<SessionIdResource>()
                .map(|s| s.0.clone())
                .unwrap_or_default();
            let parent_prompt_id = res
                .get::<CurrentPromptIdResource>()
                .map(|p| p.0.clone())
                .filter(|p| !p.is_empty());
            let depth = res.get::<SubagentDepthCounter>().map(|d| d.0).unwrap_or(0);
            let max_depth = effective_max_subagent_depth(&res);

            (
                pending,
                previous_statuses,
                backend,
                parent_session_id,
                parent_prompt_id,
                depth,
                max_depth,
            )
        };

        // Run each verifier in its own fresh subagent, outside the state lock.
        let mut results = Vec::new();
        for pending in &pending_verifications {
            results.push(
                run_verifier(
                    &ctx,
                    backend.as_ref(),
                    &parent_session_id,
                    parent_prompt_id.clone(),
                    depth,
                    max_depth,
                    pending,
                )
                .await,
            );
        }

        let (summary_for_prompt, todos, state_snapshot);
        {
            let mut res = resources.lock().await;
            let todo_state = res.get_or_default::<State<TodoState>>();

            for outcome in &results {
                if outcome.passed {
                    todo_state.0.set_verification_passed(&outcome.id, true);
                } else {
                    // A blocked verifier takes the completion back: the item
                    // returns to the status it held before this call.
                    let previous = previous_statuses
                        .get(&outcome.id)
                        .copied()
                        .flatten()
                        .unwrap_or(TodoStatus::Pending);
                    todo_state.0.update(&outcome.id, None, Some(previous));
                }
            }

            let mut summary = summarize_todo_state(&todo_state.0);
            for outcome in results.iter().filter(|outcome| !outcome.passed) {
                writeln!(
                    &mut summary,
                    "\nVERIFICATION BLOCKED todo `{}`: {}. The item was not marked completed; \
                     address the failure and mark it completed again to re-run the verifier.",
                    outcome.id, outcome.detail
                )
                .ok();
            }

            summary_for_prompt = summary;
            todos = todo_state.0.todo_items().cloned().collect::<Vec<_>>();
            state_snapshot = todo_state.0.clone();
        }

        Ok(TodoWriteOutput::TodosUpdated(TodoWriteSuccess {
            summary_for_prompt,
            todos,
            state: state_snapshot,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::output::TodoWriteOutput;
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::test_ctx;

    // -- Helpers --

    fn make_update(id: &str, content: Option<&str>, status: Option<TodoStatus>) -> TodoUpdate {
        TodoUpdate {
            id: id.to_owned(),
            content: content.map(str::to_owned),
            status,
            verification: None,
            verification_context: None,
        }
    }

    /// Unwrap a `TodoWriteOutput` expecting the `TodosUpdated` variant.
    fn expect_success(output: TodoWriteOutput) -> TodoWriteSuccess {
        match output {
            TodoWriteOutput::TodosUpdated(s) => s,
            other => panic!("expected TodosUpdated, got {other:?}"),
        }
    }

    // -- Tests --

    #[test]
    fn the_in_progress_cap_defaults_to_five_and_parses_an_override() {
        assert_eq!(max_in_progress_cap(None), 5);
        assert_eq!(max_in_progress_cap(Some("2")), 2);
        assert_eq!(max_in_progress_cap(Some(" 7 ")), 7);
        // A value that is not a positive number keeps the default.
        assert_eq!(max_in_progress_cap(Some("nope")), 5);
        assert_eq!(max_in_progress_cap(Some("0")), 5);
    }

    #[test]
    fn a_state_over_the_cap_is_refused_with_the_count_and_the_cap() {
        let mut state = TodoState::default();
        let running = || TodoItem {
            content: "work".to_string(),
            priority: TodoPriority::Medium,
            status: TodoStatus::InProgress,
            meta: None,
            verification: None,
            verification_passed: false,
        };
        for i in 0..5 {
            state.push(format!("a{i}").into(), running());
        }
        assert!(in_progress_cap_violation(&state, 5).is_none());

        state.push("a5".into(), running());
        let message = in_progress_cap_violation(&state, 5).expect("six in progress is over five");
        assert!(
            message.contains("6 items would be in progress"),
            "{message}"
        );
        assert!(message.contains("over the cap of 5"), "{message}");
    }

    #[tokio::test]
    async fn the_tool_refuses_a_sixth_in_progress_item_and_leaves_the_list_alone() {
        let tool = TodoWriteTool;
        let shared = Resources::new().into_shared();

        let at_cap: Vec<TodoUpdate> = (0..5)
            .map(|i| make_update(&format!("a{i}"), Some("work"), Some(TodoStatus::InProgress)))
            .collect();
        let output = expect_success(
            xai_tool_runtime::Tool::run(
                &tool,
                test_ctx(shared.clone()),
                TodoWriteInput {
                    merge: true,
                    prepend: false,
                    todos: at_cap,
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(output.todos.len(), 5);

        let refused = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(shared.clone()),
            TodoWriteInput {
                merge: true,
                prepend: false,
                todos: vec![make_update(
                    "a5",
                    Some("one more"),
                    Some(TodoStatus::InProgress),
                )],
            },
        )
        .await
        .unwrap();
        let TodoWriteOutput::TooManyInProgress(message) = refused else {
            panic!("expected the cap to refuse the write, got {refused:?}");
        };
        assert!(message.contains("over the cap of 5"), "{message}");

        // Nothing the refused write carried landed.
        let after = expect_success(
            xai_tool_runtime::Tool::run(
                &tool,
                test_ctx(shared.clone()),
                TodoWriteInput {
                    merge: true,
                    prepend: false,
                    todos: vec![make_update("a0", None, None)],
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(after.todos.len(), 5, "the refused write added an item");
    }

    #[test]
    fn name_and_description() {
        let tool = TodoWriteTool;
        assert_eq!(xai_tool_runtime::Tool::id(&tool).as_str(), "todo_write");
        let desc = crate::types::tool_metadata::ToolMetadata::description_template(&tool);
        assert!(desc.contains("task list"));
        assert!(
            desc.contains("as many items as the work needs"),
            "todo_write must not cap the list at 4–6 / 3+: {desc}"
        );
        assert!(!desc.contains("3+ steps"), "{desc}");
    }

    #[tokio::test]
    async fn a_first_write_creates_items() {
        let tool = TodoWriteTool;
        let resources = Resources::new();

        let input = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![
                make_update("1", Some("Task A"), Some(TodoStatus::Pending)),
                make_update("2", Some("Task B"), Some(TodoStatus::InProgress)),
            ],
        };

        let shared = resources.into_shared();
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input)
                .await
                .unwrap(),
        );
        assert_eq!(output.todos.len(), 2);
        assert!(output.summary_for_prompt.contains("Task A"));
        assert!(output.summary_for_prompt.contains("Task B"));

        // State persists in Resources
        let res = shared.lock().await;
        let state = res.get::<State<TodoState>>().unwrap();
        assert_eq!(state.0.todo_items().count(), 2);
    }

    /// `merge: false` was a wholesale replace: it cleared the list and kept
    /// only what the call resent. Through the tool, with the flag still set
    /// the destructive way, the earlier item has to survive — it is the
    /// user's, and only a status can retire it.
    #[tokio::test]
    async fn a_write_cannot_discard_what_it_omits() {
        let tool = TodoWriteTool;
        let resources = Resources::new();
        let shared = resources.into_shared();

        let input1 = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![make_update(
                "old",
                Some("Old task"),
                Some(TodoStatus::InProgress),
            )],
        };
        xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input1)
            .await
            .unwrap();

        let input2 = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![make_update(
                "new",
                Some("New task"),
                Some(TodoStatus::Pending),
            )],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input2)
                .await
                .unwrap(),
        );
        assert_eq!(output.todos.len(), 2, "the omitted item is still there");
        assert!(output.summary_for_prompt.contains("New task"));
        assert!(output.summary_for_prompt.contains("Old task"));
        assert!(
            output.summary_for_prompt.contains("[in_progress]"),
            "the survivor keeps its status: {}",
            output.summary_for_prompt
        );
    }

    #[tokio::test]
    async fn merge_mode_updates_existing() {
        let tool = TodoWriteTool;
        let resources = Resources::new();
        let shared = resources.into_shared();

        // Create initial items
        let input1 = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![
                make_update("1", Some("Build project"), Some(TodoStatus::InProgress)),
                make_update("2", Some("Run tests"), Some(TodoStatus::Pending)),
            ],
        };
        xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input1)
            .await
            .unwrap();

        // Merge: mark item 1 completed (no content), add item 3
        let input2 = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![
                make_update("1", None, Some(TodoStatus::Completed)),
                make_update("3", Some("Deploy"), Some(TodoStatus::Pending)),
            ],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input2)
                .await
                .unwrap(),
        );
        assert_eq!(output.todos.len(), 3);

        // Item 1 content preserved, status updated
        let item1 = output
            .todos
            .iter()
            .find(|t| t.content == "Build project")
            .unwrap();
        assert_eq!(item1.status, TodoStatus::Completed);
    }

    #[tokio::test]
    async fn merge_with_lost_state_uses_id_fallback() {
        let tool = TodoWriteTool;
        let resources = Resources::new();

        // Merge into empty state — should not error
        let input = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![make_update("explore", None, Some(TodoStatus::Completed))],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap(),
        );
        let [todo] = output.todos.as_slice() else {
            panic!("expected exactly one todo, got {}", output.todos.len());
        };
        // Id used as fallback content
        assert_eq!(todo.content, "explore");
        assert_eq!(todo.status, TodoStatus::Completed);
    }

    #[tokio::test]
    async fn duplicate_ids_rejected() {
        let tool = TodoWriteTool;
        let resources = Resources::new();

        let input = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![
                make_update("dup", Some("A"), Some(TodoStatus::Pending)),
                make_update("dup", Some("B"), Some(TodoStatus::Pending)),
            ],
        };
        let result = xai_tool_runtime::Tool::run(&tool, test_ctx(resources.into_shared()), input)
            .await
            .unwrap();
        assert!(
            matches!(result, TodoWriteOutput::DuplicateId(ref msg) if msg.contains("dup")),
            "expected DuplicateId variant, got {result:?}"
        );
    }

    #[tokio::test]
    async fn empty_todos_shows_no_tasks_message() {
        let tool = TodoWriteTool;
        let resources = Resources::new();

        let input = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap(),
        );
        assert!(output.summary_for_prompt.contains("No tasks"));
        assert!(output.todos.is_empty());
    }

    /// The description promises an empty call as the read. It must change
    /// nothing and must return every item with an id the model did not mint.
    #[tokio::test]
    async fn an_empty_write_reads_the_list_back_with_ids() {
        let tool = TodoWriteTool;
        let shared = Resources::new().into_shared();
        let seed = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![make_update(
                "plan-3f9a2c",
                Some("Write the migration"),
                Some(TodoStatus::Pending),
            )],
        };
        xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), seed)
            .await
            .unwrap();

        let read = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(shared), read)
                .await
                .unwrap(),
        );
        assert_eq!(
            output.summary_for_prompt,
            "- [pending] plan-3f9a2c: Write the migration\n"
        );
        assert_eq!(output.todos.len(), 1);
        let desc = crate::types::tool_metadata::ToolMetadata::description_template(&tool);
        assert!(desc.contains("empty `todos` array to read"), "{desc}");
    }

    #[tokio::test]
    async fn state_output_includes_snapshot() {
        let tool = TodoWriteTool;
        let resources = Resources::new();

        let input = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![make_update("1", Some("Task"), Some(TodoStatus::Pending))],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap(),
        );

        // state field should match what's in Resources
        assert!(!output.state.is_empty());
        assert_eq!(output.state.todo_items().count(), 1);
    }

    #[tokio::test]
    async fn state_serialization_roundtrip() {
        let tool = TodoWriteTool;
        let mut resources = Resources::new();
        resources.register_state::<TodoState>();

        // Create some state
        let input = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![
                make_update("1", Some("First"), Some(TodoStatus::Completed)),
                make_update("2", Some("Second"), Some(TodoStatus::InProgress)),
            ],
        };
        let shared = resources.into_shared();
        xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input)
            .await
            .unwrap();

        // Serialize
        let res = shared.lock().await;
        let snapshot = res.serialize();
        let state_map = snapshot.get("state").unwrap();
        assert!(
            state_map.get("grok_build.Todo").is_some(),
            "TodoState should serialize under 'grok_build.Todo'"
        );

        // Deserialize into fresh Resources
        let mut resources2 = Resources::new();
        resources2.register_state::<TodoState>();
        let data: std::collections::HashMap<
            String,
            std::collections::HashMap<String, serde_json::Value>,
        > = serde_json::from_value(snapshot).unwrap();
        resources2.load_from(data);

        // Verify state was restored
        let restored = resources2.get::<State<TodoState>>().unwrap();
        assert_eq!(restored.0.todo_items().count(), 2);
        let items: Vec<_> = restored.0.todo_items().collect();
        let [first, second] = items.as_slice() else {
            panic!("expected two items: {items:?}");
        };
        assert_eq!(first.content, "First");
        assert_eq!(first.status, TodoStatus::Completed);
        assert_eq!(second.content, "Second");
        assert_eq!(second.status, TodoStatus::InProgress);
    }

    fn seed_state(items: &[(&str, &str, TodoStatus)]) -> TodoState {
        let mut state = TodoState::default();
        for (id, content, status) in items {
            state.push(
                id.to_string(),
                TodoItem {
                    content: content.to_string(),
                    priority: TodoPriority::default(),
                    status: *status,
                    meta: None,
                    verification: None,
                    verification_passed: false,
                },
            );
        }
        state
    }

    #[test]
    fn a_finished_item_echoes_only_its_first_line() {
        let long_open = format!("open task\n{}", "detail ".repeat(40));
        let long_done = format!("shipped the parser\n{}", "detail ".repeat(40));
        let state = seed_state(&[
            ("1", &long_open, TodoStatus::InProgress),
            ("2", &long_done, TodoStatus::Completed),
            ("3", "dropped", TodoStatus::Cancelled),
            ("4", &"x".repeat(300), TodoStatus::Completed),
        ]);
        let echo = summarize_todo_state(&state);
        assert!(
            echo.contains(&long_open),
            "an open item keeps its text:\n{echo}"
        );
        assert!(
            echo.contains("- [completed] 2: shipped the parser…\n"),
            "{echo}"
        );
        assert!(!echo.contains(&long_done), "{echo}");
        assert!(echo.contains("- [cancelled] 3: dropped\n"), "{echo}");
        assert!(
            echo.contains(&format!("- [completed] 4: {}…\n", "x".repeat(100))),
            "{echo}"
        );
        assert_eq!(
            get_item(&state, "2").content,
            long_done,
            "the state keeps the full text"
        );
    }

    fn get_item<'a>(state: &'a TodoState, id: &str) -> &'a TodoItem {
        state
            .todo_items_with_ids()
            .find(|(i, _)| *i == id)
            .map(|(_, item)| item)
            .unwrap_or_else(|| panic!("item {id} not found in state"))
    }

    // ── replace (merge=false) ────────────────────────────────────────

    #[test]
    fn a_write_without_content_falls_back_to_id() {
        let mut state = TodoState::default();
        let updates = vec![make_update(
            "build_project",
            None,
            Some(TodoStatus::Pending),
        )];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "build_project");
        assert_eq!(item.content, "build_project"); // id used as fallback
        assert_eq!(item.status, TodoStatus::Pending);
    }

    #[test]
    fn a_write_without_content_or_status_defaults() {
        let mut state = TodoState::default();
        let updates = vec![make_update("task_1", None, None)];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "task_1");
        assert_eq!(item.content, "task_1");
        assert_eq!(item.status, TodoStatus::Pending);
    }

    #[test]
    fn a_write_with_content_succeeds() {
        let mut state = TodoState::default();
        let updates = vec![
            make_update("1", Some("Task A"), Some(TodoStatus::Pending)),
            make_update("2", Some("Task B"), Some(TodoStatus::InProgress)),
        ];
        apply_merge(&mut state, &updates, false).unwrap();

        assert_eq!(get_item(&state, "1").content, "Task A");
        assert_eq!(get_item(&state, "1").status, TodoStatus::Pending);
        assert_eq!(get_item(&state, "2").content, "Task B");
        assert_eq!(get_item(&state, "2").status, TodoStatus::InProgress);
    }

    /// The write that used to be a replace. Sending one brand-new item is not
    /// a statement that everything else is finished, so the item the call does
    /// not mention has to survive it.
    #[test]
    fn a_write_that_omits_an_item_keeps_it() {
        let mut state = seed_state(&[("old", "Old task", TodoStatus::InProgress)]);
        let updates = vec![make_update(
            "new",
            Some("New task"),
            Some(TodoStatus::Pending),
        )];
        apply_merge(&mut state, &updates, false).unwrap();

        let old = get_item(&state, "old");
        assert_eq!(old.content, "Old task");
        assert_eq!(
            old.status,
            TodoStatus::InProgress,
            "an omitted item keeps its status too — it is not quietly finished"
        );
        assert_eq!(get_item(&state, "new").content, "New task");
    }

    /// The three ways work is allowed to change, and the fact that none of
    /// them shortens the list.
    #[test]
    fn completing_cancelling_and_rewording_all_keep_the_item() {
        let mut state = seed_state(&[
            ("a", "Ship it", TodoStatus::InProgress),
            ("b", "Drop it", TodoStatus::Pending),
            ("c", "Reword me", TodoStatus::Pending),
        ]);
        apply_merge(
            &mut state,
            &[
                make_update("a", None, Some(TodoStatus::Completed)),
                make_update("b", None, Some(TodoStatus::Cancelled)),
                make_update("c", Some("Reworded"), None),
            ],
            false,
        )
        .unwrap();

        let ids: Vec<&str> = state
            .todo_items_with_ids()
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c"], "nothing left the list");
        assert_eq!(get_item(&state, "a").status, TodoStatus::Completed);
        assert_eq!(get_item(&state, "b").status, TodoStatus::Cancelled);
        assert_eq!(get_item(&state, "c").content, "Reworded");
        assert_eq!(
            get_item(&state, "c").status,
            TodoStatus::Pending,
            "a reword must not also move the item's status"
        );
    }

    // ── merge (merge=true) ───────────────────────────────────────────

    #[test]
    fn merge_existing_item_status_only() {
        // The core use-case: mark in_progress → completed without sending content.
        let mut state = seed_state(&[("1", "Build the project", TodoStatus::InProgress)]);
        let updates = vec![make_update("1", None, Some(TodoStatus::Completed))];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "1");
        assert_eq!(item.status, TodoStatus::Completed);
        assert_eq!(item.content, "Build the project"); // unchanged
    }

    #[test]
    fn merge_existing_item_content_and_status() {
        let mut state = seed_state(&[("1", "Old text", TodoStatus::Pending)]);
        let updates = vec![make_update(
            "1",
            Some("New text"),
            Some(TodoStatus::InProgress),
        )];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "1");
        assert_eq!(item.content, "New text");
        assert_eq!(item.status, TodoStatus::InProgress);
    }

    #[test]
    fn merge_existing_item_no_fields_is_noop() {
        let mut state = seed_state(&[("1", "Keep me", TodoStatus::Pending)]);
        let updates = vec![make_update("1", None, None)];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "1");
        assert_eq!(item.content, "Keep me");
        assert_eq!(item.status, TodoStatus::Pending);
    }

    #[test]
    fn merge_new_item_without_content_uses_id_fallback() {
        // When state is empty (e.g. lost between calls) and content is None,
        // the id is used as fallback content instead of erroring.
        let mut state = TodoState::default();
        let updates = vec![make_update(
            "explore_codebase",
            None,
            Some(TodoStatus::Completed),
        )];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "explore_codebase");
        assert_eq!(item.content, "explore_codebase"); // id used as fallback
        assert_eq!(item.status, TodoStatus::Completed);
    }

    #[test]
    fn merge_new_item_without_content_or_status_defaults_to_pending() {
        let mut state = TodoState::default();
        let updates = vec![make_update("task_1", None, None)];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "task_1");
        assert_eq!(item.content, "task_1");
        assert_eq!(item.status, TodoStatus::Pending);
    }

    #[test]
    fn merge_new_item_with_content_succeeds() {
        let mut state = TodoState::default();
        let updates = vec![make_update(
            "1",
            Some("Fresh task"),
            Some(TodoStatus::Pending),
        )];
        apply_merge(&mut state, &updates, false).unwrap();

        assert_eq!(get_item(&state, "1").content, "Fresh task");
    }

    /// `/TODO` puts what the user just asked for where they will see it
    /// first, in the order they asked for it, without disturbing the work the
    /// agent is already tracking.
    #[test]
    fn prepend_puts_new_items_first_in_order_and_leaves_existing_ones_alone() {
        let mut state = seed_state(&[
            ("a", "already first", TodoStatus::InProgress),
            ("b", "already second", TodoStatus::Pending),
        ]);
        let updates = vec![
            make_update("u1", Some("urgent one"), Some(TodoStatus::Pending)),
            make_update("u2", Some("urgent two"), Some(TodoStatus::Pending)),
        ];
        apply_merge(&mut state, &updates, true).unwrap();

        let ids: Vec<&str> = state
            .todo_items_with_ids()
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(ids, ["u1", "u2", "a", "b"]);
        assert_eq!(get_item(&state, "a").status, TodoStatus::InProgress);
        assert_eq!(get_item(&state, "a").content, "already first");
    }

    /// A prepend that names an existing id updates it in place. Moving the
    /// agent's in-progress item to the top is a reorder, which this path must
    /// never perform.
    #[test]
    fn prepend_does_not_move_an_existing_item() {
        let mut state = seed_state(&[
            ("a", "first", TodoStatus::Pending),
            ("b", "second", TodoStatus::InProgress),
        ]);
        let updates = vec![make_update("b", Some("reworded"), None)];
        apply_merge(&mut state, &updates, true).unwrap();

        let ids: Vec<&str> = state
            .todo_items_with_ids()
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(get_item(&state, "b").content, "reworded");
    }

    /// The field is for the capture path only. A model that never learns it
    /// exists cannot use it, and keeping it out of the schema also keeps the
    /// serialized tool list unchanged.
    #[test]
    fn prepend_is_absent_from_the_advertised_schema() {
        let schema = serde_json::to_string(&schemars::schema_for!(TodoWriteInput)).unwrap();
        assert!(schema.contains("todos"), "{schema}");
        assert!(!schema.contains("prepend"), "{schema}");
    }

    /// Older callers and every model call omit the field entirely.
    #[test]
    fn prepend_defaults_to_off_when_absent() {
        let input: TodoWriteInput =
            serde_json::from_value(serde_json::json!({"todos": [{"id": "1"}]})).unwrap();
        assert!(!input.prepend);
        assert!(input.merge, "merge still defaults on");
    }

    #[test]
    fn merge_mixed_existing_and_new() {
        let mut state = seed_state(&[("exist", "Existing task", TodoStatus::InProgress)]);
        let updates = vec![
            // Update existing — content omitted, just flip status.
            make_update("exist", None, Some(TodoStatus::Completed)),
            // Brand-new item — content required.
            make_update("fresh", Some("New task"), Some(TodoStatus::Pending)),
        ];
        apply_merge(&mut state, &updates, false).unwrap();

        let existing = get_item(&state, "exist");
        assert_eq!(existing.status, TodoStatus::Completed);
        assert_eq!(existing.content, "Existing task"); // preserved

        let fresh = get_item(&state, "fresh");
        assert_eq!(fresh.content, "New task");
        assert_eq!(fresh.status, TodoStatus::Pending);
    }

    // ── duplicate id validation ──────────────────────────────────────

    #[test]
    fn duplicate_ids_rejected_unit() {
        let updates = vec![
            make_update("dup", Some("A"), Some(TodoStatus::Pending)),
            make_update("dup", Some("B"), Some(TodoStatus::Pending)),
        ];
        let err = validate_no_duplicate_ids(&updates).unwrap_err();
        assert!(matches!(err, TodoError::DuplicateTodoID(ref id) if id == "dup"));
    }

    #[test]
    fn unique_ids_accepted() {
        let updates = vec![
            make_update("a", Some("A"), Some(TodoStatus::Pending)),
            make_update("b", Some("B"), Some(TodoStatus::Pending)),
        ];
        validate_no_duplicate_ids(&updates).unwrap();
    }

    // ── regression: missing merge=true auto-upgrade ────────────────────

    #[tokio::test]
    async fn a_status_only_write_updates_in_place_whatever_the_flag_says() {
        // Regression: status-only update without merge=true must not wipe content.
        let tool = TodoWriteTool;
        let resources = Resources::new();
        let shared = resources.into_shared();

        // Create todos with content
        let input1 = TodoWriteInput {
            merge: false,
            prepend: false,
            todos: vec![
                make_update("1", Some("Explore codebase"), Some(TodoStatus::InProgress)),
                make_update("2", Some("Review tools"), Some(TodoStatus::Pending)),
                make_update("3", Some("Write tests"), Some(TodoStatus::Pending)),
            ],
        };
        xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input1)
            .await
            .unwrap();

        // Status-only update without merge=true
        let input2 = TodoWriteInput {
            merge: false, // model forgot merge: true
            prepend: false,
            todos: vec![
                make_update("1", None, Some(TodoStatus::Completed)),
                make_update("2", None, Some(TodoStatus::Completed)),
                make_update("3", None, Some(TodoStatus::InProgress)),
            ],
        };
        let output = expect_success(
            xai_tool_runtime::Tool::run(&tool, test_ctx(shared.clone()), input2)
                .await
                .unwrap(),
        );

        // Content must be preserved, not replaced with id fallback.
        let [first, second, third] = output.todos.as_slice() else {
            panic!("expected three todos: {:?}", output.todos);
        };
        assert_eq!(first.content, "Explore codebase");
        assert_eq!(first.status, TodoStatus::Completed);
        assert_eq!(second.content, "Review tools");
        assert_eq!(second.status, TodoStatus::Completed);
        assert_eq!(third.content, "Write tests");
        assert_eq!(third.status, TodoStatus::InProgress);
    }

    // ── regression: merge with null content should never error ────────

    #[test]
    fn merge_after_replace_status_update_with_null_content() {
        // Reproduces the exact scenario from the bug report:
        // 1. Replace creates 3 items
        // 2. Merge updates 2 items with content=null, status changed
        let mut state = TodoState::default();

        // Step 1: replace (merge=false)
        let initial = vec![
            make_update(
                "explore_codebase",
                Some("Explore django/db/backends/sqlite3/"),
                Some(TodoStatus::InProgress),
            ),
            make_update(
                "analyze_and_propose",
                Some("Analyze current SQLite min version"),
                Some(TodoStatus::Pending),
            ),
            make_update(
                "implementation",
                Some("Update version checks"),
                Some(TodoStatus::Pending),
            ),
        ];
        apply_merge(&mut state, &initial, false).unwrap();

        // Step 2: content=null, just status changes
        let updates = vec![
            make_update("explore_codebase", None, Some(TodoStatus::Completed)),
            make_update("analyze_and_propose", None, Some(TodoStatus::InProgress)),
        ];
        apply_merge(&mut state, &updates, false).unwrap();

        // Statuses flipped, content preserved from step 1.
        assert_eq!(
            get_item(&state, "explore_codebase").status,
            TodoStatus::Completed
        );
        assert_eq!(
            get_item(&state, "explore_codebase").content,
            "Explore django/db/backends/sqlite3/"
        );
        assert_eq!(
            get_item(&state, "analyze_and_propose").status,
            TodoStatus::InProgress
        );
        assert_eq!(
            get_item(&state, "analyze_and_propose").content,
            "Analyze current SQLite min version"
        );
        // Third item unchanged.
        assert_eq!(
            get_item(&state, "implementation").status,
            TodoStatus::Pending
        );
    }

    // ── regression: empty-string content must not wipe existing content ──

    #[test]
    fn merge_existing_item_empty_string_content_preserves_original() {
        // Model sends content: "" instead of omitting it. Must not wipe.
        let mut state = seed_state(&[("1", "Build the project", TodoStatus::InProgress)]);
        let updates = vec![make_update("1", Some(""), Some(TodoStatus::Completed))];
        apply_merge(&mut state, &updates, false).unwrap();

        let item = get_item(&state, "1");
        assert_eq!(item.status, TodoStatus::Completed);
        assert_eq!(item.content, "Build the project"); // unchanged
    }

    #[test]
    fn merge_new_item_empty_string_content_falls_back_to_id() {
        let mut state = TodoState::default();
        let updates = vec![make_update("task_1", Some(""), Some(TodoStatus::Pending))];
        apply_merge(&mut state, &updates, false).unwrap();

        assert_eq!(get_item(&state, "task_1").content, "task_1");
    }

    #[test]
    fn merge_with_null_content_and_lost_state() {
        // Same scenario but state was lost between calls (empty state).
        // The tool should still not error — falls back to id as content.
        let mut state = TodoState::default();

        let updates = vec![
            make_update("explore_codebase", None, Some(TodoStatus::Completed)),
            make_update("analyze_and_propose", None, Some(TodoStatus::InProgress)),
        ];
        apply_merge(&mut state, &updates, false).unwrap();

        assert_eq!(
            get_item(&state, "explore_codebase").content,
            "explore_codebase"
        );
        assert_eq!(
            get_item(&state, "explore_codebase").status,
            TodoStatus::Completed
        );
        assert_eq!(
            get_item(&state, "analyze_and_propose").content,
            "analyze_and_propose"
        );
        assert_eq!(
            get_item(&state, "analyze_and_propose").status,
            TodoStatus::InProgress
        );
    }

    // ── verifier enforcement ─────────────────────────────────────────

    use crate::implementations::grok_build::task::backend::ChannelBackend;
    use crate::implementations::grok_build::task::types::{
        CurrentPromptIdResource, SessionIdResource, SubagentDepthCounter, SubagentEvent,
        SubagentRequest, SubagentResult, SubagentValidateTypeOutcome,
    };
    use crate::types::resources::SharedResources;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    struct VerifierHarness {
        backend: crate::implementations::grok_build::task::backend::SubagentBackendResource,
        requests: Arc<Mutex<Vec<SubagentRequest>>>,
    }

    /// A coordinator backend that answers `ValidateType` with `Ok` and every
    /// spawn with `reply`. Each spawn request is recorded so a test can
    /// inspect the subagent the tool asked for.
    fn verifier_backend(
        reply: impl Fn(&SubagentRequest) -> SubagentResult + Send + Sync + 'static,
    ) -> VerifierHarness {
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<SubagentEvent>();
        let backend = crate::implementations::grok_build::task::backend::SubagentBackendResource(
            Arc::new(ChannelBackend::new(raw_tx)),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let reply: Arc<dyn Fn(&SubagentRequest) -> SubagentResult + Send + Sync> = Arc::new(reply);
        tokio::spawn(async move {
            while let Some(event) = raw_rx.recv().await {
                match event {
                    SubagentEvent::ValidateType(req) => {
                        let _ = req.respond_to.send(SubagentValidateTypeOutcome::Ok);
                    }
                    SubagentEvent::Spawn(req) => {
                        let result = reply(&req.request);
                        recorded.lock().unwrap().push((*req.request).clone());
                        let _ = req.respond_with(move |_| result);
                    }
                    _ => {}
                }
            }
        });
        VerifierHarness { backend, requests }
    }

    fn verifier_resources(harness: &VerifierHarness) -> SharedResources {
        let mut resources = Resources::new();
        resources.insert(harness.backend.clone());
        resources.insert(SubagentDepthCounter(0));
        resources.insert(SessionIdResource("parent".to_string()));
        resources.insert(CurrentPromptIdResource("prompt-1".to_string()));
        resources.into_shared()
    }

    fn passed_result() -> SubagentResult {
        SubagentResult {
            success: true,
            output: format!("checked it\n{VERIFIER_PASS_MARKER}\n").into(),
            subagent_id: "verifier".into(),
            child_session_id: "verifier".into(),
            ..Default::default()
        }
    }

    fn failed_result(reason: &str) -> SubagentResult {
        SubagentResult {
            success: true,
            output: format!("looked, not satisfied\n{VERIFIER_FAIL_MARKER}: {reason}\n").into(),
            subagent_id: "verifier".into(),
            child_session_id: "verifier".into(),
            ..Default::default()
        }
    }

    /// Seed an item carrying a verifier through the shipped tool path.
    async fn seed_verifying_item(shared: &SharedResources, id: &str, condition: &str) {
        let input = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![TodoUpdate {
                id: id.to_string(),
                content: Some("Ship the parser".to_string()),
                status: Some(TodoStatus::Pending),
                verification: Some(condition.to_string()),
                verification_context: None,
            }],
        };
        xai_tool_runtime::Tool::run(&TodoWriteTool, test_ctx(shared.clone()), input)
            .await
            .unwrap();
    }

    /// Mark an item completed through the shipped tool path, supplying context.
    async fn complete_item(
        shared: &SharedResources,
        id: &str,
        context: Option<&str>,
    ) -> TodoWriteSuccess {
        let input = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![TodoUpdate {
                id: id.to_string(),
                content: None,
                status: Some(TodoStatus::Completed),
                verification: None,
                verification_context: context.map(str::to_owned),
            }],
        };
        expect_success(
            xai_tool_runtime::Tool::run(&TodoWriteTool, test_ctx(shared.clone()), input)
                .await
                .unwrap(),
        )
    }

    fn item_named<'a>(success: &'a TodoWriteSuccess, content: &str) -> &'a TodoItem {
        success
            .todos
            .iter()
            .find(|todo| todo.content == content)
            .unwrap_or_else(|| panic!("item {content:?} not in {success:?}"))
    }

    /// The completion is refused and the item stays open.
    #[tokio::test]
    async fn a_verifier_item_cannot_be_completed_when_no_verifier_can_run() {
        let shared = Resources::new().into_shared();
        seed_verifying_item(&shared, "1", "cargo test -p parser passes").await;

        let success = complete_item(&shared, "1", Some("the test output is in out/test.log")).await;

        let item = item_named(&success, "Ship the parser");
        assert_ne!(
            item.status,
            TodoStatus::Completed,
            "an item with an unrun verifier must not be completed"
        );
        assert!(!item.verification_passed);
        assert!(
            success.summary_for_prompt.contains("VERIFICATION BLOCKED"),
            "the refusal must be reported: {}",
            success.summary_for_prompt
        );
    }

    #[tokio::test]
    async fn a_passing_verifier_allows_completion() {
        let harness = verifier_backend(|_| passed_result());
        let shared = verifier_resources(&harness);
        seed_verifying_item(&shared, "1", "cargo test -p parser passes").await;

        let success = complete_item(&shared, "1", None).await;

        let item = item_named(&success, "Ship the parser");
        assert_eq!(item.status, TodoStatus::Completed);
        assert!(item.verification_passed);
    }

    #[tokio::test]
    async fn a_failing_verifier_blocks_completion() {
        let harness = verifier_backend(|_| failed_result("build is broken"));
        let shared = verifier_resources(&harness);
        seed_verifying_item(&shared, "1", "the build succeeds").await;

        let success = complete_item(&shared, "1", None).await;

        let item = item_named(&success, "Ship the parser");
        assert_ne!(item.status, TodoStatus::Completed);
        assert!(!item.verification_passed);
        assert!(
            success.summary_for_prompt.contains("build is broken"),
            "the verifier's reason must reach the model: {}",
            success.summary_for_prompt
        );
    }

    #[tokio::test]
    async fn the_verifier_runs_in_a_fresh_subagent_with_the_supplied_context() {
        let harness = verifier_backend(|_| passed_result());
        let shared = verifier_resources(&harness);
        seed_verifying_item(&shared, "1", "cargo test -p parser passes").await;

        complete_item(&shared, "1", Some("the evidence is in out/report.json")).await;

        let requests = harness.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "exactly one verifier runs per completion"
        );
        let request = &requests[0];
        assert!(
            !request.fork_context,
            "the verifier must not be a fork of the conversation"
        );
        assert!(request.resume_from.is_none(), "no inherited transcript");
        assert_eq!(request.subagent_type, VERIFIER_SUBAGENT_TYPE);
        assert!(
            request.prompt.contains("cargo test -p parser passes"),
            "the condition must reach the verifier: {}",
            request.prompt
        );
        assert!(
            request.prompt.contains("out/report.json"),
            "the supplied context must reach the verifier: {}",
            request.prompt
        );
        assert!(!request.run_in_background);
        assert!(
            !request.surface_completion,
            "the verdict returns through the tool call, not an idle reminder"
        );
    }

    /// An item with no verifier is completed without spawning anything.
    #[tokio::test]
    async fn an_item_without_a_verifier_completes_without_spawning() {
        let harness = verifier_backend(|_| passed_result());
        let shared = verifier_resources(&harness);

        let seed = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![make_update(
                "1",
                Some("Plain task"),
                Some(TodoStatus::Pending),
            )],
        };
        xai_tool_runtime::Tool::run(&TodoWriteTool, test_ctx(shared.clone()), seed)
            .await
            .unwrap();

        let success = complete_item(&shared, "1", None).await;

        assert_eq!(
            item_named(&success, "Plain task").status,
            TodoStatus::Completed
        );
        assert!(harness.requests.lock().unwrap().is_empty());
    }

    /// A rewording of the condition re-arms verification: a passed condition
    /// does not carry over to a new one.
    #[tokio::test]
    async fn rewording_the_condition_re_arms_verification() {
        let harness = verifier_backend(|_| passed_result());
        let shared = verifier_resources(&harness);
        seed_verifying_item(&shared, "1", "first condition").await;
        complete_item(&shared, "1", None).await;

        let reword = TodoWriteInput {
            merge: true,
            prepend: false,
            todos: vec![TodoUpdate {
                id: "1".to_string(),
                content: None,
                status: Some(TodoStatus::InProgress),
                verification: Some("a stricter condition".to_string()),
                verification_context: None,
            }],
        };
        xai_tool_runtime::Tool::run(&TodoWriteTool, test_ctx(shared.clone()), reword)
            .await
            .unwrap();

        let success = complete_item(&shared, "1", None).await;
        assert_eq!(
            item_named(&success, "Ship the parser").status,
            TodoStatus::Completed
        );
        assert_eq!(
            harness.requests.lock().unwrap().len(),
            2,
            "the reworded condition must be verified again"
        );
    }
}
