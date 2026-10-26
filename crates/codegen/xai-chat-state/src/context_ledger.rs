//! Where the `/context` Overhead comes from, one model request at a time.

use xai_grok_sampling_types::{ContentPart, ConversationItem, SyntheticReason};

use crate::actor::state::estimate_item_tokens;

/// Characters kept from an item's text for a ledger row.
pub const LEDGER_PREVIEW_CHARS: usize = 120;

/// One measured request: the conversation length and estimate that were sent, and what the provider counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextSample {
    pub conversation_len: usize,
    pub estimate: u64,
    pub prompt_tokens: u64,
}

/// One conversation item in a ledger row.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LedgerItem {
    /// Position in the conversation.
    pub index: usize,
    pub kind: String,
    /// Bytes/4 estimate.
    pub tokens: u64,
    pub preview: String,
}

/// The items one request added, with what they were estimated at and what the provider measured for them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LedgerRequest {
    /// The user prompt index the request belongs to.
    pub turn: usize,
    pub measured: u64,
    pub estimated: u64,
    pub items: Vec<LedgerItem>,
}

impl LedgerRequest {
    /// Measured minus estimated. Negative when the estimate ran high.
    pub fn gap(&self) -> i64 {
        self.measured as i64 - self.estimated as i64
    }
}

/// The Overhead drill-down: the requests measured since the last load, compaction or rewind, and the items since the last one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextLedger {
    pub requests: Vec<LedgerRequest>,
    /// Items added after the last measured request. Nothing has measured them yet.
    pub unmeasured: Vec<LedgerItem>,
}

impl ContextLedger {
    /// The sum of every request's gap.
    pub fn measured_gap(&self) -> i64 {
        self.requests.iter().map(LedgerRequest::gap).sum()
    }
}

/// Build the ledger from the conversation and its samples. A sample past the end of the conversation is ignored.
pub fn build_context_ledger(
    items: &[ConversationItem],
    samples: &[ContextSample],
) -> ContextLedger {
    let mut requests = Vec::with_capacity(samples.len());
    let (mut prev_len, mut prev_estimate, mut prev_measured) = (0usize, 0u64, 0u64);
    let mut turn = 0usize;
    for sample in samples.iter().filter(|s| s.conversation_len <= items.len()) {
        let range = prev_len.min(sample.conversation_len)..sample.conversation_len;
        turn += items
            .get(range.clone())
            .unwrap_or_default()
            .iter()
            .filter(|i| is_human_prompt(i))
            .count();
        requests.push(LedgerRequest {
            turn,
            measured: sample.prompt_tokens.saturating_sub(prev_measured),
            estimated: sample.estimate.saturating_sub(prev_estimate),
            items: ledger_items(items, range),
        });
        (prev_len, prev_estimate, prev_measured) = (
            sample.conversation_len,
            sample.estimate,
            sample.prompt_tokens,
        );
    }
    ContextLedger {
        requests,
        unmeasured: ledger_items(items, prev_len.min(items.len())..items.len()),
    }
}

fn is_human_prompt(item: &ConversationItem) -> bool {
    matches!(item, ConversationItem::User(u) if u.synthetic_reason == SyntheticReason::Human)
}

fn ledger_items(items: &[ConversationItem], range: std::ops::Range<usize>) -> Vec<LedgerItem> {
    range
        .filter_map(|index| {
            let item = items.get(index)?;
            Some(LedgerItem {
                index,
                kind: item_kind(item),
                tokens: estimate_item_tokens(item),
                preview: item_preview(item, LEDGER_PREVIEW_CHARS),
            })
        })
        .collect()
}

