//! `/todo` capture: a bounded side agent that turns one request into items on
//! the session's todo list.
//!
//! Rides the same non-interrupting path as `/btw` (`SessionCommand`, spawned
//! on the session's LocalSet, parent-cached request skeleton from
//! [`super::side_call`]), and differs in the two ways that matter: it runs a
//! short tool loop instead of a single call, and it is allowed exactly one
//! mutation — appending to the todo list.
//!
//! see AGENTS.md, "`/todo` capture feature notes"

use super::side_call::{AuxCall, log_prompt_cache_hit};
use super::*;

use xai_grok_sampling_types::ToolCall;
use xai_grok_tools::types::tool::ToolKind;

/// Model calls the capture agent gets: enough to look at a few things and then
/// write, not enough to turn a note into an investigation.
const MAX_MODEL_CALLS: usize = 4;
/// Read-only tool calls it gets across the whole run.
const MAX_TOOL_CALLS: usize = 8;
/// Bytes of a read-only tool's output it sees. Its job is to name the work,
/// not to read a file into a todo item.
const TOOL_RESULT_BUDGET: usize = 4_000;

/// The only task-list tool `/todo` can append through — see
/// [`TodoCaptureError::UnsupportedTodoTool`].
const TODO_WRITE: &str = "todo_write";

/// What a `/todo` run put on the list.
#[derive(Debug, Clone)]
pub struct TodoCaptureOutcome {
    /// Contents of the appended items, in the order they were added.
    pub added: Vec<String>,
    /// Read-only tool calls the capture agent spent getting there.
    pub tools_used: usize,
}

/// Failure surface of a `/todo` capture run. Typed to the ACP boundary so
/// model errors keep the canonical mapping (rate limits, auth) instead of a
/// flattened string, same as [`SideQuestionError`](crate::session::SideQuestionError).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TodoCaptureError {
    #[error("todo capture model call failed: {0}")]
    Sampling(#[from] SamplingError),
    #[error("failed to prepare client: {0}")]
    PrepareClient(String),
    #[error("/todo needs the `todo_write` tool; this session's task-list tool is `{0}`")]
    UnsupportedTodoTool(String),
    #[error("the capture agent finished without adding a todo")]
    NothingAdded,
    #[error("adding the todo failed: {0}")]
    TodoWriteFailed(String),
}

/// Tool kinds the capture agent may actually run. Narrower than the main
/// turn's read-only set (`prepare_tool_call`): `EnterPlan`/`ExitPlan` change
/// session mode and `AskUser` blocks on a human, neither of which a side agent
/// nobody is watching may do.
fn is_capture_readable(kind: ToolKind) -> bool {
    matches!(
        kind,
        ToolKind::Read
            | ToolKind::Search
            | ToolKind::Lsp
            | ToolKind::ListDir
            | ToolKind::List
            | ToolKind::MemorySearch
            | ToolKind::MemoryGet
            | ToolKind::WebSearch
            | ToolKind::WebFetch
    )
}

