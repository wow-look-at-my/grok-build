//! Goal planner subagent runner. Mirrors [`crate::session::goal_classifier`]
//! but is fail-CLOSED: any failure pauses the goal. Writes a structured plan
//! to the [`GoalTracker::plan_path`](crate::session::goal_tracker::GoalTracker::plan_path)
//! file; the spawn is hidden behind [`GoalPlannerSpawner`] so tests can inject
//! a deterministic spawner.

#![allow(dead_code)]

use crate::session::events::{Event, GoalPlannerFailClosedReason, GoalRoleModelFailOpenReason};
use crate::session::goal_role_tools::RoleToolNames;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xai_file_utils::events::EventWriter;
use xai_grok_tools::implementations::grok_build::task::backend::{ChannelBackend, SubagentBackend};
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
};

// Shared per-role model override + spawn-and-retry-once fail-open wrapper

/// Every `/goal` role (planner, strategist, each verifier skeptic) spawns its
/// subagent as `general-purpose`. The role's configured `agent_type` selects
/// only the HARNESS (system prompt + toolset flavor),
/// threaded as [`SubagentRuntimeOverrides::harness_agent_type`]; the
/// subagent_type stays fixed so the role keeps a capable toolset on whichever
/// harness is chosen. Single source of truth shared by the three role spawners
/// and the parent-side `describe_subagent_type` probe so the gated/probed
/// toolset matches the spawned one.
///
/// [`SubagentRuntimeOverrides::harness_agent_type`]: xai_grok_tools::implementations::grok_build::task::types::SubagentRuntimeOverrides::harness_agent_type
pub(crate) const GOAL_ROLE_SUBAGENT_TYPE: &str = "general-purpose";
pub(crate) const GOAL_ROLE_AWAIT_BUDGET_EXCEEDED: &str =
    "goal role subagent exceeded foreground wait budget";

/// Default foreground wait for the goal plan writer. This is separate from
/// the ordinary TaskTool wait budget because planning is a user-visible
/// phase and a slow planner must not be mistaken for a failed goal.
pub(crate) const GOAL_PLANNER_AWAIT_BUDGET_DEFAULT: std::time::Duration =
    std::time::Duration::from_secs(30 * 60);

/// Resolve the planner wait from a positive millisecond environment override.
/// Invalid, zero, or absent values use the 30 minute default.
pub(crate) fn goal_planner_await_budget() -> std::time::Duration {
    xai_grok_tools::implementations::grok_build::task::backend::env_duration_or(
        "GROK_GOAL_PLANNER_AWAIT_BUDGET_MS",
        GOAL_PLANNER_AWAIT_BUDGET_DEFAULT,
    )
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn planner_wait_defaults_to_30_minutes() {
        assert_eq!(
            GOAL_PLANNER_AWAIT_BUDGET_DEFAULT,
            std::time::Duration::from_secs(1800)
        );
    }
}

/// Best-effort wait for a subagent cancel to be acknowledged before giving up.
/// The planner is aborting regardless, so this is a bound on cleanup, not a
/// correctness gate.
const GOAL_PLANNER_CANCEL_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Resolved per-role spawn override.
///
/// `None`/`None` ⇒ inherit the current model + the session harness (the
/// historic `SubagentRuntimeOverrides::default()` behavior). When either field
/// is `Some` the pair is "explicit" and the spawn-and-retry-once wrapper
/// ([`spawn_with_fail_open_retry`]) retries on the current model + session
/// harness if the first attempt fails. Shared by the planner, strategist, and
/// per-skeptic classifier spawners.
#[derive(Debug, Clone, Default)]
pub(crate) struct RoleSpawnOverride {
    /// Resolved, post-auth, post-fail-open model id, or `None` to inherit.
    pub model: Option<String>,
    /// Resolved harness `agent_type` (e.g. `"grok-build-plan"`)
    /// whose `AgentDefinition` decides the spawned subagent's harness flavor
    /// (system prompt + toolset), applied REGARDLESS of the
    /// parent agent. `None` ⇒ inherit the session harness. NOT a subagent type —
    /// the subagent_type stays fixed at [`GOAL_ROLE_SUBAGENT_TYPE`].
    pub agent_type: Option<String>,
}

impl RoleSpawnOverride {
    /// `true` when a configured `{model, agent_type}` pair was committed
    /// (so a spawn failure triggers the one current-model retry).
    pub(crate) fn is_explicit(&self) -> bool {
        self.model.is_some() || self.agent_type.is_some()
    }
}

/// The model id a `/goal` role actually runs on: the committed override's
/// model when present, else the inherited parent. Reported as
/// `GoalPlannerFired.model_id` / `GoalStrategistFired.model_id` so a committed
/// remote override isn't under-reported as the parent model. Not the
/// classifier — its skeptic pool has no single representative model.
pub(crate) fn effective_role_model_id<'a>(
    override_model: Option<&'a str>,
    parent: &'a str,
) -> &'a str {
    override_model.unwrap_or(parent)
}

/// A role prompt rendered for both the toolset the FIRST attempt runs on and
/// the toolset the fail-open RETRY falls back to.
///
/// The explicit-pair retry re-runs on the default/parent toolset, so its
/// tool-name placeholders (`{READ_TOOL}`, `{WRITE_TOOL}`, …) must name THAT
/// toolset — not the configured pair's. The caller renders both up front and
/// [`spawn_with_fail_open_retry`] picks the matching one per attempt. On the
/// inherit path no retry occurs and `fallback` is never read, so callers on
/// that path may leave it empty.
pub(crate) struct RoleRenderedPrompt {
    /// Rendered for the role's RESOLVED toolset (the first/only attempt).
    pub primary: String,
    /// Rendered for the DEFAULT/parent toolset the explicit-pair fail-open
    /// retry falls back to.
    pub fallback: String,
}

/// What one planner spawn returned: the child's terminal text, plus the todo
/// items the child put on ITS OWN list while it planned.
///
/// The planner is told to build that list with the session's todo tool as it
/// writes the plan. A child session keeps its own `State<TodoState>`, read into
/// [`SubagentResult::todos`] when the child finishes, so carrying the items here
/// is what lets the parent merge the planner's own steps into the session's
/// list instead of re-deriving them from the plan file.
///
/// [`SubagentResult::todos`]: xai_grok_tools::implementations::grok_build::task::types::SubagentResult::todos
#[derive(Debug, Clone, Default)]
pub(crate) struct PlannerSpawnOutput {
    /// The child's terminal response text (the harness parses `Done` from it).
    pub(crate) output: String,
    /// Contents of the child's own todo list when it finished, in order.
    pub(crate) todos: Vec<String>,
}

/// A spawn error the fail-open wrapper can inspect to decide whether a
/// retry is appropriate.
///
/// Implemented by both `goal_planner::SpawnError` and
/// `goal_classifier::SpawnError` (structurally identical) so the generic
/// wrapper can exclude cancellations from the retry without coupling to one
/// concrete error type.
pub(crate) trait RetryableSpawnError {
    /// `true` when this error is a cancellation (user / turn abort). A
    /// cancelled explicit-pair spawn must propagate WITHOUT a fail-open retry
    /// so today's semantics are preserved (e.g. the planner still pauses as
    /// `Aborted` rather than silently re-running on the current model).
    fn is_cancelled(&self) -> bool;
}

/// Notice reason: a resumed goal keeps the skeptic models it froze at its
/// first verification, so a model configured since then is not used.
pub(crate) const GOAL_ROLE_NOTICE_ASSIGNMENT_FROZEN: &str = "assignment_frozen";

/// Notice reason: the planner forks the session conversation to reuse its
/// prompt cache, so it always runs on the session model.
pub(crate) const GOAL_ROLE_NOTICE_PLANNER_FORKS_SESSION: &str = "planner_forks_session";

/// Reports that a `/goal` role left the model the user configured for it.
///
/// The event feeds telemetry. The notice reaches the pager, which shows it as
/// a warning. Without the notice the user sees only that the role ran on the
/// session model, and never why.
#[derive(Clone, Default)]
pub(crate) struct RoleFallbackReporter {
    pub(crate) events: Option<EventWriter>,
    pub(crate) notify: Option<crate::session::goal_orchestrator::GoalNotifySender>,
    /// The session model, which is what a fallback runs on.
    pub(crate) fallback_model: Option<String>,
}

impl RoleFallbackReporter {
    pub(crate) fn report(
        &self,
        role: &'static str,
        skeptic_idx: Option<u32>,
        requested_model: &str,
        reason: GoalRoleModelFailOpenReason,
        detail: Option<String>,
    ) {
        if let Some(ev) = &self.events {
            ev.emit(Event::GoalRoleModelFailOpen {
                role,
                skeptic_idx,
                reason: reason.as_const_str(),
            });
        }
        self.notify(
            role,
            skeptic_idx,
            requested_model,
            reason.as_const_str(),
            detail,
        );
    }

