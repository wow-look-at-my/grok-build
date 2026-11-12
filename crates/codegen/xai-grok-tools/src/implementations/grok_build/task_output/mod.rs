//! `get_task_output` tool — output/status for one or many background tasks.
//!
//! Every call is a non-blocking snapshot.

pub mod terminal_command;
pub use terminal_command::GetTerminalCommandOutputTool;

use crate::DEFAULT_TOOL_OUTPUT_BYTES;
use crate::implementations::BashTool;
use crate::implementations::grok_build::task::TaskTool;
use crate::implementations::grok_build::task::backend::SubagentBackendResource;
use crate::implementations::grok_build::task::types::{SubagentSnapshot, SubagentSnapshotStatus};
use crate::implementations::grok_build_concise::BashConciseTool;
use crate::implementations::opencode::OpenCodeBashTool;
use crate::implementations::task_output::tool::snapshot_to_result;
use crate::types::requirements::{Expr, ToolParamsRequirement, ToolRequirement};
use crate::types::resources::{SharedResources, Terminal, TruncationCfg};
use crate::types::template_renderer::TemplateRenderer;
use crate::types::tool::{ToolKind, ToolNamespace};
use xai_tool_types::{
    MultiTaskOutputResult, TaskOutputOutput, TaskOutputResult, TaskOutputToolInput,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitSubject {
    Task,
    Subagent,
}

/// The hint that ends every snapshot of a live task or subagent.
fn still_running_hint(subject: WaitSubject) -> &'static str {
    // Bash tasks have no follow-up channel; only subagents can be told to stop.
    match subject {
        WaitSubject::Task => {
            "Still running. This tool does not wait. Do other work now. When you have nothing \
             else to do, end your turn: a completion message is delivered when the task \
             finishes. Unless the user specified, do not kill this task."
        }
        WaitSubject::Subagent => {
            "Still running. This tool does not wait. Do other work now. When you have nothing \
             else to do, end your turn: a completion message is delivered when the subagent \
             finishes. Unless the user specified, do not kill this subagent and do not tell it \
             to stop."
        }
    }
}

fn with_still_running_hint(body: String, subject: WaitSubject) -> String {
    format!("{body}\n\n{}", still_running_hint(subject))
}

fn apply_running_hint(mut result: TaskOutputResult, subject: WaitSubject) -> TaskOutputResult {
    if result.status == "running" {
        result.output = with_still_running_hint(std::mem::take(&mut result.output), subject);
    }
    result
}

pub(crate) fn background_bash_requires_exprs() -> Vec<Expr<ToolRequirement>> {
    use crate::types::tool_metadata::ToolMetadata;
    let grok_build_bash = Expr::Value(ToolRequirement::Tool {
        namespace: ToolMetadata::tool_namespace(&BashTool).to_string(),
        id: xai_tool_runtime::Tool::id(&BashTool).as_str().to_string(),
        if_params: Some(Expr::Value(ToolParamsRequirement {
            key: "enabled_background".to_string(),
            value: Expr::Value(serde_json::Value::Bool(true)),
        })),
    });
    let grok_build_concise_bash = Expr::Value(ToolRequirement::Tool {
        namespace: ToolMetadata::tool_namespace(&BashConciseTool).to_string(),
        id: xai_tool_runtime::Tool::id(&BashConciseTool)
            .as_str()
            .to_string(),
        if_params: Some(Expr::Value(ToolParamsRequirement {
            key: "enabled_background".to_string(),
            value: Expr::Value(serde_json::Value::Bool(true)),
        })),
    });
    let opencode_bash = Expr::Value(ToolRequirement::Tool {
        namespace: ToolMetadata::tool_namespace(&OpenCodeBashTool).to_string(),
        id: xai_tool_runtime::Tool::id(&OpenCodeBashTool)
            .as_str()
            .to_string(),
        if_params: None,
    });
    vec![grok_build_bash, grok_build_concise_bash, opencode_bash]
}

/// The `requires_expr` of `get_task_output`.
pub(crate) fn task_output_requires_expr() -> Expr<ToolRequirement> {
    use crate::types::tool_metadata::ToolMetadata;
    let task_tool = Expr::Value(ToolRequirement::Tool {
        namespace: ToolMetadata::tool_namespace(&TaskTool).to_string(),
        id: xai_tool_runtime::Tool::id(&TaskTool).as_str().to_string(),
        if_params: None,
    });
    let mut arms = background_bash_requires_exprs();
    arms.push(task_tool);
    Expr::Or(arms)
}

#[derive(Debug, Default)]
pub struct TaskOutputTool;

