//! The human-readable title of a tool call, from its parsed input alone.
//!
//! One function serves both the finished call and the one the model is still
//! writing. A streaming call re-reads its half-written arguments on each
//! fragment (`partial_json`), so a second copy of this match would let the row
//! rename itself the moment the call completed.
use super::tool_calls::{ci_tool_title, execute_tool_call_parts};
use super::*;
use xai_grok_tools::types::tool::ToolKind;
/// How much of a call's arguments is kept to read a title out of.
///
/// What names a call sits at the head of its arguments: a path, a command, a
/// query. The rest is the body being written — a file's contents, a patch. Once
/// the head is full the tail is DROPPED, not just left unparsed: a session that
/// kept it would hold a second copy of every file the model writes, for as long
/// as the write takes.
pub(crate) const STREAMING_TITLE_ARG_CAP: usize = 4096;
/// The head of what the model has written of one tool call.
///
/// Held per `tool_index` for the life of a stream. The name arrives once, on
/// the opening fragment, and the arguments accumulate under it up to the cap.
pub(crate) struct StreamingToolArgs {
    /// The wire name the opening fragment carried.
    pub(crate) name: String,
    /// The head of the arguments, bounded by [`STREAMING_TITLE_ARG_CAP`].
    pub(crate) args: String,
    /// Set once a fragment has been cut. What is held is then a prefix of a
    /// longer document, and completing a prefix that stops mid-body can name
    /// the call something the whole document would not.
    pub(crate) capped: bool,
    /// The last title this call resolved to. Only a change is worth sending.
    pub(crate) title: Option<String>,
}
impl StreamingToolArgs {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            args: String::new(),
            capped: false,
            title: None,
        }
    }
    /// Take one argument fragment, keeping only what fits under the cap.
    pub(crate) fn push(&mut self, delta: &str) {
        let room = STREAMING_TITLE_ARG_CAP.saturating_sub(self.args.len());
        if room == 0 {
            // Sticky: a call that has lost bytes never becomes whole again, so
            // a later empty fragment must not read as "nothing was cut".
            self.capped |= !delta.is_empty();
            return;
        }
        // A fragment can split a multi-byte character, so cut on a boundary.
        let mut end = delta.len().min(room);
        while end > 0 && !delta.is_char_boundary(end) {
            end -= 1;
        }
        self.args.push_str(&delta[..end]);
        self.capped |= end < delta.len();
    }
    /// Whether the bytes held are still worth re-reading for a title.
    ///
    /// Every fragment is re-read until the head fills, including a
    /// one-character one. A cheaper rule that waited for a few bytes would skip
    /// the LAST fragment of a call, which is usually two characters and is
    /// exactly the one that completes the argument the title is read from.
    pub(crate) fn wants_parse(&self) -> bool {
        !self.capped
    }
}
/// What the UI calls this tool call.
///
/// Pure: `cwd` is only read to shorten a command for display. Every arm takes
/// what has arrived. A field the model has not written yet is absent from the
/// parsed input, and the arm falls back the same way it does for a call that
/// omits it.
///
/// The match has no catch-all arm. A new `ToolInput` variant then fails to
/// compile here until it gets a title, instead of reading as a generic label.
/// `kind` is the registry's kind for `wire_name`. It names the tools whose
/// input arrives as `Dynamic`, and it survives a `name_override`.
pub(crate) fn tool_input_title(
    input: &ToolInput,
    wire_name: &str,
    kind: Option<ToolKind>,
    cwd: &std::path::Path,
) -> String {
    // The pager shows an empty title as the ACP kind, which is "Other" for
    // most tools. The wire name at least says which tool ran.
    let title = input_title(input, wire_name, kind, cwd);
    if title.is_empty() {
        wire_name.to_string()
    } else {
        title
    }
}
fn input_title(
    input: &ToolInput,
    wire_name: &str,
    kind: Option<ToolKind>,
    cwd: &std::path::Path,
) -> String {
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
            match &wait.mode {
                xai_tool_types::WaitMode::WaitAny => "wait_any",
                xai_tool_types::WaitMode::WaitAll => "wait_all",
            }
        ),
        ToolInput::KillTask(kill_task) => format!("Kill task: {}", kill_task.task_id),
        ToolInput::Skill(skill) => format!("Skill: {}", skill.skill),
        ToolInput::ApplyPatch(_) => "Apply patch".to_string(),
        ToolInput::Dynamic(_)
            if wire_name
                == xai_grok_tools::implementations::grok_build::SEND_FEEDBACK_TOOL_NAME =>
        {
            "Feedback drafted".to_string()
        }
        ToolInput::Dynamic(args) => dynamic_tool_title(wire_name, kind, args, cwd),
        ToolInput::MemorySearch(ms) => {
            let end = ms
                .query
                .char_indices()
                .nth(60)
                .map_or(ms.query.len(), |(i, _)| i);
            format!(
                "Memory search: \"{}\"",
                ms.query.get(..end).unwrap_or(ms.query.as_str())
            )
        }
        ToolInput::MemoryGet(mg) => format!("Memory read: {}", mg.path),
        ToolInput::HashlineEdit(he) => format!("Edit `{}`", he.file_path),
        ToolInput::Task(task) => task.description.clone(),
        ToolInput::EnterPlanMode(_) => "Plan: Enter".to_string(),
        ToolInput::ExitPlanMode(_) => "Plan: Exit".to_string(),
        ToolInput::AskUserQuestion(ask) => match ask.questions.as_slice() {
            [q] => format!("Ask: {}", q.question),
            qs => format!("Ask {} questions", qs.len()),
        },
        ToolInput::WebFetch(wf) => format!("Fetch: {}", wf.url),
        ToolInput::SearchTool(st) => format!("Search tools: \"{}\"", st.query),
        ToolInput::UseTool(ut) => ut
            .target_name()
            .map(str::to_owned)
            .unwrap_or_else(|| "Read MCP invocation source".to_owned()),
        ToolInput::Write(w) => format!("Write `{}`", w.file_path),
        ToolInput::Workflow(w) => {
            use xai_grok_tools::implementations::grok_build::workflow::WorkflowSource;
            let script_name = |script: &str| -> Option<String> {
                let head = script.get(..600).unwrap_or(script);
                let rest = head.get(head.find("name:")? + 5..)?;
                let rest = rest.get(rest.find('"')? + 1..)?;
                Some(rest.get(..rest.find('"')?)?.to_string())
            };
            let inline_name = match &w.source {
                WorkflowSource::Script { script } => script_name(script),
                _ => None,
            };
            if w.validate_only {
                let source_name = match &w.source {
                    WorkflowSource::Name { name } => Some(name.clone()),
                    _ => inline_name,
                };
                match source_name {
                    Some(name) => format!("Validating workflow '{name}'"),
                    None => "Validating workflow script".to_string(),
                }
            } else {
                match &w.source {
                    WorkflowSource::Script { .. } => match inline_name {
                        Some(name) => format!("Creating workflow '{name}'"),
                        None => "Creating workflow".to_string(),
                    },
                    WorkflowSource::Name { name } => format!("Workflow: {name}"),
                    WorkflowSource::Resume { .. } => "Workflow: resume run".to_string(),
                    WorkflowSource::Pause { .. } => "Workflow: pause run".to_string(),
                    WorkflowSource::Stop { .. } => "Workflow: stop run".to_string(),
                    WorkflowSource::ScriptPath { .. } => "Workflow: launch script".to_string(),
                }
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
        ToolInput::Ci(ci) => ci_tool_title(ci),
        ToolInput::CopyMove(cm) => {
            // `copy_file` and `move_file` share one input and one kind.
            let verb = if wire_name.contains("copy") {
                "Copy"
            } else {
                "Move"
            };
            format!("{verb} `{}` → `{}`", cm.source, cm.destination)
        }
        ToolInput::CodexListDir(ld) => format!("List `{}`", ld.dir_path),
        ToolInput::CodexGrepFiles(gf) => gf.pattern.clone(),
        ToolInput::CodexReadFile(rf) => format!("Read `{}`", rf.file_path),
        ToolInput::Lsp(lsp) => lsp_tool_title(lsp),
        ToolInput::SendMessage(sm) => format!("Message {}", sm.to),
        ToolInput::SendFeedback(_) => "Feedback drafted".to_string(),
        ToolInput::SendSubagentMessage(message) => {
            super::active_agent_message_presentation::active_agent_message_tool_call_display(
                message,
            )
            .0
        }
    }
}
fn lsp_tool_title(lsp: &xai_grok_tools::implementations::lsp::LspToolInput) -> String {
    use xai_grok_tools::implementations::lsp::LspOperation;
    let op = match lsp.operation {
        LspOperation::GoToDefinition => "Go to definition",
        LspOperation::FindReferences => "Find references",
        LspOperation::Hover => "Hover",
        LspOperation::GoToImplementation => "Go to implementation",
        LspOperation::DocumentSymbol => "Document symbols",
        LspOperation::WorkspaceSymbol => "Workspace symbols",
    };
    if let Some(query) = lsp.query.as_deref().filter(|q| !q.is_empty()) {
        return format!("{op}: \"{query}\"");
    }
    match (lsp.file_path.as_deref(), lsp.line) {
        // The input line is 0-indexed. An editor shows it 1-indexed.
        (Some(path), Some(line)) => format!("{op}: `{path}:{}`", line + 1),
        (Some(path), None) => format!("{op}: `{path}`"),
        (None, _) => op.to_string(),
    }
}
/// The title of a tool whose input the bridge hands over as raw JSON.
///
/// The opencode harness registers its built-ins this way. So the title comes
/// from the tool's kind and the argument that kind names. A tool the kinds do
/// not cover shows its wire name, which is the name the model called it by.
fn dynamic_tool_title(
    wire_name: &str,
    kind: Option<ToolKind>,
    args: &serde_json::Value,
    cwd: &std::path::Path,
) -> String {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args.get(*k).and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
    };
    let path = field(&["filePath", "file_path", "path"]);
    let titled = match kind {
        Some(ToolKind::Execute) => field(&["command"])
            .map(|cmd| execute_tool_call_parts(cmd, field(&["description"]), cwd).0),
        Some(ToolKind::Read) => path.map(|p| format!("Read `{p}`")),
        Some(ToolKind::Edit) => path.map(|p| format!("Edit `{p}`")),
        Some(ToolKind::Write) => path.map(|p| format!("Write `{p}`")),
        Some(ToolKind::Search) => field(&["pattern"]).map(str::to_string),
        Some(ToolKind::List) => field(&["pattern"]).map(|p| format!("Find `{p}`")),
        Some(ToolKind::Skill) => field(&["name", "skill"]).map(|s| format!("Skill: {s}")),
        Some(ToolKind::Plan) => Some("Updating plan".to_string()),
        _ => None,
    };
    titled.unwrap_or_else(|| wire_name.to_string())
}
#[cfg(test)]
mod title_tests {
    use super::{ToolInput, ToolKind, tool_input_title};
    use serde_json::json;
    use std::path::Path;
    /// Build an input the way the wire does. That keeps the test off each
    /// input struct's module path and its private defaults.
    fn input(variant: &str, fields: serde_json::Value) -> ToolInput {
        let mut obj = fields.as_object().cloned().unwrap_or_default();
        obj.insert("variant".into(), json!(variant));
        serde_json::from_value(serde_json::Value::Object(obj))
            .unwrap_or_else(|e| panic!("{variant} did not parse: {e}"))
    }
    fn title(input: &ToolInput, wire: &str, kind: Option<ToolKind>) -> String {
        tool_input_title(input, wire, kind, Path::new("/proj"))
    }
    #[test]
    fn every_builtin_that_used_to_fall_through_has_its_own_title() {
        let cases = [
            (
                input("CopyMove", json!({"source": "a.rs", "destination": "b.rs"})),
                "copy_file",
                "Copy `a.rs` → `b.rs`",
            ),
            (
                input("CopyMove", json!({"source": "a.rs", "destination": "b.rs"})),
                "move_file",
                "Move `a.rs` → `b.rs`",
            ),
            (
                input("CodexListDir", json!({"dir_path": "/proj/src"})),
                "list_dir",
                "List `/proj/src`",
            ),
            (
                input("CodexGrepFiles", json!({"pattern": "fn main"})),
                "grep_files",
                "fn main",
            ),
            (
                input("CodexReadFile", json!({"file_path": "/proj/a.rs"})),
                "read_file",
                "Read `/proj/a.rs`",
            ),
            (
                input(
                    "Lsp",
                    json!({"operation": "goToDefinition", "file_path": "/proj/a.rs", "line": 9}),
                ),
                "lsp",
                "Go to definition: `/proj/a.rs:10`",
            ),
            (
                input(
                    "Lsp",
                    json!({"operation": "workspaceSymbol", "query": "Session"}),
                ),
                "lsp",
                "Workspace symbols: \"Session\"",
            ),
            (
                input("SendMessage", json!({"to": "parent", "message": "done"})),
                "send_message",
                "Message parent",
            ),
        ];
        for (input, wire, want) in cases {
            assert_eq!(title(&input, wire, None), want, "wire name {wire}");
        }
    }
    /// The opencode harness hands its built-ins over as raw JSON. Its tools
    /// must read like the grok_build ones, not as one generic label.
    #[test]
    fn a_dynamic_builtin_is_named_by_its_kind_and_argument() {
        let bash_title =
            super::execute_tool_call_parts("cargo test", Some("run tests"), Path::new("/proj")).0;
        let cases = [
            (
                ToolKind::Read,
                "read",
                json!({"filePath": "src/a.rs"}),
                "Read `src/a.rs`",
            ),
            (
                ToolKind::Edit,
                "edit",
                json!({"filePath": "src/a.rs", "oldString": "x", "newString": "y"}),
                "Edit `src/a.rs`",
            ),
            (ToolKind::Search, "grep", json!({"pattern": "TODO"}), "TODO"),
            (
                ToolKind::List,
                "glob",
                json!({"pattern": "**/*.rs"}),
                "Find `**/*.rs`",
            ),
            (
                ToolKind::Skill,
                "skill",
                json!({"name": "deploy"}),
                "Skill: deploy",
            ),
            (
                ToolKind::Plan,
                "todowrite",
                json!({"todos": []}),
                "Updating plan",
            ),
            (
                ToolKind::Execute,
                "bash",
                json!({"command": "cargo test", "description": "run tests"}),
                bash_title.as_str(),
            ),
        ];
        for (kind, wire, args, want) in cases {
            let got = title(&ToolInput::Dynamic(args), wire, Some(kind));
            assert_eq!(got, want, "{wire}");
        }
    }
    /// A tool no kind covers, or a call whose argument has not arrived yet,
    /// shows the name the model called it by.
    #[test]
    fn a_dynamic_tool_without_a_known_argument_shows_its_wire_name() {
        let empty = ToolInput::Dynamic(json!({}));
        assert_eq!(title(&empty, "read", Some(ToolKind::Read)), "read");
        assert_eq!(title(&empty, "frobnicate", None), "frobnicate");
    }
    /// An arm that yields nothing would reach the pager as an empty title,
    /// which it draws as the bare ACP kind.
    #[test]
    fn an_empty_title_falls_back_to_the_wire_name() {
        let grep = input("CodexGrepFiles", json!({"pattern": ""}));
        assert_eq!(title(&grep, "grep_files", None), "grep_files");
    }
}
#[cfg(test)]
mod tests {
    use super::{STREAMING_TITLE_ARG_CAP, StreamingToolArgs};
    /// A write streams the whole file. Holding all of it to name the call would
    /// put a second copy of every file the model writes in this session's
    /// memory, for as long as the write takes.
    #[test]
    fn a_large_body_is_dropped_rather_than_held() {
        let mut call = StreamingToolArgs::new("write_file".to_string());
        call.push("{\"file_path\":\"big.txt\",\"content\":\"");
        for _ in 0..1024 {
            call.push(&"x".repeat(1024));
        }
        assert!(
            call.args.len() <= STREAMING_TITLE_ARG_CAP,
            "the head is bounded, not the whole body: {} bytes held",
            call.args.len()
        );
        assert!(call.capped);
        assert!(
            !call.wants_parse(),
            "a cut prefix must not be re-read: it can name the call something \
             the whole document would not"
        );
    }
    /// The head is what the title comes from, so it has to survive intact.
    #[test]
    fn the_head_is_kept_whole() {
        let mut call = StreamingToolArgs::new("read_file".to_string());
        call.push("{\"target_file\":\"");
        call.push("src/main.rs\"}");
        assert_eq!(call.args, "{\"target_file\":\"src/main.rs\"}");
        assert!(!call.capped);
        assert!(call.wants_parse());
    }
    /// Once a call has lost bytes it never becomes whole again — including
    /// across a later fragment that would have fit, or an empty one.
    #[test]
    fn the_cut_flag_is_sticky() {
        let mut call = StreamingToolArgs::new("write_file".to_string());
        call.push(&"x".repeat(STREAMING_TITLE_ARG_CAP + 1));
        assert!(call.capped);
        call.push("");
        assert!(
            call.capped,
            "an empty fragment cuts nothing and heals nothing"
        );
        call.push("y");
        assert!(call.capped);
    }
    /// A fragment can split a multi-byte character exactly at the cap.
    #[test]
    fn the_cap_never_splits_a_character() {
        let mut call = StreamingToolArgs::new("write_file".to_string());
        call.push(&"a".repeat(STREAMING_TITLE_ARG_CAP - 1));
        call.push("é");
        assert_eq!(
            call.args.len(),
            STREAMING_TITLE_ARG_CAP - 1,
            "a character that does not fit is left out whole"
        );
        assert!(call.capped);
        // The real proof: the held bytes are still a string, not a broken one.
        assert!(call.args.is_char_boundary(call.args.len()));
    }
}
