//! The human-readable title of a tool call, from its parsed input alone.
//!
//! One function serves both the finished call and the one the model is still
//! writing. A streaming call re-reads its half-written arguments on each
//! fragment (`partial_json`), so a second copy of this match would let the row
//! rename itself the moment the call completed.
use super::tool_calls::{ci_tool_title, execute_tool_call_parts};
use super::*;
/// Past this many bytes of arguments a call stops being re-read for a title.
///
/// What names a call sits at the head of its arguments: a path, a command, a
/// query. The rest is the body being written — a file's contents, a patch — and
/// re-parsing megabytes of it on every fragment buys the same title back.
pub(crate) const STREAMING_TITLE_ARG_CAP: usize = 4096;
/// What the model has written of one tool call so far.
///
/// Held per `tool_index` for the life of a stream. The name arrives once, on
/// the opening fragment, and the arguments accumulate under it.
pub(crate) struct StreamingToolArgs {
    /// The wire name the opening fragment carried.
    pub(crate) name: String,
    /// Every argument byte seen for this call.
    pub(crate) args: String,
    /// The last title this call resolved to. Only a change is worth sending.
    pub(crate) title: Option<String>,
}
impl StreamingToolArgs {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            args: String::new(),
            title: None,
        }
    }
    /// Whether the bytes received are still worth re-reading for a title.
    ///
    /// Every fragment is re-read while under the cap, including a one-character
    /// one. A cheaper rule that waited for a few bytes would skip the LAST
    /// fragment of a call, which is usually two characters and is exactly the
    /// one that completes the argument the title is read from.
    pub(crate) fn wants_parse(&self) -> bool {
        self.args.len() <= STREAMING_TITLE_ARG_CAP
    }
}
/// What the UI calls this tool call.
///
/// Pure: `cwd` is only read to shorten a command for display. Every arm takes
/// what has arrived. A field the model has not written yet is absent from the
/// parsed input, and the arm falls back the same way it does for a call that
/// omits it.
pub(crate) fn tool_input_title(input: &ToolInput, cwd: &std::path::Path) -> String {
    match input {
        ToolInput::ListDir(list_dir) => format!("List `{}`", list_dir.target_directory),
        ToolInput::SearchReplace(sr) => format!("Edit `{}`", sr.file_path.as_str()),
        ToolInput::Bash(bash_tool) => {
            execute_tool_call_parts(
                &bash_tool.command,
                Some(bash_tool.description.as_str()),
                cwd,
            )
            .0
        }
        ToolInput::ReadFile(read_file) => format!("Read `{}`", read_file.path),
        ToolInput::TodoWrite(_) => "Updating plan".to_string(),
        ToolInput::Grep(gs) => gs.pattern.clone(),
        ToolInput::WebSearch(ws) => format!("Web search: \"{}\"", ws.query),
        ToolInput::ImageGen(ig) => format!("imagine: {}", ig.prompt),
        ToolInput::ImageEdit(ie) => format!("imagine-edit: {}", ie.prompt),
        ToolInput::ImageToVideo(i2v) => format!(
            "image-to-video: {}",
            i2v.prompt.as_deref().unwrap_or(&i2v.image)
        ),
        ToolInput::ReferenceToVideo(r2v) => format!("reference-to-video: {}", r2v.prompt),
        ToolInput::MCPTool(mcp_tool) => mcp_tool.tool_name.to_owned(),
        ToolInput::TaskOutput(task_output) => {
            let ids = task_output.resolved_task_ids();
            match ids.as_slice() {
                [] => "Get task output".to_string(),
                [one] => format!("Get task output: {one}"),
                many => format!("Get task output: {} tasks", many.len()),
            }
        }
        ToolInput::WaitTasks(wait) => format!(
            "Wait tasks: {} ids, mode={}",
            wait.task_ids.len(),
            match wait.mode {
                xai_tool_types::WaitMode::WaitAny => "wait_any",
                xai_tool_types::WaitMode::WaitAll => "wait_all",
            }
        ),
        ToolInput::KillTask(kill_task) => format!("Kill task: {}", kill_task.task_id),
        ToolInput::Skill(skill) => format!("Skill: {}", skill.skill),
        ToolInput::ApplyPatch(_) => "Apply patch".to_string(),
        ToolInput::Dynamic(_) => "Dynamic tool call".to_string(),
        ToolInput::MemorySearch(ms) => {
            let end = ms
                .query
                .char_indices()
                .nth(60)
                .map_or(ms.query.len(), |(i, _)| i);
            format!("Memory search: \"{}\"", &ms.query[..end])
        }
        ToolInput::MemoryGet(mg) => format!("Memory read: {}", mg.path),
        ToolInput::HashlineEdit(he) => format!("Edit `{}`", he.file_path),
        ToolInput::Task(task) => task.description.clone(),
        ToolInput::EnterPlanMode(_) => "Plan: Enter".to_string(),
        ToolInput::ExitPlanMode(_) => "Plan: Exit".to_string(),
        ToolInput::AskUserQuestion(ask) => {
            if ask.questions.len() == 1 {
                format!("Ask: {}", ask.questions[0].question)
            } else {
                format!("Ask {} questions", ask.questions.len())
            }
        }
        ToolInput::WebFetch(wf) => format!("Fetch: {}", wf.url),
        ToolInput::SearchTool(st) => format!("Search tools: \"{}\"", st.query),
        ToolInput::UseTool(ut) => ut.tool_name.clone(),
        ToolInput::Write(w) => format!("Write `{}`", w.file_path),
        ToolInput::Workflow(w) => {
            let script_name = |script: &str| -> Option<String> {
                let head = script.get(..600).unwrap_or(script);
                let rest = &head[head.find("name:")? + 5..];
                let rest = &rest[rest.find('"')? + 1..];
                Some(rest[..rest.find('"')?].to_string())
            };
            let inline_name = w.script.as_deref().and_then(script_name);
            if w.validate_only {
                match inline_name.or_else(|| w.name.clone()) {
                    Some(n) => format!("Validating workflow '{n}'"),
                    None => "Validating workflow script".to_string(),
                }
            } else if w.script.is_some() {
                match inline_name {
                    Some(n) => format!("Creating workflow '{n}'"),
                    None => "Creating workflow".to_string(),
                }
            } else if let Some(ref name) = w.name {
                format!("Workflow: {name}")
            } else if w.resume_from_run_id.is_some() {
                "Workflow: resume run".to_string()
            } else {
                "Workflow: launch script".to_string()
            }
        }
        ToolInput::UpdateGoal(ug) => {
            if ug.completed == Some(true) {
                "Goal: marking complete".to_string()
            } else if let Some(ref reason) = ug.blocked_reason {
                format!("Goal: blocked — {reason}")
            } else if let Some(ref msg) = ug.message {
                format!("Goal: {msg}")
            } else {
                "Goal: update".to_string()
            }
        }
        ToolInput::Monitor(m) => format!("Start monitor: {}", m.description),
        ToolInput::SchedulerCreate(sc) => match (&sc.task_id, &sc.interval) {
            (Some(id), Some(interval)) => format!("Update scheduled task {id} (every {interval})"),
            (Some(id), None) => format!("Update scheduled task {id}"),
            (None, Some(interval)) => format!("Create scheduled task (every {interval})"),
            (None, None) => "Create scheduled task".to_string(),
        },
        ToolInput::SchedulerDelete(sd) => format!("Delete scheduled task: {}", sd.id),
        ToolInput::SchedulerList(_) => "List scheduled tasks".to_string(),
        // The CI tool has its own row: without this arm it fell into the
        // generic `_` below and every CI query read as "Tool call", which
        // says nothing about what was asked of CI.
        ToolInput::Ci(ci) => ci_tool_title(ci),
        #[allow(unreachable_patterns)]
        _ => "Tool call".to_string(),
    }
}
