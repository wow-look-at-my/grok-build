//! The report a compaction writes beside its checkpoint.

use xai_grok_sampling_types::{ContentPart, ConversationItem, SyntheticReason};

/// Everything the report reads. The caller measures; this module only renders.
pub struct CompactionReportInput<'a> {
    pub trigger: &'a str,
    pub model: &'a str,
    pub input_stage: &'a str,
    /// The session's token count before compaction.
    pub tokens_before: u64,
    /// The reseed divides by this number.
    pub estimate_at_last_response: u64,
    pub pre_conversation_estimate: u64,
    /// The history that replaced the conversation.
    pub history: &'a [ConversationItem],
    /// The token count the session holds after the replace.
    pub tokens_after: u64,
    pub summary: &'a str,
    pub checkpoint_file: Option<&'a str>,
}

pub struct CompactionReport {
    pub markdown: String,
    /// One line for the scrollback, under "Context compacted".
    pub breakdown: String,
}

/// Rows past this count are listed only in the largest-items table.
const MAX_ITEM_ROWS: usize = 200;
const LARGEST_ITEMS: usize = 8;
const PREVIEW_CHARS: usize = 100;
/// A scale above this means the local estimate misses most of what.
const SUSPECT_SCALE: f64 = 1.5;

struct ItemRow {
    index: usize,
    kind: String,
    tokens: u64,
    preview: String,
}

pub fn render_compaction_report(input: &CompactionReportInput<'_>) -> CompactionReport {
    let rows: Vec<ItemRow> = input
        .history
        .iter()
        .enumerate()
        .map(|(index, item)| ItemRow {
            index,
            kind: item_kind(item),
            tokens: xai_chat_state::estimate_item_tokens(item),
            preview: preview(item),
        })
        .collect();
    let history_estimate: u64 = rows.iter().map(|r| r.tokens).sum();
    let summary_tokens =
        xai_chat_state::estimate_item_tokens(&ConversationItem::user_meta(input.summary));
    let scale = if input.estimate_at_last_response > 0 && input.tokens_before > 0 {
        Some(input.tokens_before as f64 / input.estimate_at_last_response as f64)
    } else {
        None
    };
    let mut largest: Vec<&ItemRow> = rows.iter().collect();
    largest.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    largest.truncate(LARGEST_ITEMS);

    let breakdown = {
        let mut parts = vec![
            format!("summary {}", fmt_k(summary_tokens)),
            format!("{} items est. {}", rows.len(), fmt_k(history_estimate)),
        ];
        if let Some(top) = largest.first() {
            parts.push(format!(
                "largest #{} {} {}",
                top.index,
                top.kind,
                fmt_k(top.tokens)
            ));
        }
        if let Some(scale) = scale {
            parts.push(format!("scaled ×{scale:.2} to provider count"));
        }
        parts.join(" · ")
    };

    let mut md = String::new();
    md.push_str("# Compaction report\n\n");
    md.push_str(&format!("- Trigger: {}\n", input.trigger));
    md.push_str(&format!("- Model: {}\n", input.model));
    md.push_str(&format!("- Input stage: {}\n", input.input_stage));
    md.push_str(&format!(
        "- Tokens before: {}\n",
        fmt_int(input.tokens_before)
    ));
    md.push_str(&format!(
		"- Tokens after: {} (a projection; the next response replaces it with the provider's count)\n",
		fmt_int(input.tokens_after)
	));
    if let Some(file) = input.checkpoint_file {
        md.push_str(&format!(
            "- Checkpoint: `{file}` (the exact compacted history as JSON)\n"
        ));
    }
    md.push('\n');

    md.push_str("## How \"tokens after\" was computed\n\n");
    md.push_str(&format!(
        "- Estimate of the full conversation before compaction (bytes/4): {}\n",
        fmt_int(input.pre_conversation_estimate)
    ));
    md.push_str(&format!(
        "- Estimate of the conversation at the last provider response (bytes/4): {}\n",
        fmt_int(input.estimate_at_last_response)
    ));
    md.push_str(&format!(
        "- Estimate of the compacted history (bytes/4): {}\n",
        fmt_int(history_estimate)
    ));
    match scale {
        Some(scale) => {
            let projected = (history_estimate as f64 * scale).round() as u64;
            md.push_str(&format!(
                "- Scale: {} ÷ {} = ×{scale:.2}\n",
                fmt_int(input.tokens_before),
                fmt_int(input.estimate_at_last_response)
            ));
            md.push_str(&format!(
                "- Projection: {} × {scale:.2} = {}, capped at the count before ({})\n",
                fmt_int(history_estimate),
                fmt_int(projected),
                fmt_int(input.tokens_before)
            ));
            if scale > SUSPECT_SCALE {
                md.push_str(&format!(
                    "\n> The provider counted ×{scale:.2} what the local estimate saw. \
					 The tools, the images, or a stale count can cause that. \
					 \"Tokens after\" carries the same factor. \
					 Compare the compacted estimate above ({}) with the \"after\" number.\n",
                    fmt_int(history_estimate)
                ));
            }
        }
        None => md.push_str(
            "- Scale: none (no provider count yet); \"tokens after\" is the plain estimate\n",
        ),
    }
    md.push('\n');

    md.push_str(&format!("## Largest items (of {})\n\n", rows.len()));
    push_table(&mut md, largest.iter().copied());
    md.push('\n');

    md.push_str(&format!(
        "## Compacted history ({} items, est. {})\n\n",
        rows.len(),
        fmt_int(history_estimate)
    ));
    push_table(&mut md, rows.iter().take(MAX_ITEM_ROWS));
    if rows.len() > MAX_ITEM_ROWS {
        md.push_str(&format!(
            "\n{} more items are not listed. The checkpoint holds all of them.\n",
            rows.len() - MAX_ITEM_ROWS
        ));
    }
    md.push('\n');

    md.push_str(&format!(
        "## Summary (est. {})\n\n",
        fmt_int(summary_tokens)
    ));
    md.push_str(input.summary);
    md.push('\n');

    CompactionReport {
        markdown: md,
        breakdown,
    }
}