impl TaskOutputTool {
    /// The standard output of an `lpi` invocation, or nothing when it fails.
    async fn run_lpi(argv: &[String]) -> Option<String> {
        let (program, args) = argv.split_first()?;
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// A reading of a task's own log, when `lpi` can give one. A running task is
    /// read as it stands. A task that finished successfully seeds the model when
    /// none exists, because reading needs one and an unfinished log is
    /// truncated.
    async fn lpi_reading(snapshot: &crate::computer::types::TaskSnapshot) -> Option<String> {
        if !crate::lpi_progress::available() {
            return None;
        }
        let key = crate::lpi_progress::key_for(&snapshot.cwd, &snapshot.command);
        let log = snapshot.output_file.as_path();

        let Some(stdout) = Self::run_lpi(&crate::lpi_progress::read_argv(&key, log, None)).await
        else {
            if snapshot.completed && snapshot.exit_code == Some(0) {
                let _ = Self::run_lpi(&crate::lpi_progress::learn_argv(&key, log)).await;
            }
            return None;
        };

        let reading = stdout
            .lines()
            .rev()
            .find_map(crate::lpi_progress::parse_snapshot)?;
        Some(crate::lpi_progress::progress_line(&reading))
    }

    async fn run_single_task(
        &self,
        task_id: &str,
        ctx: &xai_tool_runtime::ToolCallContext,
        resources: SharedResources,
        output_byte_limit: Option<usize>,
    ) -> Result<TaskOutputOutput, xai_tool_runtime::ToolError> {
        let contract_version = ctx
            .extensions
            .get::<xai_tool_runtime::BehaviorVersion>()
            .map(|v| v.0.clone());
        let is_legacy = crate::versions::is_legacy_contract(contract_version.as_deref());
        let (terminal, my_owner) = {
            let res = resources.lock().await;
            (
                res.require::<Terminal>()?.0.clone(),
                res.get::<crate::types::resources::OwnerSessionId>()
                    .map(|owner| owner.0.clone()),
            )
        };

        if let Some(snapshot) = terminal.get_task(task_id).await {
            let read_file_name;
            {
                let res = resources.lock().await;
                let renderer = res.require::<TemplateRenderer>()?;
                read_file_name = renderer
                    .render("${{ tools.by_kind.read }}")
                    .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;
            }
            let max_output_bytes = {
                let res = resources.lock().await;
                resolved_max_output_bytes(&res, "get_command_or_subagent_output", output_byte_limit)
            };

            // A reading of the task's own log, folded in as the tool's own line.
            let mut snapshot = snapshot;
            if let Some(line) = Self::lpi_reading(&snapshot).await {
                snapshot.output.push_str(&format!("\n[lpi {line}]"));
            }

            return Ok(TaskOutputOutput::Result(apply_running_hint(
                snapshot_to_result(snapshot, &read_file_name, max_output_bytes),
                WaitSubject::Task,
            )));
        }

        let backend = {
            resources
                .lock()
                .await
                .get::<SubagentBackendResource>()
                .cloned()
        };
        if let Some(backend) = backend
            && let Some(snapshot) = backend.backend().query(task_id, false, None).await
        {
            return Ok(format_subagent_snapshot(&snapshot));
        }

        // Neither found
        {
            let msg = if is_legacy {
                render_legacy_task_output_not_found(task_id)
            } else {
                // The terminal backend is shared with the root session.
                let mut known = terminal.list_tasks().await;
                known.retain(|task| {
                    crate::reminders::task_completion::task_owned_by_session(
                        task,
                        my_owner.as_deref(),
                    )
                });
                if known.is_empty() {
                    format!(
                        "Task {task_id} not found. No background tasks or subagents exist in this session.",
                    )
                } else {
                    let ids: Vec<&str> = known.iter().map(|t| t.task_id.as_str()).collect();
                    format!(
                        "Task {task_id} not found. Known task IDs: [{}]",
                        ids.join(", ")
                    )
                }
            };
            Ok(TaskOutputOutput::TaskNotFound(msg))
        }
    }

    pub(crate) async fn run_multi_tasks_limited(
        task_ids: &[String],
        resources: SharedResources,
        tool_name_for_truncation: &str,
        output_byte_limit: Option<usize>,
    ) -> Result<TaskOutputOutput, xai_tool_runtime::ToolError> {
        let (terminal, backend, read_file_name, max_output_bytes) = {
            let res = resources.lock().await;
            let terminal = res.require::<Terminal>()?.0.clone();
            let backend = res.get::<SubagentBackendResource>().cloned();
            let renderer = res.require::<TemplateRenderer>()?;
            let rfn = renderer
                .render("${{ tools.by_kind.read }}")
                .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;
            let mob = resolved_max_output_bytes(&res, tool_name_for_truncation, output_byte_limit);
            (terminal, backend, rfn, mob)
        };

        let results = resolve_tasks(
            task_ids,
            &terminal,
            &backend,
            &read_file_name,
            max_output_bytes,
        )
        .await;

        let completed_count = results.iter().filter(|r| r.is_terminal()).count();
        let total = results.len();
        let summary = format!("{completed_count}/{total} tasks completed (poll)");

        Ok(TaskOutputOutput::MultiResult(MultiTaskOutputResult {
            mode: "poll".to_string(),
            results,
            summary,
        }))
    }
}

pub(crate) use xai_tool_types::MAX_MULTI_WAIT_IDS;

pub(crate) fn not_found_result(task_id: &str) -> TaskOutputResult {
    TaskOutputResult {
        task_id: task_id.to_string(),
        command: String::new(),
        status: "not_found".to_string(),
        exit_code: None,
        started: String::new(),
        ended: None,
        duration_secs: 0.0,
        output: format!("Task {task_id} not found."),
        output_file: String::new(),
        truncated: false,
        truncation_hint: String::new(),
        raw_output_bytes: 0,
    }
}

/// A snapshot of each task, in order, from the terminal or the subagent coordinator.
pub(crate) async fn resolve_tasks(
    task_ids: &[String],
    terminal: &std::sync::Arc<dyn crate::computer::types::TerminalBackend>,
    backend: &Option<SubagentBackendResource>,
    read_file_name: &str,
    max_output_bytes: usize,
) -> Vec<TaskOutputResult> {
    let mut results = Vec::with_capacity(task_ids.len());

    for id in task_ids {
        if let Some(snap) = terminal.get_task(id).await {
            results.push(apply_running_hint(
                snapshot_to_result(snap, read_file_name, max_output_bytes),
                WaitSubject::Task,
            ));
            continue;
        }

        if let Some(be) = backend
            && let Some(snap) = be.backend().query(id, false, None).await
            && let TaskOutputOutput::Result(r) = format_subagent_snapshot(&snap)
        {
            results.push(r);
            continue;
        }

        results.push(not_found_result(id));
    }

    results
}

// Historical fixture captured from the 0.4.10 implementation. In 0.4.10, get_task_output returned:
// Err(ToolError::ProcessManagerError(format!("Task {} not found", input.task_id))) The meaningful customer-facing
// message content is the inner string. Subagent wording is out of scope — subagents didn't exist in 0.4.10.

/// Exact historical not-found message for `get_task_output` in legacy-0.4.10.
fn render_legacy_task_output_not_found(task_id: &str) -> String {
    format!("Task {} not found", task_id)
}

pub(crate) fn format_subagent_snapshot(snap: &SubagentSnapshot) -> TaskOutputOutput {
    match &snap.status {
        SubagentSnapshotStatus::Initializing => {
            let started = format_epoch_ms_as_rfc3339(snap.started_at_epoch_ms);
            let duration_secs = snap.duration_ms as f64 / 1000.0;
            // Measure body only — the hint is harness advisory, not task output.
            let body = format!(
                "Subagent is initializing (creating worktree, resolving config).\n\
                 Type: {}\n\
                 Description: {}\n\
                 Elapsed: {duration_secs:.1}s",
                snap.subagent_type, snap.description,
            );
            let raw_output_bytes = body.len();
            let output = with_still_running_hint(body, WaitSubject::Subagent);
            TaskOutputOutput::Result(TaskOutputResult {
                task_id: snap.subagent_id.clone(),
                command: format!("[subagent:{}] {}", snap.subagent_type, snap.description),
                status: "initializing".to_string(),
                exit_code: None,
                started,
                ended: None,
                duration_secs,
                output,
                output_file: String::new(),
                truncated: false,
                truncation_hint: String::new(),
                raw_output_bytes,
            })
        }
        SubagentSnapshotStatus::Running {
            turn_count,
            tool_call_count,
            tokens_used,
            context_window_tokens,
            context_usage_pct,
            tools_used,
            error_count,
        } => {
            let started = format_epoch_ms_as_rfc3339(snap.started_at_epoch_ms);
            let tools_str = if tools_used.is_empty() {
                "none yet".to_string()
            } else {
                tools_used.join(", ")
            };
            let tokens_k = tokens_used / 1000;
            let capacity_k = context_window_tokens / 1000;
            // Measure body only — the hint is harness advisory, not task output.
            let body = format!(
                "Subagent is still running.\n\
                 Type: {}\n\
                 Description: {}\n\
                 Elapsed: {:.1}s\n\
                 Progress: turn {turn_count}, {tool_call_count} tool calls, \
                 {tokens_k}K/{capacity_k}K tokens ({context_usage_pct}% context)\n\
                 Tools used: {tools_str}\n\
                 Errors: {error_count}",
                snap.subagent_type,
                snap.description,
                snap.duration_ms as f64 / 1000.0,
            );
            let raw_output_bytes = body.len();
            let output = with_still_running_hint(body, WaitSubject::Subagent);
            TaskOutputOutput::Result(TaskOutputResult {
                task_id: snap.subagent_id.clone(),
                command: format!("[subagent:{}] {}", snap.subagent_type, snap.description),
                status: "running".to_string(),
                exit_code: None,
                started,
                ended: None,
                duration_secs: snap.duration_ms as f64 / 1000.0,
                output,
                output_file: String::new(),
                truncated: false,
                truncation_hint: String::new(),
                raw_output_bytes,
            })
        }
        SubagentSnapshotStatus::Completed { .. }
        | SubagentSnapshotStatus::Failed { .. }
        | SubagentSnapshotStatus::Cancelled { .. } => {
            TaskOutputOutput::Result(terminal_subagent_result(snap))
        }
    }
}

/// Terminal statuses only; reminders render the same value, so notice and poll cannot drift.
pub(crate) fn terminal_subagent_result(snap: &SubagentSnapshot) -> TaskOutputResult {
    let (status, exit_code, output) = match &snap.status {
        SubagentSnapshotStatus::Completed {
            output,
            tool_calls,
            turns,
            worktree_path,
        } => {
            let mut output = format!(
                "{output}\n\n<subagent_meta>id={}, tool_calls={tool_calls}, \
                 turns={turns}, duration_ms={}</subagent_meta>",
                snap.subagent_id, snap.duration_ms,
            );
            if let Some(wt) = &worktree_path {
                output.push_str(&format!("\n<worktree_path>{wt}</worktree_path>"));
            }
            output.push_str("\n\n");
            output.push_str(&xai_tool_types::format_resume_footer(
                &snap.subagent_id,
                snap.persona.as_deref(),
            ));
            ("completed", Some(0), output)
        }
        SubagentSnapshotStatus::Failed { error } => ("failed", Some(1), error.clone()),
        SubagentSnapshotStatus::Cancelled { reason } => (
            "cancelled",
            None,
            reason
                .clone()
                .unwrap_or_else(|| "Subagent was cancelled".to_string()),
        ),
        SubagentSnapshotStatus::Initializing | SubagentSnapshotStatus::Running { .. } => {
            unreachable!("terminal_subagent_result called for a live subagent")
        }
    };
    let ended_at_epoch_ms = snap.started_at_epoch_ms + snap.duration_ms;
    TaskOutputResult {
        task_id: snap.subagent_id.clone(),
        command: format!("[subagent:{}] {}", snap.subagent_type, snap.description),
        status: status.to_string(),
        exit_code,
        started: format_epoch_ms_as_rfc3339(snap.started_at_epoch_ms),
        ended: Some(format_epoch_ms_as_rfc3339(ended_at_epoch_ms)),
        duration_secs: snap.duration_ms as f64 / 1000.0,
        raw_output_bytes: output.len(),
        output,
        output_file: String::new(),
        truncated: false,
        truncation_hint: String::new(),
    }
}

fn format_epoch_ms_as_rfc3339(epoch_ms: u64) -> String {
    use chrono::{DateTime, Utc};
    let secs = (epoch_ms / 1000) as i64;
    let nanos = ((epoch_ms % 1000) * 1_000_000) as u32;
    match DateTime::from_timestamp(secs, nanos) {
        Some(dt) => dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        None => Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}

impl crate::types::tool_metadata::ToolMetadata for TaskOutputTool {
    fn kind(&self) -> ToolKind {
        ToolKind::BackgroundTaskAction
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        // Canonical wording lives in the shared builder; `versioned_definition`
        // renders it context-aware from the finalized toolset. This static
        // fallback mirrors the default grok-build toolset.
        static DESC: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            xai_tool_types::build_task_output_description(&xai_tool_types::TaskOutputToolNaming {
                monitor_tool: Some("monitor"),
                read_tool: Some("read_file"),
                bash_background_param: Some("is_background"),
                subagent_background_param: Some("run_in_background"),
                task_ids_param: "task_ids",
                task_id_param: "task_id",
            })
        });
        &DESC
    }