    /// Send the pager notice alone. For a fallback that has no telemetry
    /// reason, e.g. the planner that always forks the session model.
    pub(crate) fn notify(
        &self,
        role: &'static str,
        skeptic_idx: Option<u32>,
        requested_model: &str,
        reason: &str,
        detail: Option<String>,
    ) {
        tracing::warn!(
            role,
            ?skeptic_idx,
            requested_model,
            reason,
            detail = detail.as_deref().unwrap_or(""),
            "goal role did not run on its configured model"
        );
        if let Some(notify) = &self.notify {
            notify.send_update(
                crate::extensions::notification::SessionUpdate::GoalRoleModelFallback {
                    role: role.to_string(),
                    skeptic_idx,
                    requested_model: requested_model.to_string(),
                    fallback_model: self.fallback_model.clone(),
                    reason: reason.to_string(),
                    detail,
                },
            );
        }
    }
}

/// What a role asked for, for the fallback notice. A harness-only override
/// names no model, so the harness stands in for it.
fn requested_label(override_: &RoleSpawnOverride) -> String {
    match (&override_.model, &override_.agent_type) {
        (Some(model), _) => model.clone(),
        (None, Some(harness)) => format!("harness {harness}"),
        (None, None) => String::new(),
    }
}

impl RetryableSpawnError for SpawnError {
    fn is_cancelled(&self) -> bool {
        matches!(
            self,
            SpawnError::Runtime {
                cancelled: true,
                ..
            } | SpawnError::Interrupted
        )
    }
}

/// Spawn-and-retry-once fail-open wrapper (Key Decision #13, load-bearing).
///
/// Inherit override ⇒ exactly one attempt on the current model + session
/// harness (the `prompt` is moved, never cloned). Explicit override ⇒ one
/// attempt with the configured `{model, harness}` pair; if it returns a
/// NON-cancellation `Err`, emits `GoalRoleModelFailOpen { reason: spawn_failed }`
/// and retries ONCE with `model = None` + harness `None` (the current-model +
/// session-harness fallback); only a SECOND failure propagates. A cancellation
/// propagates as-is (no retry), so a bad configured pair can never change
/// today's failure semantics (e.g. it can never regress the fail-CLOSED planner
/// into a goal-pause).
///
/// The `spawn` closure receives `(model, harness_agent_type, prompt)` and owns
/// the fixed subagent_type ([`GOAL_ROLE_SUBAGENT_TYPE`]); the second arg is the
/// harness override (`None` ⇒ session harness), NOT a subagent type.
///
/// The first attempt uses `prompt.primary` (rendered for the configured
/// harness's toolset); the retry uses `prompt.fallback` (rendered for the
/// session-harness toolset it actually runs on), so the retried prompt never
/// names the wrong toolset's tools. Each render is moved into its attempt — no
/// clone on any path.
pub(crate) async fn spawn_with_fail_open_retry<T, E, F, Fut>(
    role: &'static str,
    skeptic_idx: Option<u32>,
    override_: &RoleSpawnOverride,
    fallback: &RoleFallbackReporter,
    prompt: RoleRenderedPrompt,
    mut spawn: F,
) -> Result<T, E>
where
    E: RetryableSpawnError + std::fmt::Display,
    F: FnMut(Option<String>, Option<String>, String) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    // Inherit path: single attempt on the current model + session harness; move
    // the prompt.
    if !override_.is_explicit() {
        return spawn(None, None, prompt.primary).await;
    }
    let first = spawn(
        override_.model.clone(),
        override_.agent_type.clone(),
        prompt.primary,
    )
    .await;
    let error = match first {
        Err(e) if !e.is_cancelled() => e,
        other => return other,
    };
    fallback.report(
        role,
        skeptic_idx,
        &requested_label(override_),
        GoalRoleModelFailOpenReason::SpawnFailed,
        Some(error.to_string()),
    );
    // Retry on the session harness — use the matching `fallback` render so the
    // prompt names the toolset the retry actually runs on.
    spawn(None, None, prompt.fallback).await
}

// Constants

/// Telemetry value on `GoalPlannerFired`. Does not cap user-initiated
/// `/goal resume` retries, which re-run the planner unbounded.
pub(crate) const GOAL_PLANNER_MAX_RUNS: u32 = 1;

/// Same general-purpose inventory each verifier skeptic uses: the
/// planner reads and greps the workspace and, when web search is
/// enabled, researches external facts to clarify scope. The configured
/// `agent_type` selects the HARNESS, not this subagent type.
const GOAL_PLANNER_SUBAGENT_TYPE: &str = GOAL_ROLE_SUBAGENT_TYPE;

const GOAL_PLANNER_SUBAGENT_DESCRIPTION: &str = "goal plan writer";

const GOAL_PLANNER_PROMPT_TEMPLATE: &str = include_str!("templates/goal_planner_prompt.md");

// Outcome + spawner abstraction

/// Result of one planner attempt. `Planned` carries the path the
/// planner wrote (always the input `plan_file`). `FailClosed` carries
/// the reason — every variant pauses the goal at the call site.
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "`latency_ms` is consumed by future telemetry")]
pub(crate) enum GoalPlannerOutcome {
    Planned {
        plan_file: PathBuf,
        /// Contents of the planner child's own todo list, in order — the steps
        /// it named with `todo_write` while planning. Empty when the child kept
        /// no list, in which case the caller falls back to the plan body.
        todos: Vec<String>,
        latency_ms: u64,
    },
    /// Produced only by the planner's [`ChannelSpawner`], whose `cancel_token`
    /// is wired to `start_planner_run`; when the token fires mid-spawn (a user
    /// Stop, or the turn being cancelled) the attempt returns `Interrupted`.
    /// A Send Now does NOT cancel the planner — it delivers context to the live
    /// planner instead (see [`planner_context_message`]). The strategist and
    /// summarizer spawners pass a fresh, never-cancelled token, so their
    /// equivalent cancel arms are unreachable in practice.
    Interrupted,
    FailClosed {
        reason: GoalPlannerFailClosedReason,
        latency_ms: u64,
    },
}

/// Subagent spawn abstraction. Production uses [`ChannelSpawner`];
/// tests use `MockSpawner` (defined in the tests module).
#[async_trait::async_trait]
pub(crate) trait GoalPlannerSpawner: Send + Sync {
    /// Spawn under `id` and return the terminal response when the
    /// subagent finishes. `prompt` carries both the configured-pair render
    /// (`primary`) and the default-toolset fail-open retry render (`fallback`).
    async fn spawn_planner(
        &self,
        id: &str,
        prompt: RoleRenderedPrompt,
    ) -> Result<PlannerSpawnOutput, SpawnError>;
}

/// Spawn-time error. Deliberately mirrored from the verifier rather
/// than shared — unifying would be a wider refactor.
#[derive(Debug)]
pub(crate) enum SpawnError {
    Transport(String),
    Runtime { message: String, cancelled: bool },
    Interrupted,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(d) => write!(f, "subagent transport error: {d}"),
            Self::Runtime { message, cancelled } => {
                write!(
                    f,
                    "subagent runtime error (cancelled={cancelled}): {message}"
                )
            }
            Self::Interrupted => f.write_str("planner interrupted before completion"),
        }
    }
}

// Terminal-token parse

/// Accepts only the literal `Done` (after trim). A malformed token isn't
/// fatal on its own — the runner gates on the plan file's presence.
pub(crate) fn parse_terminal_response(text: &str) -> bool {
    text.trim() == "Done"
}

// Send Now context

/// Prefix on the mid-turn message a Send Now delivers to the live planner.
/// The planner is mid-turn on its own prompt, so the text has to say what it
/// is and what to do with it.
const PLANNER_CONTEXT_PREFIX: &str = "Additional user context for the current plan:";

/// Wrap a Send Now's text as the message the coordinator hands to the running
/// planner child. Nothing is restarted: the planner folds the context into the
/// plan it is already writing.
pub(crate) fn planner_context_message(text: &str) -> String {
    format!("{PLANNER_CONTEXT_PREFIX}\n\n{text}")
}

// Production spawner

