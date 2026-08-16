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

use super::side_call::{
    AuxCall, aux_retry_policy, fresh_req_id, log_prompt_cache_hit, should_retry_aux_call,
};
use super::*;

use backon::Retryable as _;
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
/// Room the loop's own turns need on top of the conversation snapshot: up to
/// [`MAX_TOOL_CALLS`] results of [`TOOL_RESULT_BUDGET`] bytes each, plus the
/// assistant and reasoning items echoed alongside them. Reserved by shrinking
/// the window the snapshot is fitted to, so on a small-window model the last
/// turn still has somewhere to put the write.
const LOOP_GROWTH_RESERVE_TOKENS: u64 = 16_000;

/// The canonical name of the append-capable task-list tool. What a session
/// advertises it as can differ (a `name_override` renames it per harness), so
/// this is the name in messages and tests, never the one compared against a
/// model's call — see [`SessionActor::resolve_capture_todo_tool`].
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
    #[error(
        "/todo needs an append-capable task-list tool (`todo_write`); this session's is `{0}`, which replaces the list instead"
    )]
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

/// Tool-call arguments as JSON, tolerating what models actually emit.
///
/// Mirrors the main turn's `prepare_tool_call`: empty arguments mean `{}`, and
/// a run of concatenated objects (`{...}{...}`, which several models produce
/// under load) yields the first one rather than nothing. Anything still
/// unparseable becomes `{"raw": ...}` — the same shape the main turn hands a
/// tool, so the failure is the tool's to report, not a silent drop here.
fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    use crate::session::helpers::tool_input_parsing::{
        normalize_empty_arguments, try_extract_concatenated_json_objects,
    };
    let normalized = normalize_empty_arguments(arguments);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(normalized) {
        return value;
    }
    if let Some(objects) = try_extract_concatenated_json_objects(arguments)
        && let Some(first) = objects.into_iter().next()
    {
        return first;
    }
    serde_json::json!({ "raw": arguments })
}

/// Append-only `todo_write` arguments for `contents`. Ids are minted here,
/// status is pending, and `merge` is on — a merge of ids that are not in the
/// state can only append, so the random suffix is what keeps the write off an
/// item the main agent is working through. The `capture-` prefix is provenance,
/// visible on the list and in the model's view of it.
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

/// What one dispatched call did.
struct CaptureToolOutcome {
    /// The tool result text the capture agent sees next turn.
    result_text: String,
    /// Contents that actually landed on the todo list.
    appended: Vec<String>,
    /// Whether this call came out of the read-only budget.
    spent_a_read: bool,
    /// Why the append failed, when one was attempted and refused.
    append_error: Option<String>,
}

impl CaptureToolOutcome {
    /// The common case: the call changed nothing and only produced text.
    fn said(result_text: String) -> Self {
        Self {
            result_text,
            appended: Vec::new(),
            spent_a_read: false,
            append_error: None,
        }
    }
}

/// Classify one tool call. This is the enforcement: a call that is neither the
/// todo tool nor a readable one never reaches dispatch. The refusals are
/// written to steer the next turn toward the write, because a refusal the model
/// cannot act on just burns the turn budget.
///
/// `todo_tool` is the name the todo tool is advertised under in THIS session,
/// which is the name the model calls it by — not the canonical `todo_write`,
/// which a `name_override` can rename out from under both.
fn capture_action(
    name: &str,
    kind: Option<ToolKind>,
    todo_tool: &str,
    tools_used: usize,
) -> CaptureAction {
    // The append is always available: it is the one thing the run exists to do,
    // so a spent read budget must not strand the agent with nothing to call.
    if name == todo_tool {
        return CaptureAction::Append;
    }
    if !kind.is_some_and(is_capture_readable) {
        return CaptureAction::Refuse(format!(
            "`{name}` was not run: adding todo items is the todo-capture agent's only \
             permitted mutation, and read-only tools are the only others it may call. \
             Put the work in a todo item with `{todo_tool}` instead."
        ));
    }
    if tools_used >= MAX_TOOL_CALLS {
        return CaptureAction::Refuse(format!(
            "Read-only budget spent ({MAX_TOOL_CALLS} calls). Call `{todo_tool}` now \
             with what you have."
        ));
    }
    CaptureAction::Read
}