    fn versioned_definition(
        &self,
        _contract_version: Option<&str>,
        client_name: &str,
        description_override: Option<&str>,
        renderer: &TemplateRenderer,
        param_map: &std::collections::HashMap<String, String>,
        input_schema: &serde_json::Value,
        _effective_params: &serde_json::Value,
    ) -> crate::types::definition::ToolDefinition {
        let description = task_output_description(renderer, description_override);
        let remapped_schema = if param_map.is_empty() {
            input_schema.clone()
        } else {
            crate::util::remap::remap_schema_properties(input_schema, param_map)
        };
        crate::types::definition::ToolDefinition::function(
            client_name,
            Some(&description),
            remapped_schema,
        )
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        task_output_requires_expr()
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Resolve the model-facing `get_task_output` description from the finalized toolset, honoring an explicit config override. Wording lives in
/// the shared [`xai_tool_types::build_task_output_description`] builder so the CLI and prod-chat can't drift; presence-gated clauses (monitor
/// note, subagent source, read-file hint) follow the tools actually registered this turn.
fn task_output_description(
    renderer: &TemplateRenderer,
    description_override: Option<&str>,
) -> String {
    if let Some(ovr) = description_override {
        return renderer.render(ovr).unwrap_or_else(|e| {
            tracing::warn!("get_task_output description override render failed, using raw: {e}");
            ovr.to_string()
        });
    }
    xai_tool_types::build_task_output_description(&xai_tool_types::TaskOutputToolNaming {
        monitor_tool: renderer.tool_for_kind(ToolKind::Monitor),
        read_tool: renderer.tool_for_kind(ToolKind::Read),
        bash_background_param: renderer.param_for_kind(ToolKind::Execute, "is_background"),
        subagent_background_param: renderer.param_for_kind(ToolKind::Task, "run_in_background"),
        task_ids_param: renderer
            .param_for_kind(ToolKind::BackgroundTaskAction, "task_ids")
            .unwrap_or("task_ids"),
        // Same singular id name kill_task uses in its monitor aside.
        task_id_param: renderer
            .param_for_kind(ToolKind::KillTaskAction, "task_id")
            .unwrap_or("task_id"),
    })
}

impl xai_tool_runtime::Tool for TaskOutputTool {
    type Args = TaskOutputToolInput;
    type Output = TaskOutputOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new("get_task_output").expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "get_task_output",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.get_task_output", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: TaskOutputToolInput,
    ) -> Result<TaskOutputOutput, xai_tool_runtime::ToolError> {
        self.run_with_output_byte_limit(ctx, input, None).await
    }
}

impl TaskOutputTool {
    /// `output_byte_limit` is set only by `get_terminal_command_output`. `None`
    /// is `get_task_output`: the shared truncation lookup.
    pub(crate) async fn run_with_output_byte_limit(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: TaskOutputToolInput,
        output_byte_limit: Option<usize>,
    ) -> Result<TaskOutputOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let ids = input.resolved_task_ids();
        if ids.is_empty() {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "Provide a non-empty task_ids list.".to_string(),
            ));
        }
        if ids.len() > MAX_MULTI_WAIT_IDS {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "task_ids exceeds maximum of {MAX_MULTI_WAIT_IDS} entries."
            )));
        }

        if ids.len() == 1 {
            let Some(id) = ids.first() else {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(
                    "Provide a non-empty task_ids list.".to_string(),
                ));
            };
            return self
                .run_single_task(id, &ctx, resources, output_byte_limit)
                .await;
        }

        Self::run_multi_tasks_limited(
            &ids,
            resources,
            "get_command_or_subagent_output",
            output_byte_limit,
        )
        .await
    }
}

