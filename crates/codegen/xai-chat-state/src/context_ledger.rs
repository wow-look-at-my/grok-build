//! The `/context` request list: what the provider reported for each main-loop request, and the items each sent.

use xai_grok_sampling_types::{ContentPart, ConversationItem, SyntheticReason, TokenUsage};

/// Characters kept from an item's text for a ledger row.
pub const LEDGER_PREVIEW_CHARS: usize = 120;

/// The provider's token counts for one request, as its usage JSON reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderUsage {
    /// Prompt tokens neither read from nor written to the cache.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl ProviderUsage {
    pub fn from_token_usage(u: &TokenUsage) -> Self {
        let prompt = u64::from(u.prompt_tokens);
        let cache_read = u64::from(u.cached_prompt_tokens);
        let cache_write = u64::from(u.cache_creation_prompt_tokens);
        Self {
            input: prompt.saturating_sub(cache_read + cache_write),
            cache_read,
            cache_write,
            output: u64::from(u.completion_tokens),
        }
    }

    /// The whole prompt: input plus both cache counts.
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    /// The prompt plus the output.
    pub fn total(&self) -> u64 {
        self.prompt() + self.output
    }
}

/// One reported request: how long the conversation was when it went out, and the provider's counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextSample {
    pub conversation_len: usize,
    pub usage: ProviderUsage,
}

/// One conversation item in a request row.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LedgerItem {
    /// Position in the conversation.
    pub index: usize,
    pub kind: String,
    /// UTF-8 length of the item's text.
    pub bytes: u64,
    pub preview: String,
}

/// One request: the provider's counts and the items added since the request before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LedgerRequest {
    /// The user prompt index the request belongs to.
    pub turn: usize,
    pub usage: ProviderUsage,
    pub items: Vec<LedgerItem>,
}

/// The requests reported since the last load, compaction or rewind, and the items added after the last one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextLedger {
    pub requests: Vec<LedgerRequest>,
    /// Items added after the last request. No request has sent them yet.
    pub unsent: Vec<LedgerItem>,
}

/// Build the ledger from the conversation and its samples. A sample past the end of the conversation is ignored.
pub fn build_context_ledger(
    items: &[ConversationItem],
    samples: &[ContextSample],
) -> ContextLedger {
    let mut requests = Vec::with_capacity(samples.len());
    let mut prev_len = 0usize;
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
            usage: sample.usage,
            items: ledger_items(items, range),
        });
        prev_len = sample.conversation_len;
    }
    ContextLedger {
        requests,
        unsent: ledger_items(items, prev_len.min(items.len())..items.len()),
    }
}

fn is_human_prompt(item: &ConversationItem) -> bool {
    matches!(item, ConversationItem::User(u) if u.synthetic_reason == SyntheticReason::Human)
}

fn ledger_items(items: &[ConversationItem], range: std::ops::Range<usize>) -> Vec<LedgerItem> {
    range
        .filter_map(|index| {
            let item = items.get(index)?;
            let text = item_text(item);
            Some(LedgerItem {
                index,
                kind: item_kind(item),
                bytes: text.len() as u64,
                preview: one_line(&text, LEDGER_PREVIEW_CHARS),
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
    one_line(&item_text(item), max_chars)
}

fn item_text(item: &ConversationItem) -> String {
    match item {
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
            let calls: Vec<String> = a
                .tool_calls
                .iter()
                .map(|c| format!("{}({})", c.name, c.arguments))
                .collect();
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
    }
}

fn one_line(text: &str, max_chars: usize) -> String {
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

    fn usage(input: u64, cache_read: u64) -> ProviderUsage {
        ProviderUsage {
            input,
            cache_read,
            cache_write: 0,
            output: 10,
        }
    }

    #[test]
    fn provider_usage_splits_the_prompt_into_input_and_cache() {
        let u = ProviderUsage::from_token_usage(&TokenUsage {
            prompt_tokens: 1_000,
            completion_tokens: 50,
            total_tokens: 1_050,
            reasoning_tokens: 0,
            cached_prompt_tokens: 700,
            cache_creation_prompt_tokens: 200,
        });
        assert_eq!(
            (u.input, u.cache_read, u.cache_write, u.output),
            (100, 700, 200, 50)
        );
        assert_eq!((u.prompt(), u.total()), (1_000, 1_050));
    }

    #[test]
    fn each_request_owns_the_items_sent_since_the_one_before() {
        let mut items = vec![
            ConversationItem::system("s".repeat(400)),
            ConversationItem::user("u".repeat(4000)),
        ];
        let first = ContextSample {
            conversation_len: items.len(),
            usage: usage(3000, 0),
        };
        items.push(ConversationItem::assistant("a".repeat(40)));
        items.push(ConversationItem::user("second prompt"));
        let second = ContextSample {
            conversation_len: items.len(),
            usage: usage(30, 3000),
        };
        items.push(ConversationItem::assistant("tail"));

        let ledger = build_context_ledger(&items, &[first, second]);

        assert_eq!(ledger.requests.len(), 2);
        let r0 = &ledger.requests[0];
        assert_eq!(r0.turn, 1);
        assert_eq!(r0.usage, usage(3000, 0));
        assert_eq!(r0.items.iter().map(|i| i.index).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(
            (r0.items[1].kind.as_str(), r0.items[1].bytes),
            ("user message", 4000)
        );
        let r1 = &ledger.requests[1];
        assert_eq!(r1.turn, 2);
        assert_eq!(r1.items.len(), 2);
        assert_eq!(ledger.unsent.len(), 1);
        assert_eq!(ledger.unsent[0].index, 4);
    }

    #[test]
    fn a_sample_past_the_conversation_end_is_ignored() {
        let items = vec![ConversationItem::user("hi")];
        let stale = ContextSample {
            conversation_len: 5,
            usage: usage(10, 0),
        };
        let ledger = build_context_ledger(&items, &[stale]);
        assert!(ledger.requests.is_empty());
        assert_eq!(ledger.unsent.len(), 1);
    }

    #[test]
    fn a_preview_is_one_line_and_cut() {
        let item = ConversationItem::user("a\n\nb  c".to_string() + &"x".repeat(200));
        let preview = item_preview(&item, 10);
        assert_eq!(preview, "a b cxxxxx…");
    }
}