pub(crate) struct ChannelSpawner {
    pub(crate) event_tx: tokio::sync::mpsc::UnboundedSender<
        xai_grok_tools::implementations::grok_build::task::types::SubagentEvent,
    >,
    pub(crate) foreground_wait:
        Option<xai_grok_tools::implementations::grok_build::task::types::SubagentForegroundWait>,
    pub(crate) parent_session_id: String,
    pub(crate) parent_prompt_id: Option<String>,
    pub(crate) cwd: Option<String>,
    /// Trace-artifact sink + resolved `task` tool name; `None` disables
    /// recording. See [`super::goal_classifier::record_subagent_trace`].
    pub(crate) trace_sink: Option<(xai_chat_state::ChatStateHandle, String)>,
    /// Resolved per-role model+toolset override. Default (inherit) keeps the
    /// historic `::default()` spawn behavior.
    pub(crate) role_override: RoleSpawnOverride,
    pub(crate) cancel_token: tokio_util::sync::CancellationToken,
    /// Cell published with the coordinator id this spawn runs under, so a Send
    /// Now can address the live planner child (the goal tracker owns the cell;
    /// see `GoalTracker::planner_subagent_id`). `None` when nothing needs it.
    pub(crate) subagent_id_slot: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    /// Where a spawn-and-retry-once fail-open is reported. `Default` in tests.
    pub(crate) fallback: RoleFallbackReporter,
}

#[async_trait::async_trait]
impl GoalPlannerSpawner for ChannelSpawner {
    async fn spawn_planner(
        &self,
        id: &str,
        prompt: RoleRenderedPrompt,
    ) -> Result<PlannerSpawnOutput, SpawnError> {
        // Publish the coordinator id BEFORE awaiting the child: a Send Now that
        // lands at any point during the run has to be able to address it.
        if let Some(slot) = &self.subagent_id_slot
            && let Ok(mut published) = slot.lock()
        {
            *published = Some(id.to_string());
        }
        // Clone the primary render for the trace pair only when tracing; the
        // wrapper moves each render into its attempt (no other clone).
        let trace_prompt = self.trace_sink.as_ref().map(|_| prompt.primary.clone());
        let outcome = spawn_with_fail_open_retry(
            "planner",
            None,
            &self.role_override,
            &self.fallback,
            prompt,
            |model, harness, prompt| self.send_one(id, prompt, model, harness),
        )
        .await;

        // Trace the FINAL attempt's output / runtime error (transport errors
        // carry no subagent output, matching the prior behavior).
        match &outcome {
            Ok(out) => crate::session::goal_classifier::record_subagent_trace(
                self.trace_sink.as_ref(),
                id,
                GOAL_PLANNER_SUBAGENT_TYPE,
                GOAL_PLANNER_SUBAGENT_DESCRIPTION,
                trace_prompt.as_deref(),
                &out.output,
            ),
            Err(SpawnError::Runtime { message, .. }) => {
                crate::session::goal_classifier::record_subagent_trace(
                    self.trace_sink.as_ref(),
                    id,
                    GOAL_PLANNER_SUBAGENT_TYPE,
                    GOAL_PLANNER_SUBAGENT_DESCRIPTION,
                    trace_prompt.as_deref(),
                    message,
                )
            }
            Err(SpawnError::Transport(_) | SpawnError::Interrupted) => {}
        }
        outcome
    }
}

impl ChannelSpawner {
    /// Send one spawn (model + harness override resolved by the caller) and
    /// await its terminal result. The fail-open wrapper calls this once or
    /// twice (retry on the current model + session harness). The subagent_type
    /// is always [`GOAL_PLANNER_SUBAGENT_TYPE`]; `harness_agent_type` selects
    /// the harness flavor (`None` ⇒ session harness).
    async fn send_one(
        &self,
        id: &str,
        prompt: String,
        model: Option<String>,
        harness_agent_type: Option<String>,
    ) -> Result<PlannerSpawnOutput, SpawnError> {
        let request = SubagentRequest {
            id: id.to_string(),
            prompt,
            description: GOAL_PLANNER_SUBAGENT_DESCRIPTION.to_string(),
            subagent_type: GOAL_PLANNER_SUBAGENT_TYPE.to_string(),
            parent_session_id: self.parent_session_id.clone(),
            parent_prompt_id: self.parent_prompt_id.clone(),
            resume_from: None,
            cwd: self.cwd.clone(),
            runtime_overrides: SubagentRuntimeOverrides {
                model,
                harness_agent_type,
                // Goal planning is allowed to use the full configurable
                // foreground wait budget, whose default is 30 minutes.
                foreground_wait_budget_ms: Some(
                    crate::session::goal_planner::goal_planner_await_budget().as_millis() as u64,
                ),
                ..Default::default()
            },
            run_in_background: false,
            // Harness-internal: never surface to the model's idle reminder.
            surface_completion: false,
            await_to_completion: false,
            fork_context: true,
            owner: SubagentOwner::Task,
            cancel_token: self.cancel_token.clone(),
        };
        let backend = ChannelBackend::new(self.event_tx.clone());
        let cancel = self.cancel_token.clone();
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = tokio::time::timeout(
                    GOAL_PLANNER_CANCEL_ACK_TIMEOUT,
                    backend.cancel(id),
                )
                .await;
                return Err(SpawnError::Interrupted);
            }
            result = backend.spawn_with_foreground_wait(request, self.foreground_wait.as_ref()) => {
                result.map_err(|error| SpawnError::Transport(error.to_string()))?
            }
        };
        if result.backgrounded {
            let _ = tokio::time::timeout(
                GOAL_PLANNER_CANCEL_ACK_TIMEOUT,
                backend.cancel(&result.subagent_id),
            )
            .await;
            return Err(SpawnError::Runtime {
                message: GOAL_ROLE_AWAIT_BUDGET_EXCEEDED.to_owned(),
                cancelled: true,
            });
        }
        if !result.success {
            let message = result.error.unwrap_or_else(|| "unknown error".to_string());
            return Err(SpawnError::Runtime {
                message,
                cancelled: result.cancelled,
            });
        }
        Ok(PlannerSpawnOutput {
            output: result.output.to_string(),
            // The child's own list, read out of its live `TodoState` when it
            // finished — a `todo_write` the planner issued while planning.
            todos: result.todos,
        })
    }
}

// Runner

pub(crate) struct GoalPlannerInputs<'a> {
    pub objective: &'a str,
    pub context: &'a str,
    pub plan_file: &'a Path,
    pub attempt: u32,
    pub model_id: &'a str,
    /// Resolved tool names for the planner role's prompt placeholders
    /// (`{READ_TOOL}`/`{SEARCH_TOOL}`/`{LIST_TOOL}`/`{WRITE_TOOL}`). Built
    /// parent-side from the planner's resolved toolset.
    pub tool_names: &'a RoleToolNames,
    /// Default/parent-toolset tool names used to render the fail-open RETRY
    /// prompt, so a retry that falls back to the default toolset names THAT
    /// toolset's tools. On the inherit path this equals `tool_names`.
    pub inherit_tool_names: &'a RoleToolNames,
}