/// The items one model response contributes to the next turn's request: the
/// response echoed the way the main turn records it (`turn.rs` pushes every
/// item, assistant and otherwise), not a synthesized assistant message.
///
/// Reasoning items are what make this worth a function. The Responses API
/// rejects a continuation whose reasoning is missing from the call it belongs
/// to, and hosted-search items have to ride along for the next request to make
/// sense — but the Messages API rejects thinking blocks it was not configured
/// for, which is the one backend that strips.
fn echoed_response_items(
    items: Vec<ConversationItem>,
    strip_reasoning: bool,
) -> Vec<ConversationItem> {
    if strip_reasoning {
        xai_chat_state::compaction_utils::strip_reasoning_blocks(items)
    } else {
        items
    }
}

/// The one nudge a run spends when the model answers with prose instead of
/// calling the todo tool. Cheaper models do this; a second empty answer is
/// taken as "this model will not call it" rather than nudged again.
fn no_tool_call_nudge(tag: &str, todo_tool: &str) -> ConversationItem {
    ConversationItem::user(format!(
        "<{tag}>Nothing was added: a todo only lands on the list through a \
         `{todo_tool}` call, and prose is not one. Call `{todo_tool}` now with the \
         item(s), and reply with nothing else.</{tag}>"
    ))
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
        let todo_tool = self.resolve_capture_todo_tool(&bridge).await?;

        let sampling_client = self
            .prepare_chat_completion(false)
            .await
            .map_err(|e| TodoCaptureError::PrepareClient(e.to_string()))?;

        // Only the Messages backend rejects thinking blocks it was not
        // configured for; every other backend keeps reasoning verbatim, which
        // is what the provider's prefix cache and its own tool-call
        // continuations expect. Applies to the loop's own turns too.
        let strip_reasoning = sampling_client.api_backend().requires_reasoning_strip();
        let sampling_config = self.chat_state_handle.get_sampling_config().await;
        let reasoning_effort = sampling_config.as_ref().and_then(|c| c.reasoning_effort);
        let context_window = sampling_config
            .as_ref()
            .map(|c| c.context_window.get())
            .unwrap_or(crate::remote::DEFAULT_CONTEXT_WINDOW);
        let model = sampling_config.map(|c| c.model).unwrap_or_default();

        let tag = self.reminder_wrapper_tag();
        let conversation = self.chat_state_handle.get_conversation().await;
        // Fit the snapshot to THIS model's window rather than sending the
        // conversation whole: a small-window model would otherwise fail the
        // capture with a context-length error, which is deterministic and
        // never retried. The helper also strips reasoning where required and
        // pops a trailing tool run — `/todo` fires mid-turn, so the snapshot
        // can end with an assistant message whose tool calls have no result
        // yet.
        let mut items = crate::session::helpers::session_recap::budget_instruction_items(
            conversation,
            self.todo_capture_instruction(&todo_tool, request),
            strip_reasoning,
            context_window.saturating_sub(LOOP_GROWTH_RESERVE_TOKENS),
        );
        if items.len() == 1 {
            // Only the instruction survived the budget. The capture still runs,
            // but off the request text alone — say so rather than let a
            // context-free item look like a considered one.
            tracing::warn!(
                context_window,
                "todo capture: no conversation fit this model's window; capturing from the request alone"
            );
        }
        // Same tools as the main turn: they serialize into the cached prefix,
        // so trimming the list to the ones the loop honors would cost the whole
        // conversation's prompt cache to save nothing. What the agent may
        // actually run is decided at dispatch, in `capture_action`.
        let tool_specs = self.turn_base_tool_specs(&self.prepare_tool_definitions().await);
        let hosted_tools = self.hosted_tools_for_turn();
        let conv_id = format!("todo-{}", uuid::Uuid::new_v4());

        let mut added: Vec<String> = Vec::new();
        let mut tools_used = 0usize;
        // Why the last append attempt failed, if one did. A run that ends with
        // nothing on the list must say which of the two happened: the tool
        // refused the write, or the agent never asked for one.
        let mut append_error: Option<String> = None;
        let mut nudges_left = 1usize;

        for _ in 0..MAX_MODEL_CALLS {
            let base_request = self.parent_cached_request(AuxCall {
                items: items.clone(),
                tools: tool_specs.clone(),
                hosted_tools: hosted_tools.clone(),
                model: model.clone(),
                reasoning_effort,
                backend: sampling_client.api_backend(),
                conv_id: conv_id.clone(),
                req_id: format!("xai-todo-{}", uuid::Uuid::new_v4()),
            });
            // One capture is several model calls, so a transient failure on any
            // of them would otherwise throw away the whole run's work. Same
            // bounded budget `/btw` uses.
            let response =
                (|| sampling_client.conversation_collect(fresh_req_id(&base_request, "todo")))
                    .retry(aux_retry_policy())
                    .when(should_retry_aux_call)
                    .notify(|e: &SamplingError, backoff: std::time::Duration| {
                        tracing::warn!(
                            backoff_ms = backoff.as_millis() as u64,
                            error = %e,
                            "todo capture transient failure; retrying"
                        );
                    })
                    .await?;
            log_prompt_cache_hit("todo", sampling_client.api_backend(), &response);

            let calls: Vec<ToolCall> = response.tool_calls().to_vec();
            items.extend(echoed_response_items(response.items, strip_reasoning));

            if calls.is_empty() {
                // A model that answered in prose gets exactly one correction.
                if nudges_left == 0 {
                    break;
                }
                nudges_left -= 1;
                items.push(no_tool_call_nudge(tag, &todo_tool));
                continue;
            }
            for call in &calls {
                let outcome = self.run_capture_tool(call, &todo_tool, tools_used).await;
                if outcome.spent_a_read {
                    tools_used += 1;
                }
                if outcome.append_error.is_some() {
                    append_error = outcome.append_error;
                }
                added.extend(outcome.appended);
                items.push(ConversationItem::tool_result(
                    call.id.to_string(),
                    outcome.result_text,
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
            return Err(match append_error {
                Some(reason) => TodoCaptureError::TodoWriteFailed(reason),
                None => TodoCaptureError::NothingAdded,
            });
        }
        tracing::info!(
            added = added.len(),
            tools_used,
            "todo capture appended items"
        );
        Ok(TodoCaptureOutcome { added, tools_used })
    }

    /// The name this session advertises the append-capable todo tool under, or
    /// why `/todo` cannot run here.
    ///
    /// Kind alone is not enough: opencode's `todowrite` is also
    /// [`ToolKind::Plan`] and replaces the whole list instead of merging into
    /// it, so an append cannot be expressed through it. The namespace is what
    /// identifies the implementation, and it survives a `name_override` —
    /// which is exactly what a harness preset uses to rename tools per
    /// provider, and why nothing here may compare against the literal
    /// `todo_write`.
    async fn resolve_capture_todo_tool(
        &self,
        bridge: &xai_grok_tools::bridge::ToolBridge,
    ) -> Result<String, TodoCaptureError> {
        use xai_grok_tools::types::tool::ToolNamespace;
        let Some(name) = bridge.tool_for_kind(ToolKind::Plan).await else {
            return Err(TodoCaptureError::UnsupportedTodoTool("none".into()));
        };
        match bridge.tool_namespace(&name) {
            Some(ToolNamespace::GrokBuild) => Ok(name),
            _ => Err(TodoCaptureError::UnsupportedTodoTool(name)),
        }
    }

    /// The capture agent's instruction, appended after the snapshot as the
    /// run's one user turn.
    fn todo_capture_instruction(&self, todo_tool: &str, request: &str) -> String {
        let tag = self.reminder_wrapper_tag();
        format!(
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
             2. Call `{todo_tool}` with the item(s) to add, then stop.\n\n\
             CONSTRAINTS, enforced outside this prompt — a call that breaks one is \
             refused and never runs, so do not spend a turn on it:\n\
             - Adding todo items is your ONLY permitted mutation. Every edit, write, \
             shell command, and subagent call is refused.\n\
             - Your `{todo_tool}` call APPENDS: the ids you send are replaced with \
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
        )
    }

    /// Dispatch one tool call from the capture loop.
    async fn run_capture_tool(
        &self,
        call: &ToolCall,
        todo_tool: &str,
        tools_used: usize,
    ) -> CaptureToolOutcome {
        let args = parse_tool_arguments(&call.arguments);
        let kind = self.agent.borrow().tool_bridge().tool_kind(&call.name);
        match capture_action(&call.name, kind, todo_tool, tools_used) {
            CaptureAction::Refuse(message) => CaptureToolOutcome::said(message),
            CaptureAction::Append => {
                let contents = self.capture_todo_contents(todo_tool, &args).await;
                if contents.is_empty() {
                    return CaptureToolOutcome::said(format!(
                        "No item to add: the `{todo_tool}` call carried no todo content. \
                         Send `todos: [{{\"content\": \"...\"}}]`."
                    ));
                }
                match self
                    .append_capture_todos(todo_tool, &call.id, add_only_todo_args(&contents))
                    .await
                {
                    Ok(text) => CaptureToolOutcome {
                        appended: contents,
                        ..CaptureToolOutcome::said(text)
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "todo capture: append failed");
                        CaptureToolOutcome {
                            append_error: Some(e.to_string()),
                            ..CaptureToolOutcome::said(format!("Adding the todo failed: {e}"))
                        }
                    }
                }
            }
            CaptureAction::Read => {
                let text = match self
                    .workspace_ops
                    .call_tool(&call.name, args, &call.id, Some(&self.session_info.id.0))
                    .await
                {
                    Ok(result) => {
                        xai_grok_tools::util::truncate_str(&result.prompt_text, TOOL_RESULT_BUDGET)
                            .to_owned()
                    }
                    // A failed read still cost the budget it was given, and the
                    // model needs the error to pick a different angle.
                    Err(e) => format!("`{}` failed: {e}", call.name),
                };
                CaptureToolOutcome {
                    spent_a_read: true,
                    ..CaptureToolOutcome::said(text)
                }
            }
        }
    }

    /// The item contents in a todo-tool call, whatever the model spelled them.
    ///
    /// Parses through the bridge first, which reverse-maps client-facing
    /// parameter names to canonical ones — a harness may rename `todos` the
    /// same way it renames the tool — and yields the typed input the tool
    /// itself would see. Falls back to reading the JSON directly, so a call
    /// the strict parser rejects (an extra field, a status the schema does not
    /// know) still contributes its content instead of being dropped.
    async fn capture_todo_contents(&self, todo_tool: &str, args: &serde_json::Value) -> Vec<String> {
        use xai_grok_tools::types::tool_io::ToolInput;
        let bridge = self.agent.borrow().tool_bridge().clone();
        if let Ok(ToolInput::TodoWrite(input)) = bridge.try_parse(todo_tool, args.clone()).await {
            let contents: Vec<String> = input
                .todos
                .iter()
                .filter_map(|t| {
                    let text = t
                        .content
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(&t.id);
                    let text = text.trim();
                    (!text.is_empty()).then(|| text.to_owned())
                })
                .collect();
            if !contents.is_empty() {
                return contents;
            }
        }
        contents_from_todo_write_args(args)
    }

    /// Run the sanitized append through the session's own todo tool, so the
    /// list, its persisted state, and the client's plan view all move the way
    /// they do when the main agent writes a todo.
    ///
    /// Dispatch is by the session's advertised name with canonical parameter
    /// names: the registry reverse-maps client names onto canonical ones and
    /// leaves everything else alone, so canonical keys arrive as themselves
    /// under any rename.
    async fn append_capture_todos(
        &self,
        todo_tool: &str,
        call_id: &str,
        args: serde_json::Value,
    ) -> Result<String, xai_tool_runtime::ToolError> {
        let result = self
            .workspace_ops
            .call_tool(todo_tool, args, call_id, Some(&self.session_info.id.0))
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
            capture_action(
                TODO_WRITE,
                Some(ToolKind::Plan),
                TODO_WRITE,
                MAX_TOOL_CALLS + 3
            ),
            CaptureAction::Append
        );
    }

    /// A harness that renames the todo tool renames it for the model too, so
    /// the append is recognized by the session's advertised name. Comparing
    /// against the canonical `todo_write` instead would refuse the one call
    /// the run exists to make.
    #[test]
    fn the_append_is_recognized_under_a_renamed_todo_tool() {
        assert_eq!(
            capture_action("TodoWrite", Some(ToolKind::Plan), "TodoWrite", 0),
            CaptureAction::Append
        );
        // And the canonical name is then just another unknown tool.
        assert!(matches!(
            capture_action(TODO_WRITE, None, "TodoWrite", 0),
            CaptureAction::Refuse(_)
        ));
    }

    /// A capture turn continues a tool call it made itself, so what the model
    /// returned has to go back verbatim. Dropping the reasoning that came with
    /// a call is what the Responses API rejects the continuation over; dropping
    /// a hosted search's items leaves the next request describing a search that
    /// never happened.
    #[test]
    fn a_response_is_echoed_whole_unless_the_backend_strips() {
        let response = vec![
            ConversationItem::Reasoning(xai_grok_sampling_types::synthesized_reasoning_item(
                "which file owns the push",
            )),
            ConversationItem::assistant_tool_calls(vec![ToolCall {
                id: "call_todo_1".into(),
                name: "grep".to_string(),
                arguments: r#"{"pattern":"git push"}"#.into(),
                vendor: Default::default(),
            }]),
        ];

        let kept = echoed_response_items(response.clone(), false);
        assert_eq!(kept.len(), 2, "every item rides along by default");
        assert!(matches!(kept[0], ConversationItem::Reasoning(_)));

        // Messages is the one backend that cannot take the reasoning.
        let stripped = echoed_response_items(response, true);
        assert!(
            !stripped
                .iter()
                .any(|i| matches!(i, ConversationItem::Reasoning(_))),
            "reasoning must be stripped where the backend rejects it"
        );
        assert!(
            stripped
                .iter()
                .any(|i| matches!(i, ConversationItem::Assistant(a) if !a.tool_calls.is_empty())),
            "stripping reasoning must not take the call with it"
        );
    }

    /// What models actually emit for arguments: nothing, a run of concatenated
    /// objects, or something that is not JSON at all. The main turn tolerates
    /// all three; a capture that dropped them would silently lose the write.
    #[test]
    fn tool_arguments_survive_what_models_emit() {
        assert_eq!(parse_tool_arguments(""), serde_json::json!({}));
        assert_eq!(parse_tool_arguments("   "), serde_json::json!({}));
        assert_eq!(
            parse_tool_arguments(r#"{"todos":[{"content":"a"}]}"#),
            serde_json::json!({"todos": [{"content": "a"}]})
        );
        // Concatenated objects: the first one is the call, the rest are the
        // model repeating itself.
        assert_eq!(
            parse_tool_arguments(r#"{"todos":[{"content":"a"}]}{"todos":[{"content":"b"}]}"#),
            serde_json::json!({"todos": [{"content": "a"}]})
        );
        // Unparseable arguments reach the tool as `raw`, the same shape the
        // main turn hands it, so the tool reports the failure.
        assert_eq!(
            parse_tool_arguments("not json"),
            serde_json::json!({"raw": "not json"})
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
            match capture_action(name, kind, TODO_WRITE, 0) {
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
            capture_action(
                "read_file",
                Some(ToolKind::Read),
                TODO_WRITE,
                MAX_TOOL_CALLS - 1
            ),
            CaptureAction::Read
        );
        assert!(matches!(
            capture_action("read_file", Some(ToolKind::Read), TODO_WRITE, MAX_TOOL_CALLS),
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
