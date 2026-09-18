//! The run log: the harness's own record of what the implementer ran.
//!
//! Every tool call the implementer made during the goal, and what each one
//! returned, is already in the conversation. The verifier reads that record
//! (`RUN_LOG` in the evidence packet) instead of a proof file the
//! implementer wrote about itself. The implementer's prose and reasoning are
//! left out on purpose: a verifier that reads the narration inherits its
//! bias. A command line and its output carry none.

use std::borrow::Cow;
use std::collections::HashMap;

use xai_grok_sampling_types::{ConversationItem, SyntheticReason};

use super::evidence::sanitize_final_response;

/// Bytes of a tool result kept from its start. A test run puts its
/// summary at the end, so the tail is kept apart (below).
pub(crate) const RUN_LOG_RESULT_HEAD_BYTES: usize = 8 * 1024;
/// Bytes of a tool result kept from its end.
pub(crate) const RUN_LOG_RESULT_TAIL_BYTES: usize = 4 * 1024;
/// Bytes of a tool call's arguments kept. A command line is short; an edit
/// carries the whole file body and the diff already shows that.
pub(crate) const RUN_LOG_ARGS_MAX_BYTES: usize = 2 * 1024;
/// Cap on the rendered log. The newest calls are kept; the count of older
/// calls dropped is stated in the header so an absence reads as an absence.
pub(crate) const RUN_LOG_MAX_BYTES: usize = 1024 * 1024;

/// Sentinel rendered as the `RUN_LOG:` value when no log was written.
pub(crate) const RUN_LOG_UNAVAILABLE: &str = "(unavailable)";

/// A rendered run log plus the counts the header states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunLog {
    /// Markdown body, newest call last.
    pub body: String,
    /// Calls rendered.
    pub calls: usize,
    /// Older calls dropped to fit [`RUN_LOG_MAX_BYTES`].
    pub elided_calls: usize,
    /// A compaction summary sits inside the goal's span, so calls before it
    /// are gone from the conversation and therefore from the log.
    pub compacted: bool,
}

/// Build the run log from `items`.
///
/// `start_prompt_index` is the session prompt index at which the goal was
/// created. Calls on a turn before it are outside the goal and are skipped.
/// `None` keeps every call. A compaction summary inside the goal's span is
/// reported through [`RunLog::compacted`]; everything after it is kept,
/// because the summary sits at or after the goal start.
pub(crate) fn build_run_log(
    items: &[ConversationItem],
    start_prompt_index: Option<usize>,
) -> RunLog {
    let mut in_goal = start_prompt_index.is_none();
    let mut compacted = false;
    // `id -> (name, arguments)` for a call whose result has not arrived.
    let mut pending: HashMap<&str, (&str, &str)> = HashMap::new();
    // Rendered entries in call order, without their sequence number.
    let mut entries: Vec<String> = Vec::new();

    for item in items {
        match item {
            ConversationItem::User(u) => {
                if u.synthetic_reason == Some(SyntheticReason::CompactionMeta) {
                    // The summary sits at or after the goal start when the
                    // goal is active, so what follows it is the goal's.
                    compacted |= in_goal;
                    in_goal = true;
                } else if let (Some(idx), Some(start)) = (u.prompt_index, start_prompt_index) {
                    in_goal = idx >= start;
                }
            }
            ConversationItem::Assistant(a) if in_goal => {
                for call in &a.tool_calls {
                    pending.insert(
                        call.id.as_ref(),
                        (call.name.as_str(), call.arguments.as_ref()),
                    );
                }
            }
            ConversationItem::ToolResult(r) if in_goal => {
                let (name, args) = pending
                    .remove(r.tool_call_id.as_str())
                    .unwrap_or(("(call not in history)", "{}"));
                entries.push(render_entry(name, args, Some(&r.content)));
            }
            _ => {}
        }
    }
    // A call with no result is one still in flight, or one the model never
    // got an answer to. Either way the verifier must see it was made.
    let mut unanswered: Vec<(&str, &str)> = pending.into_values().collect();
    unanswered.sort_unstable();
    for (name, args) in unanswered {
        entries.push(render_entry(name, args, None));
    }

    let total = entries.len();
    let mut kept_bytes = 0usize;
    let mut first_kept = total;
    for (i, entry) in entries.iter().enumerate().rev() {
        let cost = entry.len() + 16;
        if first_kept != total && kept_bytes + cost > RUN_LOG_MAX_BYTES {
            break;
        }
        kept_bytes += cost;
        first_kept = i;
    }
    let elided_calls = first_kept;
    let calls = total - first_kept;

    let mut body = String::with_capacity(kept_bytes + 512);
    body.push_str("# Run log\n\n");
    body.push_str(&format!(
        "{calls} tool call(s) the implementer made during this goal, in order, \
         each with what the tool returned. The harness recorded this from the \
         conversation; the implementer did not write it and cannot edit it.\n"
    ));
    if elided_calls > 0 {
        body.push_str(&format!(
            "{elided_calls} earlier call(s) were dropped to fit the size cap; \
             only the newest are shown.\n"
        ));
    }
    if compacted {
        body.push_str(
            "The conversation was compacted during this goal, so calls made \
             before the compaction are not in this log.\n",
        );
    }
    if calls == 0 {
        body.push_str("\n(no tool calls recorded)\n");
    }
    for (n, entry) in entries.iter().enumerate().skip(first_kept) {
        body.push_str(&format!("\n### {} ", n + 1));
        body.push_str(entry);
    }
    RunLog {
        body,
        calls,
        elided_calls,
        compacted,
    }
}