/// Run one planner attempt. Fail-CLOSED: any failure path returns
/// `FailClosed { reason }` and the caller pauses the goal.
pub(crate) async fn run_goal_planner(
    spawner: Arc<dyn GoalPlannerSpawner>,
    inputs: GoalPlannerInputs<'_>,
    emit_event: &dyn Fn(Event),
) -> GoalPlannerOutcome {
    let started = std::time::Instant::now();
    emit_event(Event::GoalPlannerFired {
        attempt: inputs.attempt,
        max_runs: GOAL_PLANNER_MAX_RUNS,
        model_id: inputs.model_id.to_string(),
    });

    if let Some(parent) = inputs.plan_file.parent()
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        tracing::warn!(
            parent = %parent.display(),
            error = %err,
            "goal planner: failed to pre-create plan-file parent dir; failing closed",
        );
        return record_fail_closed(
            GoalPlannerFailClosedReason::FileWriteFailed,
            inputs.attempt,
            started,
            emit_event,
        );
    }

    let plan_file_str = inputs.plan_file.to_string_lossy();
    let with_plan_file = GOAL_PLANNER_PROMPT_TEMPLATE.replace("{PLAN_FILE}", &plan_file_str);
    // Render once per toolset: `primary` for the resolved toolset, `fallback`
    // for the default/parent toolset the explicit-pair retry falls back to.
    let render = |tool_names: &RoleToolNames| -> String {
        let rendered = tool_names.apply(&with_plan_file);
        let mut full = String::with_capacity(rendered.len() + inputs.objective.len() + 256);
        full.push_str(&rendered);
        full.push_str("\n\nOBJECTIVE:\n");
        full.push_str(inputs.objective);
        full.push_str("\n\nCONTEXT:\n");
        full.push_str(inputs.context);
        full.push('\n');
        full
    };
    let prompt = RoleRenderedPrompt {
        primary: render(inputs.tool_names),
        fallback: render(inputs.inherit_tool_names),
    };

    let spawn_id = uuid::Uuid::now_v7().to_string();
    let (response, todos) = match spawner.spawn_planner(&spawn_id, prompt).await {
        Ok(PlannerSpawnOutput { output, todos }) => (output, todos),
        Err(SpawnError::Transport(detail)) => {
            tracing::warn!(error = %detail, "goal planner: transport error; failing closed");
            return record_fail_closed(
                GoalPlannerFailClosedReason::Transport,
                inputs.attempt,
                started,
                emit_event,
            );
        }
        Err(SpawnError::Interrupted) => return GoalPlannerOutcome::Interrupted,
        Err(SpawnError::Runtime { message, cancelled }) => {
            let reason = if cancelled {
                GoalPlannerFailClosedReason::Aborted
            } else {
                GoalPlannerFailClosedReason::Runtime
            };
            tracing::warn!(
                error = %message,
                cancelled,
                "goal planner: subagent runtime error; failing closed",
            );
            return record_fail_closed(reason, inputs.attempt, started, emit_event);
        }
    };

    let plan_present = match tokio::fs::metadata(inputs.plan_file).await {
        Ok(meta) => meta.is_file() && meta.len() > 0,
        Err(_) => false,
    };

    if !plan_present {
        tracing::info!(
            plan_file = %plan_file_str,
            terminal_token_ok = parse_terminal_response(&response),
            response_snippet = %response.chars().take(120).collect::<String>(),
            "goal planner: plan file missing or empty; failing closed",
        );
        return record_fail_closed(
            GoalPlannerFailClosedReason::MissingPlan,
            inputs.attempt,
            started,
            emit_event,
        );
    }

    let latency_ms = started.elapsed().as_millis() as u64;
    emit_event(Event::GoalPlannerCompleted {
        attempt: inputs.attempt,
        latency_ms,
    });
    GoalPlannerOutcome::Planned {
        plan_file: inputs.plan_file.to_path_buf(),
        todos,
        latency_ms,
    }
}