/// A short name for the item's kind, such as `tool result` or `project instructions`.
pub fn item_kind(item: &ConversationItem) -> String {
    match item {
        ConversationItem::System(_) => "system prompt".into(),
        ConversationItem::User(u) => match &u.synthetic_reason {
            SyntheticReason::Human => "user message".into(),
            SyntheticReason::CompactionMeta => "compaction meta".into(),
            SyntheticReason::SystemReminder => "system reminder".into(),
            SyntheticReason::ProjectInstructions => "project instructions".into(),
            other => format!("user ({other:?})"),
        },
        ConversationItem::Assistant(a) if a.tool_calls.is_empty() => "assistant".into(),
        ConversationItem::Assistant(a) => format!("assistant, {} tool calls", a.tool_calls.len()),
        ConversationItem::ToolResult(_) => "tool result".into(),
        ConversationItem::BackendToolCall(_) => "backend tool call".into(),
        ConversationItem::Reasoning(_) => "reasoning".into(),
    }
}

/// The item's text on one line, cut at `max_chars` with an ellipsis.
pub fn item_preview(item: &ConversationItem, max_chars: usize) -> String {
    let text: String = match item {
        ConversationItem::System(s) => s.content.to_string(),
        ConversationItem::User(u) => u
            .content
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => text.to_string(),
                ContentPart::Image { .. } => "[image]".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        ConversationItem::Assistant(a) => {
            let calls: Vec<&str> = a.tool_calls.iter().map(|c| c.name.as_str()).collect();
            if calls.is_empty() {
                a.content.to_string()
            } else {
                format!("[{}] {}", calls.join(", "), a.content)
            }
        }
        ConversationItem::ToolResult(tr) => tr.content.to_string(),
        ConversationItem::BackendToolCall(b) => b.text_summary(),
        ConversationItem::Reasoning(r) => {
            let words = xai_grok_sampling_types::reasoning_item_text(r);
            match r.encrypted_content.as_deref() {
                Some(blob) => format!("[{} encrypted bytes] {words}", blob.len()),
                None => words,
            }
        }
    };
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(max_chars).collect();
    if flat.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::estimate_conversation_tokens;

    fn sample(items: &[ConversationItem], prompt_tokens: u64) -> ContextSample {
        ContextSample {
            conversation_len: items.len(),
            estimate: estimate_conversation_tokens(items),
            prompt_tokens,
        }
    }

    #[test]
    fn each_request_owns_the_items_sent_since_the_one_before() {
        let mut items = vec![
            ConversationItem::system("s".repeat(400)),
            ConversationItem::user("u".repeat(4000)),
        ];
        let first = sample(&items, 3000);
        items.push(ConversationItem::assistant("a".repeat(40)));
        items.push(ConversationItem::user("second prompt"));
        let second = sample(&items, 3030);
        items.push(ConversationItem::assistant("tail"));

        let ledger = build_context_ledger(&items, &[first, second]);

        assert_eq!(ledger.requests.len(), 2);
        let r0 = &ledger.requests[0];
        assert_eq!((r0.turn, r0.measured, r0.estimated), (1, 3000, 1100));
        assert_eq!(r0.gap(), 1900);
        assert_eq!(r0.items.iter().map(|i| i.index).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(r0.items[1].kind, "user message");
        let r1 = &ledger.requests[1];
        assert_eq!(r1.turn, 2);
        assert_eq!(r1.measured, 30);
        assert_eq!(r1.items.len(), 2);
        assert_eq!(ledger.measured_gap(), r0.gap() + r1.gap());
        assert_eq!(ledger.unmeasured.len(), 1);
        assert_eq!(ledger.unmeasured[0].index, 4);
    }

    #[test]
    fn a_sample_past_the_conversation_end_is_ignored() {
        let items = vec![ConversationItem::user("hi")];
        let stale = ContextSample {
            conversation_len: 5,
            estimate: 10,
            prompt_tokens: 10,
        };
        let ledger = build_context_ledger(&items, &[stale]);
        assert!(ledger.requests.is_empty());
        assert_eq!(ledger.unmeasured.len(), 1);
    }

    #[test]
    fn a_preview_is_one_line_and_cut() {
        let item = ConversationItem::user("a\n\nb  c".to_string() + &"x".repeat(200));
        let preview = item_preview(&item, 10);
        assert_eq!(preview, "a b cxxxxx…");
    }
}