/// `output_byte_limit` is `get_terminal_command_output`'s session param. `Some`
/// is that tool's builtin cap, and `TruncationConfig` may override it under
/// `get_terminal_command_output`. `None` keeps `lookup_name`.
fn resolved_max_output_bytes(
    res: &crate::types::resources::Resources,
    lookup_name: &str,
    output_byte_limit: Option<usize>,
) -> usize {
    let (name, builtin) = match output_byte_limit {
        Some(limit) => ("get_terminal_command_output", limit),
        None => (lookup_name, DEFAULT_TOOL_OUTPUT_BYTES),
    };
    res.get::<TruncationCfg>()
        .map(|cfg| cfg.0.max_output_bytes_for(name, builtin))
        .unwrap_or(builtin)
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use crate::computer::types::{
        BackgroundHandle, KillOutcome, TaskSnapshot, TerminalBackend, TerminalRunRequest,
        TerminalRunResult,
    };
    use crate::types::resources::{Resources, Terminal};
    use crate::types::template_renderer::TemplateRenderer;
    use crate::types::tool::ToolKind;

    /// Mock backend that returns a pre-configured snapshot and task listing.
    pub(crate) struct MockTerminal {
        snapshot: Option<TaskSnapshot>,
        tasks: Vec<TaskSnapshot>,
    }

    impl MockTerminal {
        pub(crate) fn with_snapshot(snapshot: TaskSnapshot) -> Self {
            Self {
                tasks: vec![snapshot.clone()],
                snapshot: Some(snapshot),
            }
        }

        pub(crate) fn empty() -> Self {
            Self {
                snapshot: None,
                tasks: Vec::new(),
            }
        }

        pub(crate) fn listing(tasks: Vec<TaskSnapshot>) -> Self {
            Self {
                snapshot: None,
                tasks,
            }
        }
    }

    #[async_trait::async_trait]
    impl TerminalBackend for MockTerminal {
        async fn run(
            &self,
            _request: TerminalRunRequest,
        ) -> Result<TerminalRunResult, crate::computer::types::ComputerError> {
            unimplemented!()
        }

        async fn run_background(
            &self,
            _request: TerminalRunRequest,
        ) -> Result<BackgroundHandle, crate::computer::types::ComputerError> {
            unimplemented!()
        }

        async fn kill_task(&self, _task_id: &str) -> KillOutcome {
            unimplemented!()
        }

        async fn get_task(&self, _task_id: &str) -> Option<TaskSnapshot> {
            self.snapshot.clone()
        }

        async fn wait_for_completion(
            &self,
            _task_id: &str,
            _timeout: Option<Duration>,
        ) -> Option<TaskSnapshot> {
            self.snapshot.clone()
        }

        async fn list_tasks(&self) -> Vec<TaskSnapshot> {
            self.tasks.clone()
        }
    }

    pub(crate) fn make_snapshot(
        task_id: &str,
        completed: bool,
        exit_code: Option<i32>,
    ) -> TaskSnapshot {
        TaskSnapshot {
            task_id: task_id.to_string(),
            command: "echo hello".to_string(),
            display_command: None,
            cwd: "/tmp".to_string(),
            start_time: SystemTime::now(),
            end_time: if completed {
                Some(SystemTime::now())
            } else {
                None
            },
            output: "test output".to_string(),
            output_file: PathBuf::from(format!("/tmp/{}.log", task_id)),
            truncated: false,
            exit_code,
            signal: None,
            completed,
            kind: Default::default(),
            block_waited: false,
            explicitly_killed: false,
            kill_result_delivered: false,
            owner_session_id: None,
            description: None,
            is_backgrounded: false,
            output_total_bytes: 0,
        }
    }

    pub(crate) fn resources_with_terminal(snapshot: Option<TaskSnapshot>) -> Resources {
        let mut resources = Resources::new();
        let backend: Arc<dyn TerminalBackend> = match snapshot {
            Some(s) => Arc::new(MockTerminal::with_snapshot(s)),
            None => Arc::new(MockTerminal::empty()),
        };
        resources.insert(Terminal(backend));
        resources.insert(TemplateRenderer::new(
            std::collections::HashMap::from([(ToolKind::Read, "read_file".to_string())]),
            std::collections::HashMap::new(),
        ));
        resources
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use crate::computer::types::{
        BackgroundHandle, KillOutcome, TaskSnapshot, TerminalBackend, TerminalRunRequest,
        TerminalRunResult,
    };
    use crate::types::resources::Resources;
    use crate::types::tool_metadata::ToolMetadata;
    use crate::types::tool_metadata::test_ctx;
    use std::sync::Arc;

    use std::time::Duration;

    #[test]
    fn still_running_hint_tells_the_model_to_end_its_turn() {
        assert_eq!(
            still_running_hint(WaitSubject::Task),
            "Still running. This tool does not wait. Do other work now. When you have nothing else to do, end your turn: a completion message is delivered when the task finishes. Unless the user specified, do not kill this task."
        );
        assert_eq!(
            still_running_hint(WaitSubject::Subagent),
            "Still running. This tool does not wait. Do other work now. When you have nothing else to do, end your turn: a completion message is delivered when the subagent finishes. Unless the user specified, do not kill this subagent and do not tell it to stop."
        );
    }

    /// Drives the shipped reading path, not the pure helpers: a fake `lpi` on
    /// `PATH` prints its own notice and a snapshot. The reading surfaces the
    /// last snapshot line.
    #[tokio::test]
    #[cfg(unix)]
    async fn lpi_reading_surfaces_the_snapshot_from_a_task_log() {
        use std::io::Write;

        const SNAPSHOT: &str = r#"{"progress":0.6012861728668213,"units_done":2,"units_total":3,"units_pct":66.66666666666666,"has_times":true,"elapsed_seconds":0.00018,"elapsed_known":true,"eta_kind":"none","pace":0,"match_rate":1,"confidence":"high","current_lines":2,"matched_lines":2,"novel_lines":0,"overflow_lines":0}"#;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lpi");
        let mut file = std::fs::File::create(&bin).unwrap();
        write!(
            file,
            "#!/bin/sh\nprintf '%s\\n' 'no model for key x yet'\nprintf '%s\\n' '{SNAPSHOT}'\n"
        )
        .unwrap();
        drop(file);
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // PATH is process-global, so one lock serializes the readers here.
        static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("PATH");
        let mut entries = vec![dir.path().to_path_buf()];
        if let Some(saved) = &saved {
            entries.extend(std::env::split_paths(saved));
        }
        unsafe { std::env::set_var("PATH", std::env::join_paths(entries).unwrap()) };

        let log = dir.path().join("task.log");
        std::fs::write(&log, "compiling a\ncompiling b\n").unwrap();
        let mut snapshot = make_snapshot("task-lpi", false, None);
        snapshot.output_file = log;

        let reading = TaskOutputTool::lpi_reading(&snapshot).await;

        unsafe {
            match saved {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }

        assert_eq!(
            reading.as_deref(),
            Some("progress 60% (2/3 units), confidence high")
        );
    }

    #[test]
    fn tool_name_and_description() {
        let tool = TaskOutputTool;
        assert_eq!(
            xai_tool_runtime::Tool::id(&tool).as_str(),
            "get_task_output"
        );
        // The static fallback is the shared builder's default grok-build
        // rendering (monitor + task + bash + read present): concrete names, no
        // leftover template markers.
        let desc = ToolMetadata::description_template(&tool);
        assert!(desc.contains("monitor"));
        assert!(
            desc.contains("read_file"),
            "fallback names the read tool: {desc}"
        );
        assert!(
            !desc.contains("${"),
            "fallback must not leak template markers: {desc}"
        );
    }

    /// The context-aware `versioned_definition` path only mentions a tool when
    /// it's actually in the finalized toolset. Renders every relevant subset
    /// through the shared builder via `task_output_description`.
    #[test]
    fn description_context_aware_for_tool_subsets() {
        use crate::types::template_renderer::TemplateRenderer;
        use std::collections::HashMap;

        let cases: &[(&str, &[(ToolKind, &str)])] = &[
            (
                "execute only",
                &[(ToolKind::Execute, "run_terminal_command")],
            ),
            (
                "execute + read",
                &[
                    (ToolKind::Execute, "run_terminal_command"),
                    (ToolKind::Read, "read_file"),
                ],
            ),
            (
                "execute + task",
                &[
                    (ToolKind::Execute, "run_terminal_command"),
                    (ToolKind::Task, "spawn_subagent"),
                ],
            ),
            (
                "task + monitor + read (no bash)",
                &[
                    (ToolKind::Task, "spawn_subagent"),
                    (ToolKind::Monitor, "monitor"),
                    (ToolKind::Read, "read_file"),
                ],
            ),
        ];

        for (label, kinds) in cases {
            let tools: HashMap<ToolKind, String> =
                kinds.iter().map(|(k, n)| (*k, n.to_string())).collect();
            // Seed the param map for present tools the way `finalize` does, so
            // the background sources / subagent header resolve via
            // `param_for_kind` (which reads the param map, not the tool map).
            let mut params: HashMap<ToolKind, HashMap<String, String>> = HashMap::new();
            for (k, _) in kinds.iter() {
                match k {
                    ToolKind::Execute => {
                        params
                            .entry(ToolKind::Execute)
                            .or_default()
                            .insert("is_background".to_string(), "is_background".to_string());
                    }
                    ToolKind::Task => {
                        params.entry(ToolKind::Task).or_default().insert(
                            "run_in_background".to_string(),
                            "run_in_background".to_string(),
                        );
                    }
                    _ => {}
                }
            }
            let renderer = TemplateRenderer::new(tools, params);
            let rendered = task_output_description(&renderer, None);

            let has_monitor = kinds.iter().any(|(k, _)| *k == ToolKind::Monitor);
            let has_task = kinds.iter().any(|(k, _)| *k == ToolKind::Task);
            let has_read = kinds.iter().any(|(k, _)| *k == ToolKind::Read);

            assert!(
                !rendered.contains("${"),
                "[{label}] left an unrendered template marker:\n{rendered}"
            );
            assert_eq!(
                rendered.contains("monitor"),
                has_monitor,
                "[{label}] monitor mention must match monitor-tool presence:\n{rendered}"
            );
            assert_eq!(
                rendered.contains("subagent"),
                has_task,
                "[{label}] subagent mention must match task-tool presence:\n{rendered}"
            );
            assert_eq!(
                rendered.contains("read_file"),
                has_read,
                "[{label}] read_file mention must match read-tool presence:\n{rendered}"
            );
            assert_eq!(
                rendered.contains("output_file"),
                has_read,
                "[{label}] read-file hint must match read-tool presence:\n{rendered}"
            );
        }
    }

    #[test]
    fn description_tracks_renamed_task_ids() {
        use crate::types::template_renderer::TemplateRenderer;
        use std::collections::HashMap;

        let tools = HashMap::from([
            (ToolKind::Execute, "run_terminal_command".to_string()),
            (ToolKind::Monitor, "monitor".to_string()),
            (
                ToolKind::BackgroundTaskAction,
                "get_task_output".to_string(),
            ),
            (ToolKind::KillTaskAction, "kill_task".to_string()),
        ]);
        let params = HashMap::from([
            (
                ToolKind::Execute,
                HashMap::from([("is_background".to_string(), "is_background".to_string())]),
            ),
            (
                ToolKind::BackgroundTaskAction,
                HashMap::from([("task_ids".to_string(), "process_ids".to_string())]),
            ),
            (
                ToolKind::KillTaskAction,
                HashMap::from([("task_id".to_string(), "id".to_string())]),
            ),
        ]);
        let rendered = task_output_description(&TemplateRenderer::new(tools, params), None);
        assert!(
            rendered.contains("process_ids"),
            "renamed task_ids must appear:\n{rendered}"
        );
        assert!(
            rendered.contains("monitor"),
            "monitor tool name must appear:\n{rendered}"
        );
        assert!(
            !rendered.contains("task_ids")
                && !rendered.contains("timeout_ms")
                && !rendered.contains("task_id"),
            "canonical param names must not remain after rename:\n{rendered}"
        );
        assert!(
            !rendered.contains("${"),
            "must not leak template markers:\n{rendered}"
        );
    }

    #[tokio::test]
    async fn get_task_running() {
        let snapshot = make_snapshot("task-1", false, None);
        let resources = resources_with_terminal(Some(snapshot));
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-1".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "task-1");
                assert_eq!(r.status, "running");
                assert!(r.exit_code.is_none());
                assert!(r.ended.is_none());
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_completed_success() {
        let snapshot = make_snapshot("task-2", true, Some(0));
        let resources = resources_with_terminal(Some(snapshot));
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-2".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.status, "completed");
                assert_eq!(r.exit_code, Some(0));
                assert!(r.ended.is_some());
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_completed_failed() {
        let snapshot = make_snapshot("task-3", true, Some(1));
        let resources = resources_with_terminal(Some(snapshot));
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-3".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.status, "failed");
                assert_eq!(r.exit_code, Some(1));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_not_found_no_tasks_exist() {
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-x".into()],
            },
        )
        .await
        .unwrap(); // Should be Ok, not Err

        match result {
            TaskOutputOutput::TaskNotFound(msg) => {
                assert!(msg.contains("not found"), "msg: {msg}");
                assert!(
                    msg.contains("No background tasks or subagents exist"),
                    "msg: {msg}"
                );
            }
            other => panic!("Expected TaskNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_not_found_lists_known_tasks() {
        // Mock that returns None for get_task but has known tasks in list_tasks.
        struct MockTerminalWithKnown;

        #[async_trait::async_trait]
        impl TerminalBackend for MockTerminalWithKnown {
            async fn run(
                &self,
                _request: TerminalRunRequest,
            ) -> Result<TerminalRunResult, crate::computer::types::ComputerError> {
                unimplemented!()
            }

            async fn run_background(
                &self,
                _request: TerminalRunRequest,
            ) -> Result<BackgroundHandle, crate::computer::types::ComputerError> {
                unimplemented!()
            }

            async fn kill_task(&self, _task_id: &str) -> KillOutcome {
                unimplemented!()
            }

            async fn get_task(&self, _task_id: &str) -> Option<TaskSnapshot> {
                None // requested task not found
            }

            async fn wait_for_completion(
                &self,
                _task_id: &str,
                _timeout: Option<Duration>,
            ) -> Option<TaskSnapshot> {
                None
            }

            async fn list_tasks(&self) -> Vec<TaskSnapshot> {
                vec![
                    make_snapshot("task-abc", false, None),
                    make_snapshot("task-def", true, Some(0)),
                ]
            }
        }

        let mut resources = Resources::new();
        let backend: Arc<dyn TerminalBackend> = Arc::new(MockTerminalWithKnown);
        resources.insert(Terminal(backend));
        resources.insert(TemplateRenderer::new(
            std::collections::HashMap::from([(ToolKind::Read, "read_file".to_string())]),
            std::collections::HashMap::new(),
        ));

        let tool = TaskOutputTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-unknown".into()],
            },
        )
        .await
        .unwrap(); // Should be Ok, not Err

        match result {
            TaskOutputOutput::TaskNotFound(msg) => {
                assert!(msg.contains("task-unknown"), "msg: {msg}");
                assert!(msg.contains("Known task IDs"), "msg: {msg}");
                assert!(msg.contains("task-abc"), "msg: {msg}");
                assert!(msg.contains("task-def"), "msg: {msg}");
            }
            other => panic!("Expected TaskNotFound, got {:?}", other),
        }
    }

    /// Not-found hint for `"task-unknown"` as seen by session `"me"` when the
    /// shared terminal lists `tasks`.
    async fn owner_scoped_not_found_message(tasks: &[(&str, Option<&str>)]) -> String {
        let tasks = tasks
            .iter()
            .map(|(id, owner)| TaskSnapshot {
                owner_session_id: owner.map(str::to_owned),
                ..make_snapshot(id, false, None)
            })
            .collect();
        let mut resources = Resources::new();
        let backend: Arc<dyn TerminalBackend> = Arc::new(MockTerminal::listing(tasks));
        resources.insert(Terminal(backend));
        resources.insert(crate::types::resources::OwnerSessionId("me".into()));
        resources.insert(TemplateRenderer::new(
            std::collections::HashMap::from([(ToolKind::Read, "read_file".to_string())]),
            std::collections::HashMap::new(),
        ));
        let result = xai_tool_runtime::Tool::run(
            &TaskOutputTool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-unknown".into()],
            },
        )
        .await
        .unwrap();
        match result {
            TaskOutputOutput::TaskNotFound(msg) => msg,
            other => panic!("Expected TaskNotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_task_not_found_lists_only_this_sessions_known_tasks() {
        let msg = owner_scoped_not_found_message(&[
            ("task-mine", Some("me")),
            ("task-theirs", Some("other")),
            ("task-legacy", None),
        ])
        .await;
        assert!(msg.contains("task-mine"), "msg: {msg}");
        assert!(msg.contains("task-legacy"), "msg: {msg}");
        assert!(!msg.contains("task-theirs"), "msg: {msg}");
    }

    #[tokio::test]
    async fn get_task_not_found_with_only_foreign_tasks_reports_empty_session() {
        let msg =
            owner_scoped_not_found_message(&[("task-a", Some("other")), ("task-b", Some("other"))])
                .await;
        assert!(
            msg.contains("No background tasks or subagents exist in this session."),
            "msg: {msg}"
        );
        assert!(!msg.contains("Known task IDs"), "msg: {msg}");
        assert!(
            !msg.contains("task-a") && !msg.contains("task-b"),
            "msg: {msg}"
        );
    }

    #[tokio::test]
    async fn errors_when_terminal_not_in_resources() {
        let resources = Resources::new();
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-x".into()],
            },
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .to_lowercase()
                .contains("terminal")
        );
    }

    #[tokio::test]
    async fn uses_tool_name_mapping_for_truncation_hint() {
        let mut snapshot = make_snapshot("task-4", true, Some(0));
        snapshot.output = "x".repeat(500_000); // large output triggers truncation

        let mut resources = resources_with_terminal(Some(snapshot));
        // Set a custom model-facing name for the Read tool
        resources.insert(TemplateRenderer::new(
            [(ToolKind::Read, "Read".to_string())].into(),
            Default::default(),
        ));

        let tool = TaskOutputTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-4".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(r.truncated);
                assert!(
                    r.output.contains("Read"),
                    "truncation hint should use 'Read'"
                );
                assert!(r.truncation_hint.contains("Read"));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    /// A terminal whose `wait_for_completion` records the call and, for a live
    /// task, sleeps for the whole requested timeout.
    struct StampWaitTerminal {
        snapshot: TaskSnapshot,
        waited: std::sync::Arc<std::sync::atomic::AtomicBool>,
        stamped_block_waited: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl TerminalBackend for StampWaitTerminal {
        async fn run(
            &self,
            _request: TerminalRunRequest,
        ) -> Result<TerminalRunResult, crate::computer::types::ComputerError> {
            unimplemented!()
        }

        async fn run_background(
            &self,
            _request: TerminalRunRequest,
        ) -> Result<BackgroundHandle, crate::computer::types::ComputerError> {
            unimplemented!()
        }

        async fn kill_task(&self, _task_id: &str) -> KillOutcome {
            unimplemented!()
        }

        async fn get_task(&self, _task_id: &str) -> Option<TaskSnapshot> {
            Some(self.snapshot.clone())
        }

        async fn wait_for_completion(
            &self,
            _task_id: &str,
            timeout: Option<Duration>,
        ) -> Option<TaskSnapshot> {
            self.waited.store(true, std::sync::atomic::Ordering::SeqCst);
            if self.snapshot.completed {
                self.stamped_block_waited
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let mut snapshot = self.snapshot.clone();
                snapshot.block_waited = true;
                return Some(snapshot);
            }
            tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(600))).await;
            Some(self.snapshot.clone())
        }

        async fn list_tasks(&self) -> Vec<TaskSnapshot> {
            vec![self.snapshot.clone()]
        }
    }

    fn resources_with_stamp_wait(
        snapshot: TaskSnapshot,
    ) -> (
        Resources,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let waited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stamped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut resources = Resources::new();
        let backend: Arc<dyn TerminalBackend> = Arc::new(StampWaitTerminal {
            snapshot,
            waited: waited.clone(),
            stamped_block_waited: stamped.clone(),
        });
        resources.insert(Terminal(backend));
        resources.insert(TemplateRenderer::new(
            std::collections::HashMap::from([(ToolKind::Read, "read_file".to_string())]),
            std::collections::HashMap::new(),
        ));
        (resources, waited, stamped)
    }

    /// A call that still sends a large `timeout_ms` returns a snapshot at once:
    /// the key is ignored, and the tool never calls `wait_for_completion`.
    #[tokio::test]
    async fn get_on_still_running_task_returns_at_once_with_the_hint() {
        let snapshot = make_snapshot("task-run", false, None);
        let (resources, waited, stamped) = resources_with_stamp_wait(snapshot);
        let input: TaskOutputToolInput = serde_json::from_value(serde_json::json!({
            "task_ids": ["task-run"],
            "timeout_ms": 600_000
        }))
        .unwrap();
        let started = std::time::Instant::now();
        let result =
            xai_tool_runtime::Tool::run(&TaskOutputTool, test_ctx(resources.into_shared()), input)
                .await
                .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a running task must not hold the call; elapsed {:?}",
            started.elapsed()
        );
        assert!(
            !waited.load(std::sync::atomic::Ordering::SeqCst),
            "get_task_output must not call wait_for_completion"
        );
        assert!(!stamped.load(std::sync::atomic::Ordering::SeqCst));
        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.status, "running");
                assert!(
                    r.output
                        .ends_with(&format!("\n\n{}", still_running_hint(WaitSubject::Task))),
                    "a running result must end with the hint: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multi_get_on_already_completed_task_does_not_wait() {
        let snapshot = make_snapshot("task-done", true, Some(0));
        let (resources, waited, _) = resources_with_stamp_wait(snapshot);
        let result = TaskOutputTool::run_multi_tasks_limited(
            &["task-done".into()],
            resources.into_shared(),
            "get_command_or_subagent_output",
            None,
        )
        .await
        .unwrap();
        assert!(
            !waited.load(std::sync::atomic::Ordering::SeqCst),
            "a multi-id get must not wait_for_completion"
        );
        match result {
            TaskOutputOutput::MultiResult(m) => {
                assert_eq!(m.mode, "poll");
                let [first] = m.results.as_slice() else {
                    panic!("expected exactly one result, got {}", m.results.len());
                };
                assert_eq!(first.status, "completed");
            }
            other => panic!("Expected MultiResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multi_get_on_still_running_task_returns_at_once() {
        let snapshot = make_snapshot("task-run", false, None);
        let (resources, waited, _) = resources_with_stamp_wait(snapshot);
        let started = std::time::Instant::now();
        let result = TaskOutputTool::run_multi_tasks_limited(
            &["task-run".into()],
            resources.into_shared(),
            "get_command_or_subagent_output",
            None,
        )
        .await
        .unwrap();
        assert!(
            !waited.load(std::sync::atomic::Ordering::SeqCst),
            "a multi-id get must not wait_for_completion on a running task"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a running task must not hold the call; elapsed {:?}",
            started.elapsed()
        );
        match result {
            TaskOutputOutput::MultiResult(m) => {
                assert_eq!(m.mode, "poll");
                let Some(first) = m.results.first() else {
                    panic!("expected one result: {:?}", m.results);
                };
                assert_eq!(first.status, "running");
                assert!(
                    first
                        .output
                        .ends_with(still_running_hint(WaitSubject::Task)),
                    "{}",
                    first.output
                );
            }
            other => panic!("Expected MultiResult, got {other:?}"),
        }
    }

    #[test]
    fn is_read_only_returns_true() {
        let tool = TaskOutputTool;
        assert!(
            ToolMetadata::is_read_only(&tool),
            "get_task_output must be classified as read-only"
        );
    }

    #[tokio::test]
    async fn respects_truncation_config() {
        let mut snapshot = make_snapshot("task-6", true, Some(0));
        snapshot.output = "x".repeat(10_000); // 10KB

        let mut resources = resources_with_terminal(Some(snapshot));

        // Set a custom truncation config with 5KB limit
        let mut trunc = crate::types::context::TruncationConfig::default();
        trunc
            .per_tool_max_output_bytes
            .insert("get_command_or_subagent_output".to_string(), 5_000);
        resources.insert(TruncationCfg(trunc));

        let tool = TaskOutputTool;
        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-6".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert!(
                    r.truncated,
                    "10KB output should be truncated with 5KB limit"
                );
                assert!(r.output.contains("[Output truncated"));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
    }

    // Legacy message parity fixture tests These tests verify exact historical wording for legacy-0.4.10. Fixture source: the historical 0.4.10
    // task_output implementation. Historical 0.4.10 message (inner string from ToolError::ProcessManagerError): "Task {task_id} not found"
    // Subagent wording is out of scope — subagents didn't exist in 0.4.10.

    #[tokio::test]
    async fn legacy_get_task_not_found_exact_historical_message() {
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;

        let mut ctx = test_ctx(resources.into_shared());
        ctx.extensions.insert(xai_tool_runtime::BehaviorVersion(
            "legacy-0.4.10".to_string(),
        ));

        let result = xai_tool_runtime::Tool::run(
            &tool,
            ctx,
            TaskOutputToolInput {
                task_ids: vec!["task-xyz".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::TaskNotFound(msg) => {
                // Exact historical fixture — no trailing period.
                assert_eq!(msg, "Task task-xyz not found");
            }
            other => panic!("Expected TaskNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn current_get_task_not_found_includes_discoverability() {
        // Current (non-legacy) path must still include known task IDs
        // or "No background tasks" text for discoverability.
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["task-xyz".into()],
            },
        )
        .await
        .unwrap();

        match result {
            TaskOutputOutput::TaskNotFound(msg) => {
                assert!(
                    msg.contains("No background tasks or subagents exist"),
                    "Current path must include discoverability text, got: {msg}"
                );
            }
            other => panic!("Expected TaskNotFound, got {:?}", other),
        }
    }

    // ── Subagent running snapshot formatting ─────────────────────────────

    #[test]
    fn format_initializing_subagent_reports_status() {
        let snap = SubagentSnapshot {
            subagent_id: "sub-init".to_string(),
            description: "Check PR status".to_string(),
            subagent_type: "general-purpose".to_string(),
            persona: None,
            status: SubagentSnapshotStatus::Initializing,
            started_at_epoch_ms: 1_700_000_000_000,
            duration_ms: 8_500,
        };
        let result = format_subagent_snapshot(&snap);
        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "sub-init");
                assert_eq!(r.status, "initializing");
                assert!(r.exit_code.is_none());
                assert!(r.ended.is_none());
                assert!(
                    r.output.contains("initializing"),
                    "output should mention initializing: {}",
                    r.output
                );
                assert!(
                    r.output.contains("8.5s"),
                    "output should contain elapsed time: {}",
                    r.output
                );
                assert!(
                    r.output
                        .ends_with(still_running_hint(WaitSubject::Subagent)),
                    "output should end with the hint: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn format_running_subagent_includes_progress_fields() {
        let snap = SubagentSnapshot {
            subagent_id: "sub-abc".to_string(),
            description: "Find all API endpoints".to_string(),
            subagent_type: "explore".to_string(),
            persona: None,
            status: SubagentSnapshotStatus::Running {
                turn_count: 3,
                tool_call_count: 12,
                tokens_used: 45_000,
                context_window_tokens: 128_000,
                context_usage_pct: 35,
                tools_used: vec![
                    "bash".to_string(),
                    "read_file".to_string(),
                    "grep".to_string(),
                ],
                error_count: 0,
            },
            started_at_epoch_ms: 1_700_000_000_000,
            duration_ms: 12_500,
        };
        let result = format_subagent_snapshot(&snap);
        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "sub-abc");
                assert_eq!(r.status, "running");
                assert!(r.exit_code.is_none());
                assert!(r.ended.is_none());
                // Progress line
                assert!(
                    r.output.contains("turn 3"),
                    "should contain turn count: {}",
                    r.output
                );
                assert!(
                    r.output.contains("12 tool calls"),
                    "should contain tool call count: {}",
                    r.output
                );
                assert!(
                    r.output.contains("45K/128K tokens"),
                    "should contain token usage: {}",
                    r.output
                );
                assert!(
                    r.output.contains("35% context"),
                    "should contain context pct: {}",
                    r.output
                );
                // Tools used
                assert!(
                    r.output.contains("bash, read_file, grep"),
                    "should contain tools list: {}",
                    r.output
                );
                // Errors
                assert!(
                    r.output.contains("Errors: 0"),
                    "should contain error count: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    // raw_output_bytes counts the body only, not the hint after it.
    #[test]
    fn format_running_subagent_raw_output_bytes_count_the_body_only() {
        let snap = SubagentSnapshot {
            subagent_id: "sub-stable".to_string(),
            description: "stable body".to_string(),
            subagent_type: "explore".to_string(),
            persona: None,
            status: SubagentSnapshotStatus::Running {
                turn_count: 1,
                tool_call_count: 2,
                tokens_used: 3_000,
                context_window_tokens: 128_000,
                context_usage_pct: 2,
                tools_used: vec!["bash".to_string()],
                error_count: 0,
            },
            started_at_epoch_ms: 1_700_000_000_000,
            duration_ms: 1_000,
        };
        let expected_body = format!(
            "Subagent is still running.\n\
             Type: {}\n\
             Description: {}\n\
             Elapsed: {:.1}s\n\
             Progress: turn 1, 2 tool calls, \
             3K/128K tokens (2% context)\n\
             Tools used: bash\n\
             Errors: 0",
            snap.subagent_type,
            snap.description,
            snap.duration_ms as f64 / 1000.0,
        );
        let result = match format_subagent_snapshot(&snap) {
            TaskOutputOutput::Result(r) => r,
            other => panic!("Expected Result, got {:?}", other),
        };
        assert_eq!(result.raw_output_bytes, expected_body.len());
        assert_eq!(
            result.output,
            format!(
                "{expected_body}\n\n{}",
                still_running_hint(WaitSubject::Subagent)
            )
        );
    }

    #[test]
    fn format_running_subagent_with_no_tools_shows_none_yet() {
        let snap = SubagentSnapshot {
            subagent_id: "sub-new".to_string(),
            description: "just started".to_string(),
            subagent_type: "general-purpose".to_string(),
            persona: None,
            status: SubagentSnapshotStatus::Running {
                turn_count: 0,
                tool_call_count: 0,
                tokens_used: 0,
                context_window_tokens: 128_000,
                context_usage_pct: 0,
                tools_used: vec![],
                error_count: 0,
            },
            started_at_epoch_ms: 1_700_000_000_000,
            duration_ms: 500,
        };
        let result = format_subagent_snapshot(&snap);
        match result {
            TaskOutputOutput::Result(r) => {
                assert!(
                    r.output.contains("none yet"),
                    "empty tools should show 'none yet': {}",
                    r.output
                );
                assert!(
                    r.output.contains("turn 0"),
                    "should show turn 0: {}",
                    r.output
                );
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn empty_task_id_fails_validation() {
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;

        let result = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec![String::new()],
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("task_id")
                || err_msg.contains("task_ids")
                || err_msg.contains("empty"),
            "expected validation error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn multi_task_ids_poll_returns_multi_result_mode_poll() {
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;
        let out = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["missing-a".into(), "missing-b".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        match out {
            TaskOutputOutput::MultiResult(m) => {
                assert_eq!(m.mode, "poll");
                assert_eq!(m.results.len(), 2);
                assert!(m.results.iter().all(|r| r.status == "not_found"));
            }
            other => panic!("expected MultiResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn one_element_task_ids_returns_single_result_not_multi() {
        let resources = resources_with_terminal(None);
        let tool = TaskOutputTool;
        let out = xai_tool_runtime::Tool::run(
            &tool,
            test_ctx(resources.into_shared()),
            TaskOutputToolInput {
                task_ids: vec!["only-one".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        match out {
            TaskOutputOutput::TaskNotFound(_) | TaskOutputOutput::Result(_) => {}
            TaskOutputOutput::MultiResult(_) => {
                panic!("single resolved id must not use MultiResult envelope")
            }
        }
    }

    // ── Subagent backend query fallback tests ────────────────────────────

    /// Build resources with a terminal that returns None (task not found)
    /// and a SubagentBackendResource backed by the unified event channel.
    fn resources_with_backend_query() -> (
        Resources,
        tokio::sync::mpsc::UnboundedReceiver<
            crate::implementations::grok_build::task::types::SubagentEvent,
        >,
    ) {
        use crate::implementations::grok_build::task::backend::{
            ChannelBackend, SubagentBackendResource,
        };
        use crate::implementations::grok_build::task::types::SubagentEvent;
        use std::sync::Arc as StdArc;

        let mut resources = resources_with_terminal(None);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
        resources.insert(SubagentBackendResource(StdArc::new(ChannelBackend::new(
            tx,
        ))));
        (resources, rx)
    }

    /// Extract a `SubagentQueryRequest` from a `SubagentEvent`, panicking on wrong variant.
    fn unwrap_query(
        event: crate::implementations::grok_build::task::types::SubagentEvent,
    ) -> crate::implementations::grok_build::task::types::SubagentQueryRequest {
        match event {
            crate::implementations::grok_build::task::types::SubagentEvent::Query(q) => q,
            _ => panic!("Expected SubagentEvent::Query"),
        }
    }

    #[tokio::test]
    async fn get_task_subagent_completed() {
        let (resources, mut query_rx) = resources_with_backend_query();
        let shared = resources.into_shared();

        let handle = xai_spawn::spawn(async move {
            let req = unwrap_query(query_rx.recv().await.unwrap());
            assert_eq!(req.subagent_id, "sub-done");
            req.respond_to
                .send(Some(SubagentSnapshot {
                    subagent_id: "sub-done".to_string(),
                    description: "find files".to_string(),
                    subagent_type: "explore".to_string(),
                    status: SubagentSnapshotStatus::Completed {
                        output: "Found 3 files".to_string(),
                        tool_calls: 5,
                        turns: 2,
                        worktree_path: None,
                    },
                    started_at_epoch_ms: 1_700_000_000_000,
                    duration_ms: 1500,
                    persona: None,
                }))
                .unwrap();
        });

        let result = xai_tool_runtime::Tool::run(
            &TaskOutputTool,
            test_ctx(shared),
            TaskOutputToolInput {
                task_ids: vec!["sub-done".into()],
            },
        )
        .await
        .unwrap();

        handle.await.unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "sub-done");
                assert_eq!(r.status, "completed");
                assert!(r.output.contains("Found 3 files"), "output: {}", r.output);
            }
            other => panic!("Expected Result(completed), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_subagent_running() {
        let (resources, mut query_rx) = resources_with_backend_query();
        let shared = resources.into_shared();

        let handle = xai_spawn::spawn(async move {
            let req = unwrap_query(query_rx.recv().await.unwrap());
            assert!(!req.block, "get_task_output must never block on a subagent");
            assert_eq!(req.timeout_ms, None);
            req.respond_to
                .send(Some(SubagentSnapshot {
                    subagent_id: "sub-run".to_string(),
                    description: "exploring".to_string(),
                    subagent_type: "general-purpose".to_string(),
                    status: SubagentSnapshotStatus::Running {
                        turn_count: 2,
                        tool_call_count: 5,
                        tokens_used: 10_000,
                        context_window_tokens: 128_000,
                        context_usage_pct: 8,
                        tools_used: vec!["grep".to_string()],
                        error_count: 0,
                    },
                    started_at_epoch_ms: 1_700_000_000_000,
                    duration_ms: 3000,
                    persona: None,
                }))
                .unwrap();
        });

        let result = xai_tool_runtime::Tool::run(
            &TaskOutputTool,
            test_ctx(shared),
            TaskOutputToolInput {
                task_ids: vec!["sub-run".into()],
            },
        )
        .await
        .unwrap();

        handle.await.unwrap();

        match result {
            TaskOutputOutput::Result(r) => {
                assert_eq!(r.task_id, "sub-run");
                assert_eq!(r.status, "running");
                assert!(r.output.contains("still running"), "output: {}", r.output);
                assert!(
                    r.output
                        .ends_with(still_running_hint(WaitSubject::Subagent)),
                    "output: {}",
                    r.output
                );
            }
            other => panic!("Expected Result(running), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_task_subagent_not_found_falls_through() {
        let (resources, mut query_rx) = resources_with_backend_query();
        let shared = resources.into_shared();

        let handle = xai_spawn::spawn(async move {
            let req = unwrap_query(query_rx.recv().await.unwrap());
            req.respond_to.send(None).unwrap();
        });

        let result = xai_tool_runtime::Tool::run(
            &TaskOutputTool,
            test_ctx(shared),
            TaskOutputToolInput {
                task_ids: vec!["sub-nope".into()],
            },
        )
        .await
        .unwrap();

        handle.await.unwrap();

        match result {
            TaskOutputOutput::TaskNotFound(msg) => {
                assert!(msg.contains("not found"), "msg: {msg}");
            }
            other => panic!("Expected TaskNotFound, got {:?}", other),
        }
    }

    fn completed_subagent_snapshot(id: &str) -> SubagentSnapshot {
        SubagentSnapshot {
            subagent_id: id.to_string(),
            description: "find files".to_string(),
            subagent_type: "explore".to_string(),
            status: SubagentSnapshotStatus::Completed {
                output: "Found 3 files".to_string(),
                tool_calls: 5,
                turns: 2,
                worktree_path: None,
            },
            started_at_epoch_ms: 1_700_000_000_000,
            duration_ms: 1500,
            persona: None,
        }
    }

    #[tokio::test]
    async fn get_on_cancelled_and_failed_subagent_reports_the_terminal_status() {
        for (id, snapshot, expected) in [
            (
                "sub-cancel",
                SubagentSnapshot {
                    subagent_id: "sub-cancel".to_string(),
                    description: "cancelled".to_string(),
                    subagent_type: "explore".to_string(),
                    status: SubagentSnapshotStatus::Cancelled {
                        reason: Some("stop".to_string()),
                    },
                    started_at_epoch_ms: 1_700_000_000_000,
                    duration_ms: 10,
                    persona: None,
                },
                "cancelled",
            ),
            (
                "sub-fail",
                SubagentSnapshot {
                    subagent_id: "sub-fail".to_string(),
                    description: "failed".to_string(),
                    subagent_type: "explore".to_string(),
                    status: SubagentSnapshotStatus::Failed {
                        error: "boom".to_string(),
                    },
                    started_at_epoch_ms: 1_700_000_000_000,
                    duration_ms: 10,
                    persona: None,
                },
                "failed",
            ),
        ] {
            let (resources, mut query_rx) = resources_with_backend_query();
            let shared = resources.into_shared();
            let handle = xai_spawn::spawn(async move {
                let req = unwrap_query(query_rx.recv().await.unwrap());
                assert!(!req.block);
                req.respond_to.send(Some(snapshot)).unwrap();
            });
            let result = xai_tool_runtime::Tool::run(
                &TaskOutputTool,
                test_ctx(shared),
                TaskOutputToolInput {
                    task_ids: vec![id.to_string()],
                },
            )
            .await
            .unwrap();
            handle.await.unwrap();
            match result {
                TaskOutputOutput::Result(r) => assert_eq!(r.status, expected),
                other => panic!("Expected Result({expected}), got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn multi_get_on_completed_subagent_issues_one_snapshot_query() {
        let (resources, mut query_rx) = resources_with_backend_query();
        let shared = resources.into_shared();
        let handle = xai_spawn::spawn(async move {
            let req = unwrap_query(query_rx.recv().await.unwrap());
            assert!(!req.block, "a multi-id get must snapshot, not block");
            req.respond_to
                .send(Some(completed_subagent_snapshot("sub-done")))
                .unwrap();
            if query_rx.recv().await.is_some() {
                panic!("a multi-id get must not issue a second query");
            }
        });
        let result = TaskOutputTool::run_multi_tasks_limited(
            &["sub-done".into()],
            shared,
            "get_command_or_subagent_output",
            None,
        )
        .await
        .unwrap();
        handle.await.unwrap();
        match result {
            TaskOutputOutput::MultiResult(m) => {
                let [first] = m.results.as_slice() else {
                    panic!("expected exactly one result, got {}", m.results.len());
                };
                assert_eq!(first.status, "completed");
            }
            other => panic!("Expected MultiResult, got {other:?}"),
        }
    }
}