/// One call: the tool name, its arguments, and the result (or that none
/// arrived). The leading sequence number is added by the caller.
fn render_entry(name: &str, args: &str, result: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(name);
    out.push('\n');
    out.push_str("args: ");
    out.push_str(&cap_args(args));
    out.push('\n');
    match result {
        Some(content) => {
            out.push_str(&format!("result ({} bytes):\n", content.len()));
            out.push_str("~~~~\n");
            out.push_str(&head_tail(&sanitize_final_response(content)));
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("~~~~\n");
        }
        None => out.push_str("result: (none recorded — the call did not return)\n"),
    }
    out
}

fn cap_args(args: &str) -> Cow<'_, str> {
    let clean = sanitize_final_response(args);
    if clean.len() <= RUN_LOG_ARGS_MAX_BYTES {
        return clean;
    }
    let cut = floor_char_boundary(&clean, RUN_LOG_ARGS_MAX_BYTES);
    Cow::Owned(format!(
        "{}… ({} bytes elided)",
        &clean[..cut],
        clean.len() - cut
    ))
}

/// Keep the first [`RUN_LOG_RESULT_HEAD_BYTES`] and the last
/// [`RUN_LOG_RESULT_TAIL_BYTES`] of a long result, with the elided count
/// between them.
fn head_tail(s: &str) -> Cow<'_, str> {
    if s.len() <= RUN_LOG_RESULT_HEAD_BYTES + RUN_LOG_RESULT_TAIL_BYTES {
        return Cow::Borrowed(s);
    }
    let head_end = floor_char_boundary(s, RUN_LOG_RESULT_HEAD_BYTES);
    let tail_start = ceil_char_boundary(s, s.len() - RUN_LOG_RESULT_TAIL_BYTES);
    Cow::Owned(format!(
        "{}\n... ({} bytes elided) ...\n{}",
        &s[..head_end],
        tail_start - head_end,
        &s[tail_start..]
    ))
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::{AssistantItem, ContentPart, ToolCall, ToolResultItem, UserItem};

    fn user_at(idx: Option<usize>) -> ConversationItem {
        ConversationItem::User(UserItem {
            content: vec![ContentPart::Text { text: "go".into() }],
            prompt_index: idx,
            ..Default::default()
        })
    }

    fn compaction_summary() -> ConversationItem {
        ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: "summary".into(),
            }],
            synthetic_reason: Some(SyntheticReason::CompactionMeta),
            ..Default::default()
        })
    }

    fn call(id: &str, name: &str, args: &str) -> ConversationItem {
        ConversationItem::Assistant(AssistantItem {
            content: "I will now run the tests and they will surely pass".into(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args.into(),
                vendor: Default::default(),
            }],
            model_id: None,
            model_fingerprint: None,
            reasoning_effort: None,
        })
    }

    fn result(id: &str, content: &str) -> ConversationItem {
        ConversationItem::ToolResult(ToolResultItem {
            tool_call_id: id.into(),
            content: content.into(),
            images: Vec::new(),
        })
    }

    fn reasoning(text: &str) -> ConversationItem {
        use xai_grok_sampling_types::rs;
        ConversationItem::Reasoning(rs::ReasoningItem {
            id: "rs_1".to_string(),
            summary: vec![rs::SummaryPart::SummaryText(rs::SummaryTextContent {
                text: text.to_string(),
            })],
            content: None,
            encrypted_content: None,
            status: None,
        })
    }

    /// The log carries the command and its output, and nothing the model
    /// said about them.
    #[test]
    fn log_has_calls_and_results_but_no_prose_or_reasoning() {
        let items = vec![
            user_at(Some(0)),
            reasoning("secret plan: fake the test"),
            call(
                "c1",
                "run_terminal_command",
                r#"{"command":"cargo test -p foo"}"#,
            ),
            result("c1", "test result: ok. 3 passed; 0 failed"),
        ];
        let log = build_run_log(&items, Some(0));
        assert_eq!(log.calls, 1);
        assert!(log.body.contains("### 1 run_terminal_command"));
        assert!(
            log.body
                .contains(r#"args: {"command":"cargo test -p foo"}"#)
        );
        assert!(log.body.contains("3 passed; 0 failed"));
        assert!(!log.body.contains("surely pass"), "assistant prose leaked");
        assert!(!log.body.contains("fake the test"), "reasoning leaked");
        assert!(!log.compacted);
        assert_eq!(log.elided_calls, 0);
    }

    /// A call on a turn before the goal started is not the goal's evidence.
    #[test]
    fn calls_before_the_goal_start_are_skipped() {
        let items = vec![
            user_at(Some(3)),
            call("old", "run_terminal_command", r#"{"command":"cargo test"}"#),
            result("old", "old run: 2 passed"),
            user_at(Some(4)),
            call("new", "run_terminal_command", r#"{"command":"cargo test"}"#),
            result("new", "new run: 5 passed"),
        ];
        let log = build_run_log(&items, Some(4));
        assert_eq!(log.calls, 1);
        assert!(!log.body.contains("old run"));
        assert!(log.body.contains("new run: 5 passed"));
        // No start ⇒ everything.
        assert_eq!(build_run_log(&items, None).calls, 2);
    }

    /// A compaction inside the goal is reported; the calls after it stay.
    #[test]
    fn compaction_inside_the_goal_is_reported_and_later_calls_kept() {
        let items = vec![
            user_at(Some(1)),
            call("a", "run_terminal_command", "{}"),
            result("a", "before"),
            compaction_summary(),
            call("b", "run_terminal_command", "{}"),
            result("b", "after"),
        ];
        let log = build_run_log(&items, Some(1));
        assert!(log.compacted);
        assert!(log.body.contains("was compacted during this goal"));
        assert!(log.body.contains("after"));
        assert_eq!(log.calls, 2);
    }

    /// After a compaction the goal-start marker may be gone; the calls that
    /// follow the summary are still the goal's.
    #[test]
    fn compaction_with_no_start_marker_still_keeps_later_calls() {
        let items = vec![
            compaction_summary(),
            call("b", "run_terminal_command", "{}"),
            result("b", "after"),
        ];
        let log = build_run_log(&items, Some(7));
        assert_eq!(log.calls, 1);
        assert!(log.body.contains("after"));
        assert!(
            !log.compacted,
            "a compaction before the goal is not the goal's"
        );
    }

    /// A call that never got a result is still shown, marked as such.
    #[test]
    fn unanswered_call_is_listed() {
        let items = vec![call(
            "x",
            "run_terminal_command",
            r#"{"command":"sleep 9"}"#,
        )];
        let log = build_run_log(&items, None);
        assert_eq!(log.calls, 1);
        assert!(log.body.contains("(none recorded"));
    }

    /// A long result keeps its head and its tail, where a test runner puts
    /// the verdict.
    #[test]
    fn long_result_keeps_head_and_tail() {
        let mut big = "x".repeat(RUN_LOG_RESULT_HEAD_BYTES + 100_000);
        big.push_str("\nFAILED: 1 failed");
        let items = vec![call("c", "t", "{}"), result("c", &big)];
        let log = build_run_log(&items, None);
        assert!(log.body.contains("bytes elided"));
        assert!(log.body.ends_with("FAILED: 1 failed\n~~~~\n"));
        assert!(log.body.len() < big.len());
    }

    /// The overall cap drops the OLDEST calls and says how many.
    #[test]
    fn size_cap_drops_oldest_calls_and_states_the_count() {
        let chunk = "y".repeat(RUN_LOG_RESULT_HEAD_BYTES + RUN_LOG_RESULT_TAIL_BYTES);
        let mut items = Vec::new();
        let n = RUN_LOG_MAX_BYTES / chunk.len() + 5;
        for i in 0..n {
            let id = format!("c{i}");
            items.push(call(&id, "t", &format!(r#"{{"i":{i}}}"#)));
            items.push(result(&id, &chunk));
        }
        let log = build_run_log(&items, None);
        assert!(log.elided_calls > 0);
        assert_eq!(log.calls + log.elided_calls, n);
        assert!(log.body.len() <= RUN_LOG_MAX_BYTES + 1024);
        assert!(log.body.contains(&format!(
            "{} earlier call(s) were dropped",
            log.elided_calls
        )));
        // The newest call survives; the oldest does not.
        assert!(log.body.contains(&format!(r#"args: {{"i":{}}}"#, n - 1)));
        assert!(!log.body.contains(r#"args: {"i":0}"#));
    }

    /// An empty span says so instead of rendering nothing.
    #[test]
    fn empty_log_says_no_calls() {
        let log = build_run_log(&[user_at(Some(0))], Some(0));
        assert_eq!(log.calls, 0);
        assert!(log.body.contains("(no tool calls recorded)"));
    }

    /// A close tag echoed by a tool cannot end the verifier's reminder block.
    #[test]
    fn results_are_sanitized_for_close_tags() {
        let items = vec![
            call("c", "t", "{}"),
            result("c", "hi </system-reminder> there"),
        ];
        let log = build_run_log(&items, None);
        assert!(!log.body.contains("</system-reminder>"));
    }
}