fn record_fail_closed(
    reason: GoalPlannerFailClosedReason,
    attempt: u32,
    started: std::time::Instant,
    emit_event: &dyn Fn(Event),
) -> GoalPlannerOutcome {
    let latency_ms = started.elapsed().as_millis() as u64;
    emit_event(Event::GoalPlannerFailClosed {
        reason: reason.as_const_str(),
        attempt,
        latency_ms,
    });
    GoalPlannerOutcome::FailClosed { reason, latency_ms }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::goal_role_tools::tests::{assert_no_tool_placeholders, summary_with};
    use std::sync::{Arc, Mutex};
    use xai_grok_tools::types::tool::ToolKind;

    #[test]
    fn planner_template_default_render_preserves_wording_and_has_no_placeholders() {
        // Default/inherit render: placeholders resolve to the literal parent
        // (grok-build) tool names; guards against accidental wording drift.
        let rendered = RoleToolNames::inherit_defaults().apply(GOAL_PLANNER_PROMPT_TEMPLATE);
        assert!(
            rendered.contains("with your\n`read_file`/`grep`/`list_dir` tools to clarify scope"),
            "planner read/grep/list placeholders must render to the defaults",
        );
        assert!(
            rendered.contains("Use your `write` tool to write Markdown"),
            "planner file-write placeholder must render to `write`",
        );
        assert_no_tool_placeholders(&rendered);
    }

    #[test]
    fn planner_template_explicit_render_has_no_leftover_placeholders() {
        // The explicit `from_summary` path must leave no
        // tool placeholder unresolved on the planner template.
        for summary in [
            summary_with(&[
                (ToolKind::Read, "cursor_read"),
                (ToolKind::ListDir, "cursor_ls"),
                (ToolKind::Search, "cursor_grep"),
                (ToolKind::Write, "cursor_write"),
            ]),
            summary_with(&[
                (ToolKind::Read, "read_file"),
                (ToolKind::ListDir, "list_dir"),
                (ToolKind::Search, "grep"),
                (ToolKind::Write, "write"),
            ]),
        ] {
            let rendered =
                RoleToolNames::from_summary(&summary).apply(GOAL_PLANNER_PROMPT_TEMPLATE);
            assert_no_tool_placeholders(&rendered);
        }
    }

    #[test]
    fn planner_template_default_render_names_the_web_tools() {
        // The research mandate is inert if the planner can't see the tool: the
        // inherit/default render must NAME a real web tool (the stock
        // `web_search`/`web_fetch`) and leave no tool placeholder unresolved.
        let rendered = RoleToolNames::inherit_defaults().apply(GOAL_PLANNER_PROMPT_TEMPLATE);
        assert!(
            rendered.contains("web_search"),
            "default render must name the web-search tool",
        );
        assert!(rendered.contains("web_fetch"));
        assert_no_tool_placeholders(&rendered);
    }

    #[test]
    fn planner_template_cursor_render_names_the_web_search_tool() {
        // The previously-broken case: on the alternate toolset the web
        // tool is named "WebSearch", so the planner prompt must render THAT —
        // otherwise the weak model never reaches for it and plans from memory.
        let rendered = RoleToolNames::from_summary(&summary_with(&[
            (ToolKind::WebSearch, "WebSearch"),
            (ToolKind::WebFetch, "WebFetch"),
        ]))
        .apply(GOAL_PLANNER_PROMPT_TEMPLATE);
        assert!(
            rendered.contains("WebSearch"),
            "cursor render must name the cursor web-search tool",
        );
        assert!(rendered.contains("WebFetch"));
        assert_no_tool_placeholders(&rendered);
    }

    /// The planner is told to build its OWN todo list with the session's todo
    /// tool, and the prompt names the tool the harness actually exposes. That
    /// list is what the parent merges into the session's list, so a prompt that
    /// drops the instruction silently loses every step.
    #[test]
    fn planner_prompt_requires_the_todo_list_and_names_the_real_tool() {
        let template = GOAL_PLANNER_PROMPT_TEMPLATE;
        assert!(
            template.contains("## Todo list — REQUIRED"),
            "the todo-list section must stay in the planner prompt",
        );
        assert!(
            template.contains("one item per `## Task checklist` line"),
            "the planner must be told to list one todo per checklist line",
        );
        assert!(template.contains("{TODO_TOOL}"));

        let rendered = RoleToolNames::from_summary(&summary_with(&[
            (ToolKind::Read, "read_file"),
            (ToolKind::Plan, "cursor_todo"),
        ]))
        .apply(template);
        assert!(
            rendered.contains("`cursor_todo`"),
            "the render must name the harness's own todo tool:\n{rendered}",
        );
        assert!(!rendered.contains("{TODO_TOOL}"));
        assert_no_tool_placeholders(&rendered);
    }

    /// Pin each load-bearing clause of the named-artifact research mandate so a
    /// targeted revert fails (the convergence balance: fix under-scoping, never
    /// reopen over-scoping).
    /// The planner's harness must actually expose the todo tool: the prompt's
    /// instruction to list the plan's work is inert, and the session's list
    /// stays empty, if the toolset the planner runs on has no such tool.
    #[test]
    fn planner_harness_toolset_exposes_the_todo_tool() {
        let definition = xai_grok_agent::config::AgentDefinition::general_purpose();
        let ids: Vec<&str> = definition
            .tool_config
            .tools
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert!(
            ids.iter().any(|id| id.ends_with("todo_write")),
            "the goal planner's `{GOAL_PLANNER_SUBAGENT_TYPE}` harness must expose a todo \
             tool, or it cannot make the call the session's list is populated from: {ids:?}",
        );
    }

    #[test]
    fn planner_prompt_pins_named_artifact_research_mandate() {
        let t = GOAL_PLANNER_PROMPT_TEMPLATE;
        // Research a named artifact's defining mechanics, not from memory.
        assert!(t.contains("DEFINING mechanics"));
        assert!(t.contains("do NOT plan it from memory alone"));
        // Domain-agnostic (non-game example) + primary, not error-path behaviors.
        assert!(t.contains("round-trip of valid input"));
        assert!(t.contains("error/edge/invalid-input handling"));
        // Convergence guard: group (don't drop) to fit the cap; a core mechanic
        // that can't fit is an EXPLICIT deferral, never a silent omission.
        assert!(t.contains("Do not map one criterion per mechanic"));
        assert!(t.contains("Grouping, NOT dropping"));
        assert!(t.contains("record it under `## Non-goals` (or `## Assumed scope`)"));
        // Gating-test sentence (distinctive substring, not the bare word).
        assert!(t.contains("is it still recognizably"));
        // OBJECTIVE wins; non-core routed to Non-goals.
        assert!(t.contains("OBJECTIVE's explicit words always win"));
        assert!(t.contains("list it under `## Non-goals`"));
        // Gated away from generic archetypes; the web step is optional.
        assert!(t.contains("is not a named artifact"));
        assert!(t.contains("note the gap under `## Assumed scope`"));
    }

    #[tokio::test]
    async fn channel_spawner_request_is_harness_internal() {
        use xai_grok_tools::implementations::grok_build::task::types::{
            SubagentEvent, SubagentResult,
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let wait_depth = Arc::new(crate::tools::tool_context::BlockingWaitState::new());
        let spawner = ChannelSpawner {
            event_tx: tx,
            foreground_wait: Some(crate::tools::tool_context::subagent_foreground_wait(
                Arc::clone(&wait_depth),
            )),
            parent_session_id: "parent".into(),
            parent_prompt_id: None,
            cwd: None,
            trace_sink: None,
            role_override: RoleSpawnOverride::default(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            subagent_id_slot: None,
            fallback: RoleFallbackReporter::default(),
        };
        let handle = tokio::spawn(async move {
            let _ = spawner
                .spawn_planner("plan-id", role_prompt("prompt"))
                .await;
        });

        let SubagentEvent::Spawn(request) = rx.recv().await.expect("spawn event") else {
            panic!("expected Spawn");
        };
        assert_eq!(wait_depth.depth(), 1);
        assert!(
            !request.surface_completion,
            "planner subagent must not surface to the idle reminder"
        );
        let _ = request.result_tx.send(SubagentResult::default());
        handle.await.unwrap();
        assert_eq!(wait_depth.depth(), 0);
    }

    #[test]
    fn parse_terminal_response_accepts_done_exact() {
        assert!(parse_terminal_response("Done"));
        assert!(parse_terminal_response("  Done \n"));
        assert!(!parse_terminal_response("done"));
        assert!(!parse_terminal_response("DONE"));
        assert!(!parse_terminal_response("Done."));
        assert!(!parse_terminal_response("Done!"));
        assert!(!parse_terminal_response(""));
    }

    /// Deterministic spawner with knobs for response text, whether to
    /// write the plan file, and the file body.
    struct MockSpawner {
        response: Result<String, SpawnError>,
        write_plan: bool,
        plan_body: Vec<u8>,
        plan_file_target: std::path::PathBuf,
        last_prompt: Mutex<Option<String>>,
        /// The child's own todo list, standing in for what a real planner child
        /// built with `todo_write` while planning.
        todos: Vec<String>,
    }

    impl MockSpawner {
        fn ok_writes(plan_file: &Path, body: &[u8]) -> Self {
            Self {
                response: Ok("Done".to_string()),
                write_plan: true,
                plan_body: body.to_vec(),
                plan_file_target: plan_file.to_path_buf(),
                last_prompt: Mutex::new(None),
                todos: Vec::new(),
            }
        }

        fn ok_does_not_write(plan_file: &Path) -> Self {
            Self {
                response: Ok("Done".to_string()),
                write_plan: false,
                plan_body: Vec::new(),
                plan_file_target: plan_file.to_path_buf(),
                last_prompt: Mutex::new(None),
                todos: Vec::new(),
            }
        }

        fn fails(err: SpawnError, plan_file: &Path) -> Self {
            Self {
                response: Err(err),
                write_plan: false,
                plan_body: Vec::new(),
                plan_file_target: plan_file.to_path_buf(),
                last_prompt: Mutex::new(None),
                todos: Vec::new(),
            }
        }

        /// Stand in for a planner child that listed its steps with `todo_write`.
        fn with_todos(mut self, todos: &[&str]) -> Self {
            self.todos = todos.iter().map(|t| (*t).to_string()).collect();
            self
        }
    }

    #[async_trait::async_trait]
    impl GoalPlannerSpawner for MockSpawner {
        async fn spawn_planner(
            &self,
            #[allow(unused_variables)] id: &str,
            prompt: RoleRenderedPrompt,
        ) -> Result<PlannerSpawnOutput, SpawnError> {
            *self.last_prompt.lock().unwrap() = Some(prompt.primary);
            if self.write_plan {
                let _ = tokio::fs::write(&self.plan_file_target, &self.plan_body).await;
            }
            match &self.response {
                Ok(text) => Ok(PlannerSpawnOutput {
                    output: text.clone(),
                    todos: self.todos.clone(),
                }),
                Err(SpawnError::Transport(s)) => Err(SpawnError::Transport(s.clone())),
                Err(SpawnError::Runtime { message, cancelled }) => Err(SpawnError::Runtime {
                    message: message.clone(),
                    cancelled: *cancelled,
                }),
                Err(SpawnError::Interrupted) => Err(SpawnError::Interrupted),
            }
        }
    }

    fn collect_events() -> (Arc<Mutex<Vec<String>>>, impl Fn(Event) + Send + Sync) {
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = log.clone();
        let emit = move |e: Event| {
            let tag = match e {
                Event::GoalPlannerFired { .. } => "fired".to_string(),
                Event::GoalPlannerCompleted { .. } => "completed".to_string(),
                Event::GoalPlannerFailClosed { reason, .. } => format!("fail_closed:{reason}"),
                other => format!("other:{other:?}"),
            };
            log_clone.lock().unwrap().push(tag);
        };
        (log, emit)
    }

    fn tmp_plan_file(name: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "goal-planner-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        tmp.join("plan.md")
    }

    #[tokio::test]
    async fn success_path_emits_fired_and_completed_and_returns_planned() {
        let plan_file = tmp_plan_file("happy");
        let spawner = Arc::new(MockSpawner::ok_writes(
            &plan_file,
            b"# Plan: foo\n\n## Goal kind\n\ncode-change\n",
        ));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(outcome, GoalPlannerOutcome::Planned { .. }));
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2, "{log:?}");
        assert_eq!(log[0], "fired");
        assert_eq!(log[1], "completed");
        let _ = std::fs::remove_file(&plan_file);
    }

    /// A planner child that kept its own todo list hands those items to the
    /// caller on `Planned` — that is the channel the parent merges from.
    #[tokio::test]
    async fn planned_carries_the_planner_childs_own_todo_list() {
        let plan_file = tmp_plan_file("planner-todos");
        let spawner = Arc::new(
            MockSpawner::ok_writes(
                &plan_file,
                b"# Plan: foo\n\n## Task checklist\n- [ ] first step\n- [ ] second step\n",
            )
            .with_todos(&["first step", "second step"]),
        );
        let (_log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        let GoalPlannerOutcome::Planned { todos, .. } = outcome else {
            panic!("a written plan must return Planned");
        };
        assert_eq!(
            todos,
            vec!["first step".to_string(), "second step".to_string()],
            "the child's own list must ride back on the outcome",
        );
        let _ = std::fs::remove_file(&plan_file);
    }

    /// A planner that kept no list reports none: the caller falls back to the
    /// plan body rather than seeding an invented list.
    #[tokio::test]
    async fn planned_reports_no_todos_when_the_child_kept_none() {
        let plan_file = tmp_plan_file("planner-no-todos");
        let spawner = Arc::new(MockSpawner::ok_writes(
            &plan_file,
            b"# Plan: foo\n\n## Task checklist\n- [ ] first step\n",
        ));
        let (_log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        let GoalPlannerOutcome::Planned { todos, .. } = outcome else {
            panic!("a written plan must return Planned");
        };
        assert!(todos.is_empty());
        let _ = std::fs::remove_file(&plan_file);
    }

    #[tokio::test]
    async fn missing_plan_file_fails_closed() {
        let plan_file = tmp_plan_file("missing");
        let spawner = Arc::new(MockSpawner::ok_does_not_write(&plan_file));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(
            outcome,
            GoalPlannerOutcome::FailClosed {
                reason: GoalPlannerFailClosedReason::MissingPlan,
                ..
            }
        ));
        let log = log.lock().unwrap();
        assert!(log.iter().any(|t| t == "fail_closed:missing_plan_file"));
    }

    #[tokio::test]
    async fn transport_error_fails_closed_with_transport_reason() {
        let plan_file = tmp_plan_file("transport");
        let spawner = Arc::new(MockSpawner::fails(
            SpawnError::Transport("channel closed".to_string()),
            &plan_file,
        ));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(
            outcome,
            GoalPlannerOutcome::FailClosed {
                reason: GoalPlannerFailClosedReason::Transport,
                ..
            }
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| t == "fail_closed:transport")
        );
    }

    #[tokio::test]
    async fn runtime_cancelled_maps_to_aborted() {
        let plan_file = tmp_plan_file("aborted");
        let spawner = Arc::new(MockSpawner::fails(
            SpawnError::Runtime {
                message: "user aborted".into(),
                cancelled: true,
            },
            &plan_file,
        ));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(
            outcome,
            GoalPlannerOutcome::FailClosed {
                reason: GoalPlannerFailClosedReason::Aborted,
                ..
            }
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| t == "fail_closed:aborted")
        );
    }

    #[tokio::test]
    async fn runtime_not_cancelled_maps_to_runtime() {
        let plan_file = tmp_plan_file("runtime");
        let spawner = Arc::new(MockSpawner::fails(
            SpawnError::Runtime {
                message: "subagent crashed".into(),
                cancelled: false,
            },
            &plan_file,
        ));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(
            outcome,
            GoalPlannerOutcome::FailClosed {
                reason: GoalPlannerFailClosedReason::Runtime,
                ..
            }
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| t == "fail_closed:runtime")
        );
    }

    #[tokio::test]
    async fn malformed_terminal_with_file_present_still_succeeds() {
        // A botched terminal token is fine as long as the plan file is written.
        let plan_file = tmp_plan_file("malformed-but-written");
        let mut spawner = MockSpawner::ok_writes(&plan_file, b"# Plan: foo\n");
        spawner.response = Ok("done.".to_string());
        let spawner = Arc::new(spawner);
        let (_, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(outcome, GoalPlannerOutcome::Planned { .. }));
        let _ = std::fs::remove_file(&plan_file);
    }

    #[tokio::test]
    async fn empty_plan_file_fails_closed() {
        // Existence isn't enough; the file must be non-empty.
        let plan_file = tmp_plan_file("empty");
        let spawner = Arc::new(MockSpawner::ok_writes(&plan_file, b""));
        let (log, emit) = collect_events();

        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        assert!(matches!(
            outcome,
            GoalPlannerOutcome::FailClosed {
                reason: GoalPlannerFailClosedReason::MissingPlan,
                ..
            }
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|t| t.starts_with("fail_closed:"))
        );
    }

    #[tokio::test]
    async fn prompt_substitutes_plan_file_path_and_carries_objective() {
        let plan_file = tmp_plan_file("prompt");
        let spawner = Arc::new(MockSpawner::ok_writes(&plan_file, b"# Plan\n"));
        let spawner_obs = spawner.clone();
        let (_, emit) = collect_events();

        let _ = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "implement feature X",
                context: "prior conversation\n",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;

        let prompt = spawner_obs
            .last_prompt
            .lock()
            .unwrap()
            .clone()
            .expect("prompt captured");
        let expected_path = plan_file.to_string_lossy();
        assert!(
            !prompt.contains("{PLAN_FILE}"),
            "placeholder must be substituted"
        );
        assert!(
            prompt.contains(&*expected_path),
            "rendered prompt must reference plan path"
        );
        assert!(prompt.contains("OBJECTIVE:\nimplement feature X"));
        assert!(prompt.contains("CONTEXT:\nprior conversation"));
        let _ = std::fs::remove_file(&plan_file);
    }

    #[test]
    fn planner_prompt_pins_objective_fidelity_and_environment_evidence_contract() {
        // The optional risks section and the must-have-fidelity /
        // capturable-evidence rules are load-bearing for the
        // planner↔verifier contract; pin them so a template edit can't
        // silently drop them.
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Risks / Contradictions"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("must-have"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("capturable"));
    }

    /// Pin the anti-inflation + atomic-criterion rules (the failure mode this
    /// contract targets: re-inflated scope or a single holistic end-to-end gate).
    #[test]
    fn planner_prompt_pins_anti_inflation_and_atomic_criteria() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("do NOT invent scope"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("atomic and independently checkable"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("holistic end-to-end gate"));
        // A defining mechanic of a named artifact is requested, not "unrequested"
        // scope, so the carve-out keeps it out of Non-goals.
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("it is requested, so it stays here"));
    }

    /// Pin the static-check fallback and the run-is-the-evidence contract: a
    /// plan step names a command and what its output shows, never a file to
    /// save output to.
    #[test]
    fn planner_prompt_pins_static_fallback_and_recorded_runs() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("Static / structural fallback"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("the verifiers audit"));
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE.contains("the recorded runs rather than build their own")
        );
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("never require saving output"));
        assert!(!GOAL_PLANNER_PROMPT_TEMPLATE.contains("captured run output"));
    }

    /// Pin the outcomes-not-architecture contract: the frozen plan must not
    /// freeze the module/file layout or exact signatures.
    #[test]
    fn planner_prompt_pins_outcomes_not_architecture() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("OUTCOMES, not architecture"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("MUST NOT prescribe the module/file layout"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("exact signatures"));
    }

    /// A verification step checks what the objective asked for. It never
    /// widens the work, which is how a check became an unrequested action.
    #[test]
    fn planner_prompt_keeps_verification_inside_the_objective() {
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE
                .contains("Verification checks the work. It never adds to it"),
            "a check that acts beyond the objective is new scope, not verification"
        );
    }

    /// Pin the gating-vs-best-effort split: a small gating set decides pass/fail
    /// and best-effort `evidence` steps must not deny completion on their own.
    #[test]
    fn planner_prompt_pins_gating_vs_evidence_split() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("these are the GATING set"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("must NOT deny completion"));
    }

    /// Pin the minimal-honest-evidence path for headless-unobservable behavior:
    /// no mandated capture ritual/oracle, only artifact-exists + shipped units.
    #[test]
    fn planner_prompt_pins_minimal_honest_path_for_unobservable_behavior() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("MINIMAL honest path"));
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE.contains("verifier will then\n  rightly call theater")
        );
    }

    /// Pin the testable-structure guidance (separate logic from I/O so
    /// tests drive the real shipped code), and that it stays design
    /// guidance, not an acceptance criterion.
    #[test]
    fn planner_prompt_requires_testable_structure_guidance() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Implementation approach"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("easy to test"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("NOT an acceptance criterion"));
    }

    /// Pin the `## Task checklist` contract: the planner emits `- [ ]`
    /// checkbox steps (code-change only) that `goal_next_step` mines for
    /// the per-turn nudge, and the checklist is HOW guidance, never part
    /// of the judged contract.
    #[test]
    fn planner_prompt_requires_task_checklist_section() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Task checklist"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("unchecked box"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("never part of the judged contract"),);
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE.contains("as many ordered"),
            "checklist size must follow the work, not a 4–6 / 3–8 cap"
        );
        assert!(
            !GOAL_PLANNER_PROMPT_TEMPLATE.contains("3-8"),
            "a numeric todo cap makes small tasks over-specific and large ones underspecified"
        );
        assert!(
            !GOAL_PLANNER_PROMPT_TEMPLATE.contains("4-6")
                && !GOAL_PLANNER_PROMPT_TEMPLATE.contains("4–6"),
            "a numeric todo cap makes small tasks over-specific and large ones underspecified"
        );
    }

    /// Pin the visual/interactive guidance: gamedev/UI goals must be
    /// anchored on the static/structural fallback plus unit tests of the
    /// pure logic (physics, collision, input mapping), with capturable
    /// extras as `evidence`, never `gating`.
    #[test]
    fn planner_prompt_covers_visual_interactive_goals() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Visual / interactive objectives"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("input mapping"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("as `evidence`, never as `gating`"));
        // Browser-load check: scripts must provably load without Node globals.
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("NO Node globals"));
        // Launch check is gating for every runnable deliverable, with the
        // headless page-load as the browser instance of the general rule.
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Entry-point launch check"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("bare specifiers"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("Server/service"));
    }

    /// The launch gate must prove the primary observable is CORRECT, not just
    /// present — keeps a renders-but-wrong deliverable from passing planning.
    #[test]
    fn planner_prompt_requires_correct_primary_observable_launch_gate() {
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("PRIMARY OBSERVABLE is CORRECT"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("present and non-empty is INSUFFICIENT"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("output CONTENT, not just that it ran"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("the response BODY is sane"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("assert a real call's RETURN VALUE"));
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE
                .contains("drawing dimensions equal the intended/target size")
        );
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("SUBSTANTIALLY filled"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("NOT a `> 0 pixels` check"));
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE
                .contains("a driven input produces the expected visible change")
        );
    }

    /// The launch gate must run more than once and route non-determinism by
    /// cause — an app defect is fixed, a flaky/unobservable environment falls
    /// back — so a correct app converges and the readback seam stays shut.
    #[test]
    fn planner_prompt_requires_repeated_consistent_launch() {
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE.contains("MORE THAN ONCE and assert CONSISTENT success")
        );
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("cherry-pick a success"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("APP-side defect to FIX"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("if the ENVIRONMENT is what's flaky"));
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE
                .contains("cannot reliably read back the primary observable")
        );
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE
                .contains("is the app's output, not an unavailable readback")
        );
    }

    #[test]
    fn planner_prompt_requires_shared_verification_plan_section() {
        // The `## Verification plan` is the shared implementer↔verifier
        // procedure (the bias-reduction mechanism): pin both that the
        // section is required and that it is framed as observable
        // checks rather than free-text "it works".
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("## Verification plan"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("implementer"));
        assert!(GOAL_PLANNER_PROMPT_TEMPLATE.contains("observations that MUST be"));
    }

    /// The planner must instruct verification-plan output paths to use the
    /// literal `{SCRATCH}` placeholder instead of hardcoded `/tmp/...`, so
    /// the implementer and each skeptic write to distinct private dirs and
    /// never race on a shared screenshot file.
    #[test]
    fn planner_prompt_instructs_scratch_placeholder() {
        assert!(
            GOAL_PLANNER_PROMPT_TEMPLATE.contains("{SCRATCH}"),
            "planner prompt must instruct the `{{SCRATCH}}` placeholder",
        );
    }

    // ── RoleSpawnOverride + spawn-and-retry-once wrapper ─────

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A `RoleRenderedPrompt` whose two renders are identical (the inherit /
    /// same-toolset case). Tests that exercise the distinct-fallback path build
    /// the struct inline instead.
    fn role_prompt(p: &str) -> RoleRenderedPrompt {
        RoleRenderedPrompt {
            primary: p.to_string(),
            fallback: p.to_string(),
        }
    }

    /// 3-role parity: an explicit planner pair threads `agent_type` as the
    /// request's `harness_agent_type`, not the subagent_type.
    #[tokio::test]
    async fn channel_spawner_threads_harness_override_to_request() {
        use xai_grok_tools::implementations::grok_build::task::types::{
            SubagentEvent, SubagentResult,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let spawner = ChannelSpawner {
            event_tx: tx,
            foreground_wait: None,
            parent_session_id: "parent".into(),
            parent_prompt_id: None,
            cwd: None,
            trace_sink: None,
            role_override: RoleSpawnOverride {
                model: Some("cfg-model".into()),
                agent_type: Some("cursor".into()),
            },
            cancel_token: tokio_util::sync::CancellationToken::new(),
            subagent_id_slot: None,
            fallback: RoleFallbackReporter::default(),
        };
        let handle = tokio::spawn(async move {
            let _ = spawner
                .spawn_planner("plan-id", role_prompt("prompt"))
                .await;
        });
        let SubagentEvent::Spawn(request) = rx.recv().await.expect("spawn event") else {
            panic!("expected Spawn");
        };
        assert_eq!(
            request.subagent_type, GOAL_PLANNER_SUBAGENT_TYPE,
            "planner always spawns the fixed role subagent type",
        );
        assert_eq!(
            request.runtime_overrides.harness_agent_type.as_deref(),
            Some("cursor"),
            "configured agent_type must thread as the harness override",
        );
        assert_eq!(
            request.runtime_overrides.model.as_deref(),
            Some("cfg-model")
        );
        // Reply SUCCESS so the explicit pair does NOT trigger a fail-open retry.
        let _ = request.result_tx.send(SubagentResult {
            success: true,
            output: std::sync::Arc::from("ok"),
            ..Default::default()
        });
        handle.await.unwrap();
    }

    #[test]
    fn effective_role_model_id_prefers_override_else_parent() {
        assert_eq!(
            effective_role_model_id(Some("override-model"), "parent-model"),
            "override-model",
        );
        assert_eq!(
            effective_role_model_id(None, "parent-model"),
            "parent-model"
        );
    }

    #[test]
    fn role_spawn_override_is_explicit() {
        assert!(!RoleSpawnOverride::default().is_explicit());
        assert!(
            RoleSpawnOverride {
                model: Some("m".into()),
                agent_type: None,
            }
            .is_explicit()
        );
        assert!(
            RoleSpawnOverride {
                model: None,
                agent_type: Some("t".into()),
            }
            .is_explicit()
        );
    }

    #[tokio::test]
    async fn retry_once_on_explicit_failure_then_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ov = RoleSpawnOverride {
            model: Some("cfg-model".into()),
            agent_type: Some("cfg-type".into()),
        };
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "planner",
            None,
            &ov,
            &RoleFallbackReporter::default(),
            role_prompt("PROMPT"),
            |model, harness, prompt| {
                let c = c.clone();
                async move {
                    // The prompt is forwarded to every attempt.
                    assert_eq!(prompt, "PROMPT");
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        // First attempt uses the configured pair.
                        assert_eq!(model.as_deref(), Some("cfg-model"));
                        assert_eq!(harness.as_deref(), Some("cfg-type"));
                        Err(SpawnError::Transport("boom".into()))
                    } else {
                        // Retry drops the model + harness (inherit session harness).
                        assert_eq!(model, None, "retry must inherit the current model");
                        assert_eq!(harness, None, "retry must inherit the session harness");
                        Ok("retried".into())
                    }
                }
            },
        )
        .await;
        assert_eq!(out.unwrap(), "retried");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "explicit failure retries once"
        );
    }

    /// The explicit-pair retry must use the `fallback` render (default-toolset
    /// tool names), not re-send the `primary` render (configured pair's names).
    #[tokio::test]
    async fn retry_uses_fallback_render_not_primary() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ov = RoleSpawnOverride {
            model: Some("cfg-model".into()),
            agent_type: Some("cfg-type".into()),
        };
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "planner",
            None,
            &ov,
            &RoleFallbackReporter::default(),
            RoleRenderedPrompt {
                primary: "PRIMARY".to_string(),
                fallback: "FALLBACK".to_string(),
            },
            |_model, _harness, prompt| {
                let c = c.clone();
                async move {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        assert_eq!(prompt, "PRIMARY", "first attempt uses the primary render");
                        Err(SpawnError::Transport("boom".into()))
                    } else {
                        assert_eq!(
                            prompt, "FALLBACK",
                            "retry must use the default-toolset fallback render",
                        );
                        Ok("retried".into())
                    }
                }
            },
        )
        .await;
        assert_eq!(out.unwrap(), "retried");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn inherit_override_never_retries() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ov = RoleSpawnOverride::default();
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "planner",
            None,
            &ov,
            &RoleFallbackReporter::default(),
            role_prompt("PROMPT"),
            |model, harness, _prompt| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(model, None);
                    assert_eq!(harness, None, "inherit must not pin a harness");
                    Err(SpawnError::Transport("boom".into()))
                }
            },
        )
        .await;
        assert!(out.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "inherit already ran on the current model — no retry",
        );
    }

    #[tokio::test]
    async fn explicit_success_does_not_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ov = RoleSpawnOverride {
            model: Some("m".into()),
            agent_type: None,
        };
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "skeptic",
            Some(2),
            &ov,
            &RoleFallbackReporter::default(),
            role_prompt("PROMPT"),
            |_m, _h, _prompt| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok("ok".into())
                }
            },
        )
        .await;
        assert_eq!(out.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A CANCELLATION on the explicit-pair path must propagate
    /// WITHOUT a fail-open retry (so the planner still pauses as `Aborted`
    /// rather than silently re-running on the current model).
    #[tokio::test]
    async fn cancellation_on_explicit_path_does_not_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ov = RoleSpawnOverride {
            model: Some("cfg-model".into()),
            agent_type: Some("cfg-type".into()),
        };
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "planner",
            None,
            &ov,
            &RoleFallbackReporter::default(),
            role_prompt("PROMPT"),
            |_m, _h, _prompt| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err(SpawnError::Runtime {
                        message: "user aborted".into(),
                        cancelled: true,
                    })
                }
            },
        )
        .await;
        assert!(
            matches!(
                out,
                Err(SpawnError::Runtime {
                    cancelled: true,
                    ..
                })
            ),
            "a cancellation must propagate unchanged",
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "cancellation must NOT trigger the inherit retry",
        );
    }

    /// A reporter wired to a real notify sender, and the persistence channel
    /// that every notice it sends lands on.
    fn notice_reporter() -> (
        RoleFallbackReporter,
        tokio::sync::mpsc::UnboundedReceiver<crate::session::persistence::PersistenceMsg>,
    ) {
        let (gateway_tx, _gateway_rx) = tokio::sync::mpsc::unbounded_channel();
        let (persist_tx, persist_rx) = tokio::sync::mpsc::unbounded_channel();
        let notify = crate::session::goal_orchestrator::GoalNotifySender::new(
            agent_client_protocol::SessionId::new("s"),
            xai_acp_lib::AcpAgentGatewaySender::new(gateway_tx),
            persist_tx,
        );
        let reporter = RoleFallbackReporter {
            events: None,
            notify: Some(notify),
            fallback_model: Some("grok-4.7".into()),
        };
        (reporter, persist_rx)
    }

    /// Every `GoalRoleModelFallback` notice the channel holds.
    fn drain_fallback_notices(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::persistence::PersistenceMsg>,
    ) -> Vec<crate::extensions::notification::SessionUpdate> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let crate::session::persistence::PersistenceMsg::Update(
                crate::session::storage::SessionUpdate::Xai(n),
            ) = msg
                && matches!(
                    n.update,
                    crate::extensions::notification::SessionUpdate::GoalRoleModelFallback { .. }
                )
            {
                out.push(n.update);
            }
        }
        out
    }

    /// The user must SEE a fallback, with the spawn's own error text. An
    /// event in `events.jsonl` alone reaches only a dashboard.
    #[tokio::test]
    async fn fail_open_retry_tells_the_user_why() {
        let ov = RoleSpawnOverride {
            model: Some("claude-opus".into()),
            agent_type: None,
        };
        let (reporter, mut rx) = notice_reporter();
        let out: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "skeptic",
            Some(0),
            &ov,
            &reporter,
            role_prompt("PROMPT"),
            |model, _h, _prompt| async move {
                match model {
                    Some(_) => Err(SpawnError::Transport("401 bad api key".into())),
                    None => Ok("ok".to_string()),
                }
            },
        )
        .await;
        assert_eq!(out.unwrap(), "ok");
        let notices = drain_fallback_notices(&mut rx);
        assert_eq!(notices.len(), 1, "one notice per fallback: {notices:?}");
        let crate::extensions::notification::SessionUpdate::GoalRoleModelFallback {
            role,
            skeptic_idx,
            requested_model,
            fallback_model,
            reason,
            detail,
        } = &notices[0]
        else {
            unreachable!()
        };
        assert_eq!(role, "skeptic");
        assert_eq!(*skeptic_idx, Some(0));
        assert_eq!(requested_model, "claude-opus");
        assert_eq!(fallback_model.as_deref(), Some("grok-4.7"));
        assert_eq!(reason, "spawn_failed");
        assert!(
            detail
                .as_deref()
                .is_some_and(|d| d.contains("401 bad api key")),
            "the spawn's own error must reach the user: {detail:?}"
        );
    }

    /// A spawn on the configured model that succeeds is not a fallback.
    #[tokio::test]
    async fn a_successful_configured_spawn_sends_no_notice() {
        let ov = RoleSpawnOverride {
            model: Some("claude-opus".into()),
            agent_type: None,
        };
        let (reporter, mut rx) = notice_reporter();
        let _: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "skeptic",
            Some(0),
            &ov,
            &reporter,
            role_prompt("PROMPT"),
            |_m, _h, _prompt| async move { Ok::<_, SpawnError>("ok".to_string()) },
        )
        .await;
        assert!(drain_fallback_notices(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn fail_open_retry_emits_spawn_failed_event() {
        let dir = tempfile::tempdir().unwrap();
        let writer = xai_file_utils::events::EventWriter::open(dir.path());
        let ov = RoleSpawnOverride {
            model: Some("m".into()),
            agent_type: Some("t".into()),
        };
        let reporter = RoleFallbackReporter {
            events: Some(writer),
            ..Default::default()
        };
        // Both attempts fail: wrapper still emits the SpawnFailed reason once.
        let _: Result<String, SpawnError> = spawn_with_fail_open_retry(
            "skeptic",
            Some(1),
            &ov,
            &reporter,
            role_prompt("PROMPT"),
            |_m, _h, _prompt| async move { Err::<String, _>(SpawnError::Transport("x".into())) },
        )
        .await;
        let mut body = String::new();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            body.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default());
        }
        assert!(
            body.contains("goal_role_model_fail_open"),
            "must emit fail-open event: {body}",
        );
        assert!(body.contains("spawn_failed"), "reason must be spawn_failed");
        assert!(body.contains("\"skeptic_idx\":1"), "skeptic_idx carried");
    }

    /// Load-bearing (Key Decision #13): a bad configured pair must NOT
    /// convert the fail-CLOSED planner into a goal-pause. The retry-once
    /// wrapper degrades the explicit pair to the current model, so a
    /// `ChannelSpawner` whose explicit spawn fails still returns `Planned`.
    #[tokio::test]
    async fn planner_retries_to_inherit_instead_of_failing_closed() {
        use xai_grok_tools::implementations::grok_build::task::types::{
            SubagentEvent, SubagentResult,
        };
        let plan_file = tmp_plan_file("retry-failopen");
        let plan_for_coord = plan_file.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Fake coordinator: explicit-model spawn fails; the inherit retry
        // (model None) writes the plan and succeeds.
        let coord = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let SubagentEvent::Spawn(req) = ev else {
                    continue;
                };
                if req.runtime_overrides.model.is_some() {
                    let _ = req.result_tx.send(SubagentResult {
                        success: false,
                        error: Some("bad configured model".into()),
                        ..Default::default()
                    });
                } else {
                    let _ = tokio::fs::write(&plan_for_coord, b"# Plan\n").await;
                    let _ = req.result_tx.send(SubagentResult {
                        success: true,
                        output: std::sync::Arc::from("Done"),
                        ..Default::default()
                    });
                }
            }
        });
        let spawner = Arc::new(ChannelSpawner {
            event_tx: tx,
            foreground_wait: None,
            parent_session_id: "p".into(),
            parent_prompt_id: None,
            cwd: None,
            trace_sink: None,
            role_override: RoleSpawnOverride {
                model: Some("cfg-model".into()),
                agent_type: Some("general-purpose".into()),
            },
            cancel_token: tokio_util::sync::CancellationToken::new(),
            subagent_id_slot: None,
            fallback: RoleFallbackReporter::default(),
        });
        let (_log, emit) = collect_events();
        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;
        assert!(
            matches!(outcome, GoalPlannerOutcome::Planned { .. }),
            "bad pair must retry to inherit, never fail closed: {outcome:?}",
        );
        drop(coord);
        let _ = std::fs::remove_file(&plan_file);
    }

    /// Integration: a CANCELLED explicit-pair planner spawn must
    /// still pause the goal as `Aborted` — the cancellation propagates with
    /// NO inherit retry (only ONE spawn is sent), preserving today's
    /// fail-CLOSED cancellation semantics.
    #[tokio::test]
    async fn planner_cancellation_pauses_as_aborted_without_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use xai_grok_tools::implementations::grok_build::task::types::{
            SubagentEvent, SubagentResult,
        };
        let plan_file = tmp_plan_file("cancel-aborted");
        let spawns = Arc::new(AtomicUsize::new(0));
        let spawns_coord = spawns.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Fake coordinator: every spawn reports a cancellation.
        let coord = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let SubagentEvent::Spawn(req) = ev else {
                    continue;
                };
                spawns_coord.fetch_add(1, Ordering::SeqCst);
                let _ = req.result_tx.send(SubagentResult {
                    success: false,
                    cancelled: true,
                    error: Some("user aborted".into()),
                    ..Default::default()
                });
            }
        });
        let spawner = Arc::new(ChannelSpawner {
            event_tx: tx,
            foreground_wait: None,
            parent_session_id: "p".into(),
            parent_prompt_id: None,
            cwd: None,
            trace_sink: None,
            role_override: RoleSpawnOverride {
                model: Some("cfg-model".into()),
                agent_type: Some("general-purpose".into()),
            },
            cancel_token: tokio_util::sync::CancellationToken::new(),
            subagent_id_slot: None,
            fallback: RoleFallbackReporter::default(),
        });
        let (_log, emit) = collect_events();
        let outcome = run_goal_planner(
            spawner,
            GoalPlannerInputs {
                objective: "do X",
                context: "",
                plan_file: &plan_file,
                attempt: 1,
                model_id: "grok-test",
                tool_names: &RoleToolNames::inherit_defaults(),
                inherit_tool_names: &RoleToolNames::inherit_defaults(),
            },
            &emit,
        )
        .await;
        assert!(
            matches!(
                outcome,
                GoalPlannerOutcome::FailClosed {
                    reason: GoalPlannerFailClosedReason::Aborted,
                    ..
                }
            ),
            "a cancelled explicit-pair spawn must fail closed as Aborted: {outcome:?}",
        );
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            1,
            "cancellation must NOT trigger the inherit retry (one spawn only)",
        );
        drop(coord);
        let _ = std::fs::remove_file(&plan_file);
    }
}