/// Item contents from a `todo_write` call, in order. Everything else the model
/// asked for is dropped: ids, statuses, and `merge`.
///
/// This is the gate that holds the one-mutation rule. The capture agent is
/// handed the session's real todo tool (so the request stays byte-identical to
/// the main turn's and keeps its prompt cache), which means a `merge: false`
/// replace, a status flip, or a reworded existing item are all one argument
/// away — until they come through here.
fn contents_from_todo_write_args(args: &serde_json::Value) -> Vec<String> {
    args.get("todos")
        .and_then(serde_json::Value::as_array)
        .map(|todos| {
            todos
                .iter()
                .filter_map(|t| {
                    // `content` is optional in the tool's schema and falls back
                    // to the id there too.
                    let text = t
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| t.get("id").and_then(serde_json::Value::as_str))?;
                    Some(text.trim().to_owned())
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Append-only `todo_write` arguments for `contents`. Ids are minted here (the
/// `capture-` prefix marks provenance on the list and cannot collide with an
/// id the main agent is using), status is pending, and `merge` is on — a merge
/// of ids that exist nowhere in the state can only append.
fn add_only_todo_args(contents: &[String]) -> serde_json::Value {
    let todos: Vec<serde_json::Value> = contents
        .iter()
        .map(|content| {
            serde_json::json!({
                "id": format!("capture-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]),
                "content": content,
                "status": "pending",
            })
        })
        .collect();
    serde_json::json!({ "merge": true, "todos": todos })
}

/// What the capture loop does with one tool call the model emitted.
#[derive(Debug, PartialEq, Eq)]
enum CaptureAction {
    /// Run it for real and feed the output back.
    Read,
    /// Sanitize it into an append and run it.
    Append,
    /// Never run it; feed this sentence back instead.
    Refuse(String),
}

/// Classify one tool call. This is the enforcement: a call that is neither the
/// todo tool nor a readable one never reaches dispatch. The refusals are
/// written to steer the next turn toward the write, because a refusal the model
/// cannot act on just burns the turn budget.
fn capture_action(name: &str, kind: Option<ToolKind>, tools_used: usize) -> CaptureAction {
    // The append is always available: it is the one thing the run exists to do,
    // so a spent read budget must not strand the agent with nothing to call.
    if name == TODO_WRITE {
        return CaptureAction::Append;
    }
    if !kind.is_some_and(is_capture_readable) {
        return CaptureAction::Refuse(format!(
            "`{name}` was not run: adding todo items is the todo-capture agent's only \
             permitted mutation, and read-only tools are the only others it may call. \
             Put the work in a todo item with `{TODO_WRITE}` instead."
        ));
    }
    if tools_used >= MAX_TOOL_CALLS {
        return CaptureAction::Refuse(format!(
            "Read-only budget spent ({MAX_TOOL_CALLS} calls). Call `{TODO_WRITE}` now \
             with what you have."
        ));
    }
    CaptureAction::Read
}

impl SessionActor {
    /// Run a `/todo` capture: a few read-only tool calls to name the work, then
    /// one append to the todo list. Never touches the parent conversation, and
    /// never interrupts the running turn.
    pub(super) async fn handle_todo_capture(
        &self,
        request: &str,
    ) -> Result<TodoCaptureOutcome, TodoCaptureError> {
        let bridge = self.agent.borrow().tool_bridge().clone();
        // Other harnesses' task-list tools replace the whole list rather than
        // merging into it (opencode's `todowrite` has no `merge`), so an
        // append cannot be expressed through them. Say so instead of writing
        // through a tool whose semantics are wrong for this.
        match bridge.tool_for_kind(ToolKind::Plan).await {
            Some(name) if name == TODO_WRITE => {}
            Some(name) => return Err(TodoCaptureError::UnsupportedTodoTool(name)),
            None => return Err(TodoCaptureError::UnsupportedTodoTool("none".into())),
        }

        let sampling_client = self
            .prepare_chat_completion(false)
            .await
            .map_err(|e| TodoCaptureError::PrepareClient(e.to_string()))?;

        let conversation = self.chat_state_handle.get_conversation().await;
        let mut items: Vec<ConversationItem> =
            if sampling_client.api_backend().requires_reasoning_strip() {
                xai_chat_state::compaction_utils::strip_reasoning_blocks(conversation)
            } else {
                conversation
            };
        // `/todo` fires mid-turn, so the snapshot may end with an assistant
        // message whose tool_calls have no matching ToolResult yet.
        crate::session::helpers::session_recap::pop_trailing_tool_run(&mut items);
        items.push(self.todo_capture_instruction(request));

        let sampling_config = self.chat_state_handle.get_sampling_config().await;
        let reasoning_effort = sampling_config.as_ref().and_then(|c| c.reasoning_effort);
        let model = sampling_config.map(|c| c.model).unwrap_or_default();
        // Same tools as the main turn: they serialize into the cached prefix,
        // so trimming the list to the ones the loop honors would cost the whole
        // conversation's prompt cache to save nothing. What the agent may
        // actually run is decided at dispatch, in `capture_action`.
        let tool_specs = self.turn_base_tool_specs(&self.prepare_tool_definitions().await);
        let hosted_tools = self.hosted_tools_for_turn();
        let conv_id = format!("todo-{}", uuid::Uuid::new_v4());

        let mut added: Vec<String> = Vec::new();
        let mut tools_used = 0usize;

        for _ in 0..MAX_MODEL_CALLS {
            let model_request = self.parent_cached_request(AuxCall {
                items: items.clone(),
                tools: tool_specs.clone(),
                hosted_tools: hosted_tools.clone(),
                model: model.clone(),
                reasoning_effort,
                backend: sampling_client.api_backend(),
                conv_id: conv_id.clone(),
                req_id: format!("xai-todo-{}", uuid::Uuid::new_v4()),
            });
            let response = sampling_client.conversation_collect(model_request).await?;
            log_prompt_cache_hit("todo", sampling_client.api_backend(), &response);

            let calls: Vec<ToolCall> = response.tool_calls().to_vec();
            if calls.is_empty() {
                break;
            }
            items.push(ConversationItem::assistant_tool_calls(calls.clone()));
            for call in &calls {
                let (result_text, appended, ran_read) =
                    self.run_capture_tool(call, tools_used).await;
                if ran_read {
                    tools_used += 1;
                }
                added.extend(appended);
                items.push(ConversationItem::tool_result(
                    call.id.to_string(),
                    result_text,
                ));
            }
            // The append is the end of the job. Finish the batch that produced
            // it (a split across two calls in one batch is still one write),
            // then stop rather than paying for a turn that can only chat.
            if !added.is_empty() {
                break;
            }
        }

        if added.is_empty() {
            return Err(TodoCaptureError::NothingAdded);
        }
        tracing::info!(
            added = added.len(),
            tools_used,
            "todo capture appended items"
        );
        Ok(TodoCaptureOutcome { added, tools_used })
    }

    /// The capture agent's instruction turn.
    fn todo_capture_instruction(&self, request: &str) -> ConversationItem {
        let tag = self.reminder_wrapper_tag();
        ConversationItem::user(format!(
            "<{tag}>You are a todo-capture agent: a separate, lightweight agent \
             spawned to turn one request from the user into items on the shared \
             todo list.\n\n\
             CONTEXT:\n\
             - The main agent is NOT interrupted; it keeps working while you run\n\
             - You share its conversation context but are a separate instance\n\
             - Do NOT reference being interrupted or what you were \"previously doing\"\n\n\
             YOUR JOB, IN ORDER:\n\
             1. Spend a few READ-ONLY tool calls (read, search, list) if and only if \
             you need them to name the work concretely. Skip this entirely when the \
             request is already specific.\n\
             2. Call `{TODO_WRITE}` with the item(s) to add, then stop.\n\n\
             CONSTRAINTS, enforced outside this prompt — a call that breaks one is \
             refused and never runs, so do not spend a turn on it:\n\
             - Adding todo items is your ONLY permitted mutation. Every edit, write, \
             shell command, and subagent call is refused.\n\
             - Your `{TODO_WRITE}` call APPENDS: the ids you send are replaced with \
             fresh ones, statuses are forced to pending, and existing items are \
             untouched. You cannot complete, reword, reorder, or drop the main \
             agent's work.\n\
             - You get at most {MAX_MODEL_CALLS} turns and {MAX_TOOL_CALLS} \
             read-only tool calls.\n\n\
             Write items the main agent can act on later without you: state the \
             outcome, and name the file, symbol, or command when you have one. One \
             item per separable piece of work — do not inflate a single ask into a \
             checklist.\n\n\
             The user's request follows.</{tag}>\n\n\
             {request}"
        ))
    }

    /// Dispatch one tool call from the capture loop.
    ///
    /// Returns the text the model sees, the contents of anything appended, and
    /// whether a read-only call was spent.
    async fn run_capture_tool(
        &self,
        call: &ToolCall,
        tools_used: usize,
    ) -> (String, Vec<String>, bool) {
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
        let kind = self.agent.borrow().tool_bridge().tool_kind(&call.name);
        match capture_action(&call.name, kind, tools_used) {
            CaptureAction::Refuse(message) => (message, Vec::new(), false),
            CaptureAction::Append => {
                let contents = contents_from_todo_write_args(&args);
                if contents.is_empty() {
                    return (
                        format!(
                            "No item to add: the `{TODO_WRITE}` call carried no todo content. \
                             Send `todos: [{{\"content\": \"...\"}}]`."
                        ),
                        Vec::new(),
                        false,
                    );
                }
                match self
                    .append_capture_todos(&call.id, add_only_todo_args(&contents))
                    .await
                {
                    Ok(text) => (text, contents, false),
                    Err(e) => {
                        tracing::warn!(error = %e, "todo capture: append failed");
                        (format!("Adding the todo failed: {e}"), Vec::new(), false)
                    }
                }
            }
            CaptureAction::Read => {
                match self
                    .workspace_ops
                    .call_tool(&call.name, args, &call.id, Some(&self.session_info.id.0))
                    .await
                {
                    Ok(result) => (
                        xai_grok_tools::util::truncate_str(&result.prompt_text, TOOL_RESULT_BUDGET)
                            .to_owned(),
                        Vec::new(),
                        true,
                    ),
                    Err(e) => (format!("`{}` failed: {e}", call.name), Vec::new(), true),
                }
            }
        }
    }

    /// Run the sanitized append through the session's own todo tool, so the
    /// list, its persisted state, and the client's plan view all move the way
    /// they do when the main agent writes a todo.
    async fn append_capture_todos(
        &self,
        call_id: &str,
        args: serde_json::Value,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        let result = self
            .workspace_ops
            .call_tool(TODO_WRITE, args, call_id, Some(&self.session_info.id.0))
            .await?;
        if let Some(plan) = crate::session::acp_conversion::acp_plan_update(&result.output) {
            self.send_update(acp::SessionUpdate::Plan(plan), None).await;
        }
        Ok(result.prompt_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_only_args_mint_ids_and_force_pending_merge() {
        let args = add_only_todo_args(&["wire the exporter".to_owned()]);
        assert_eq!(args["merge"], serde_json::json!(true));
        let todo = &args["todos"][0];
        assert_eq!(todo["content"], serde_json::json!("wire the exporter"));
        assert_eq!(todo["status"], serde_json::json!("pending"));
        let id = todo["id"].as_str().unwrap();
        assert!(
            id.starts_with("capture-"),
            "{id} must be marked as captured"
        );
        // Two calls with the same content must not collide on the list.
        let other = add_only_todo_args(&["wire the exporter".to_owned()]);
        assert_ne!(id, other["todos"][0]["id"].as_str().unwrap());
    }

    /// The model's own ids, statuses and `merge: false` are the three ways a
    /// `todo_write` call could touch existing items. All three are dropped:
    /// only the content survives into [`add_only_todo_args`].
    #[test]
    fn a_replace_call_over_existing_items_survives_as_content_only() {
        let contents = contents_from_todo_write_args(&serde_json::json!({
            "merge": false,
            "todos": [
                {"id": "1", "content": "ship the release", "status": "completed"},
                {"id": "2", "status": "cancelled"},
            ],
        }));
        assert_eq!(contents, vec!["ship the release", "2"]);
        let args = add_only_todo_args(&contents);
        assert_eq!(args["merge"], serde_json::json!(true));
        for todo in args["todos"].as_array().unwrap() {
            assert_eq!(todo["status"], serde_json::json!("pending"));
            assert!(todo["id"].as_str().unwrap().starts_with("capture-"));
        }
    }

    #[test]
    fn blank_and_missing_content_do_not_become_items() {
        assert!(
            contents_from_todo_write_args(&serde_json::json!({"todos": [{"content": "   "}]}))
                .is_empty()
        );
        assert!(contents_from_todo_write_args(&serde_json::json!({"todos": []})).is_empty());
        assert!(contents_from_todo_write_args(&serde_json::json!({})).is_empty());
        assert!(contents_from_todo_write_args(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn the_append_is_reachable_even_with_the_read_budget_spent() {
        assert_eq!(
            capture_action(TODO_WRITE, Some(ToolKind::Plan), MAX_TOOL_CALLS + 3),
            CaptureAction::Append
        );
    }

    #[test]
    fn a_mutating_or_unknown_tool_never_reaches_dispatch() {
        for (name, kind) in [
            ("search_replace", Some(ToolKind::Edit)),
            ("bash", Some(ToolKind::Execute)),
            ("spawn_subagent", Some(ToolKind::Task)),
            ("exit_plan_mode", Some(ToolKind::ExitPlan)),
            ("ask_user_question", Some(ToolKind::AskUser)),
            // A name the bridge cannot classify (a hallucinated tool, or one
            // this session does not have) fails closed.
            ("definitely_not_a_tool", None),
        ] {
            match capture_action(name, kind, 0) {
                CaptureAction::Refuse(message) => {
                    assert!(
                        message.contains(name),
                        "refusal must name the tool: {message}"
                    );
                    assert!(
                        message.contains(TODO_WRITE),
                        "refusal must point at the one thing it may do: {message}"
                    );
                }
                other => panic!("{name} must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn reads_run_until_the_budget_is_spent_then_are_refused() {
        assert_eq!(
            capture_action("read_file", Some(ToolKind::Read), MAX_TOOL_CALLS - 1),
            CaptureAction::Read
        );
        assert!(matches!(
            capture_action("read_file", Some(ToolKind::Read), MAX_TOOL_CALLS),
            CaptureAction::Refuse(_)
        ));
    }

    #[test]
    fn only_read_kinds_are_runnable() {
        for kind in [
            ToolKind::Read,
            ToolKind::Search,
            ToolKind::Lsp,
            ToolKind::ListDir,
            ToolKind::List,
            ToolKind::MemorySearch,
            ToolKind::MemoryGet,
            ToolKind::WebSearch,
            ToolKind::WebFetch,
        ] {
            assert!(is_capture_readable(kind), "{kind:?} must be runnable");
        }
        // Mutations, and the two the main turn counts as read-only but a
        // side agent must not have: plan-mode switches and asking a human.
        for kind in [
            ToolKind::Edit,
            ToolKind::Write,
            ToolKind::Delete,
            ToolKind::Move,
            ToolKind::Execute,
            ToolKind::Task,
            ToolKind::Monitor,
            ToolKind::Workflow,
            ToolKind::GoalUpdate,
            ToolKind::Plan,
            ToolKind::EnterPlan,
            ToolKind::ExitPlan,
            ToolKind::AskUser,
        ] {
            assert!(!is_capture_readable(kind), "{kind:?} must be refused");
        }
    }
}