fn push_table<'a>(md: &mut String, rows: impl Iterator<Item = &'a ItemRow>) {
    md.push_str("| # | Kind | Tokens (est.) | Preview |\n|---|---|---|---|\n");
    for r in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            r.index,
            r.kind,
            fmt_int(r.tokens),
            r.preview
        ));
    }
}

fn item_kind(item: &ConversationItem) -> String {
    match item {
        ConversationItem::System(_) => "system prompt".into(),
        ConversationItem::User(u) => match &u.synthetic_reason {
<<<<<<< HEAD
            SyntheticReason::Human => "user message".into(),
            SyntheticReason::CompactionMeta => "compaction meta".into(),
            SyntheticReason::SystemReminder => "system reminder".into(),
            SyntheticReason::ProjectInstructions => "project instructions".into(),
            other => format!("user ({other:?})"),
=======
            None => "user message".into(),
            Some(SyntheticReason::CompactionMeta) => "compaction meta".into(),
            Some(SyntheticReason::SystemReminder) => "system reminder".into(),
            Some(SyntheticReason::ProjectInstructions) => "project instructions".into(),
            Some(other) => format!("user ({other:?})"),
>>>>>>> origin/master
        },
        ConversationItem::Assistant(a) if a.tool_calls.is_empty() => "assistant".into(),
        ConversationItem::Assistant(a) => format!("assistant, {} tool calls", a.tool_calls.len()),
        ConversationItem::ToolResult(_) => "tool result".into(),
        ConversationItem::BackendToolCall(_) => "backend tool call".into(),
        ConversationItem::Reasoning(_) => "reasoning".into(),
    }
}

fn preview(item: &ConversationItem) -> String {
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
    let mut out: String = flat.chars().take(PREVIEW_CHARS).collect();
    if flat.chars().count() > PREVIEW_CHARS {
        out.push('…');
    }
    out.replace('|', "\\|").replace('`', "'")
}

fn fmt_int(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn fmt_k(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::{SystemItem, ToolResultItem};

    fn tool_result(chars: usize) -> ConversationItem {
        ConversationItem::ToolResult(ToolResultItem {
            tool_call_id: "call_1".into(),
            content: "x".repeat(chars).into(),
            images: vec![],
        })
    }

    fn input<'a>(history: &'a [ConversationItem], summary: &'a str) -> CompactionReportInput<'a> {
        CompactionReportInput {
            trigger: "auto",
            model: "m",
            input_stage: "verbatim",
            tokens_before: 255_800,
            estimate_at_last_response: 64_000,
            pre_conversation_estimate: 64_000,
            history,
            tokens_after: 231_800,
            summary,
            checkpoint_file: Some("compaction_checkpoints/abc.json"),
        }
    }

    #[test]
    fn names_the_largest_item_and_the_scale() {
        let history = vec![
            ConversationItem::System(SystemItem {
                content: "sys".into(),
<<<<<<< HEAD
                synthetic_reason: SyntheticReason::Primary,
=======
>>>>>>> origin/master
            }),
            ConversationItem::user_meta("prefix"),
            tool_result(400_000),
            ConversationItem::user_meta("the summary"),
        ];
        let report = render_compaction_report(&input(&history, "the summary"));
        let top = xai_chat_state::estimate_item_tokens(&history[2]);
        assert!(
            report
                .breakdown
                .contains(&format!("largest #2 tool result {}", fmt_k(top))),
            "breakdown: {}",
            report.breakdown
        );
        assert!(
            report.breakdown.contains("scaled ×4.00"),
            "breakdown: {}",
            report.breakdown
        );
        assert!(
            report.markdown.contains("> The provider counted ×4.00"),
            "{}",
            report.markdown
        );
        assert!(report.markdown.contains("## Summary"));
        assert!(report.markdown.ends_with("the summary\n"));
        assert!(report.markdown.contains("compaction_checkpoints/abc.json"));
    }

    #[test]
    fn no_provider_count_means_no_scale() {
        let history = vec![ConversationItem::user_meta("s")];
        let mut i = input(&history, "s");
        i.estimate_at_last_response = 0;
        let report = render_compaction_report(&i);
        assert!(!report.breakdown.contains("scaled"), "{}", report.breakdown);
        assert!(report.markdown.contains("Scale: none"));
    }

    #[test]
    fn a_preview_cannot_break_the_table() {
        let history = vec![ConversationItem::user_meta("a | b `c`\nd")];
        let report = render_compaction_report(&input(&history, "s"));
        assert!(
            report.markdown.contains("| a \\| b 'c' d |"),
            "{}",
            report.markdown
        );
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(255_800), "255,800");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
    }
}
