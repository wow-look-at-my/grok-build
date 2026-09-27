//! Prompt helpers for the summary under a long thinking block.

use crate::sampling::ConversationResponse;
use crate::session::helpers::chat::floor_char_boundary;

pub(crate) const THINKING_SUMMARY_MIN_CHARS: usize = 800;
pub(crate) const THINKING_SUMMARY_MAX_CHARS: usize = 320;
const INPUT_HEAD_CHARS: usize = 16_000;
const INPUT_TAIL_CHARS: usize = 32_000;

/// The words of every reasoning item in the response, in order.
pub(crate) fn response_thinking_text(response: &ConversationResponse) -> String {
    response
        .reasoning_items()
        .map(xai_grok_sampling_types::reasoning_item_text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn summarizable_thinking(thinking: &str) -> Option<String> {
    let thinking = thinking.trim();
    if thinking.len() < THINKING_SUMMARY_MIN_CHARS {
        return None;
    }
    if thinking.len() <= INPUT_HEAD_CHARS + INPUT_TAIL_CHARS {
        return Some(thinking.to_string());
    }
    let head_end = floor_char_boundary(thinking, INPUT_HEAD_CHARS);
    let mut tail_start = thinking.len() - INPUT_TAIL_CHARS;
    while !thinking.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    Some(format!(
        "{}\n\n[... middle of the reasoning omitted ...]\n\n{}",
        &thinking[..head_end],
        &thinking[tail_start..]
    ))
}

/// The single user message sent to the summary model.
pub(crate) fn thinking_summary_instruction(thinking: &str) -> String {
    format!(
        "Below is the private reasoning a coding assistant wrote before it acted. \
         Summarize it for the user in one or two short sentences: what the \
         assistant worked out and what it decided to do. Plain text only. No \
         preamble, no labels, no markdown, no quotes. Do not call tools.\n\n\
         <reasoning>\n{thinking}\n</reasoning>"
    )
}

/// Clean the raw model output into a short plain-text summary.
pub(crate) fn clean_thinking_summary(raw: &str) -> String {
    let mut out = super::session_recap::clean_recap_text(raw);
    if out.len() > THINKING_SUMMARY_MAX_CHARS {
        let cut = floor_char_boundary(&out, THINKING_SUMMARY_MAX_CHARS);
        out.truncate(cut);
        out = out.trim_end().to_string();
        out.push('\u{2026}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_thinking_needs_no_summary() {
        assert_eq!(summarizable_thinking("I will read the file."), None);
        assert_eq!(
            summarizable_thinking(&"x".repeat(THINKING_SUMMARY_MIN_CHARS - 1)),
            None
        );
    }

    #[test]
    fn long_thinking_is_passed_whole_under_the_budget() {
        let text = "word ".repeat(THINKING_SUMMARY_MIN_CHARS);
        assert_eq!(summarizable_thinking(&text).as_deref(), Some(text.trim()));
    }

    #[test]
    fn very_long_thinking_keeps_its_head_and_its_tail() {
        let text = format!(
            "START {} END",
            "é".repeat(INPUT_HEAD_CHARS + INPUT_TAIL_CHARS)
        );
        let cut = summarizable_thinking(&text).expect("long enough");
        assert!(cut.starts_with("START"));
        assert!(cut.ends_with("END"));
        assert!(cut.contains("middle of the reasoning omitted"));
        assert!(cut.len() < text.len());
    }

    #[test]
    fn instruction_carries_the_reasoning() {
        let text = thinking_summary_instruction("check the parser first");
        assert!(text.contains("<reasoning>\ncheck the parser first\n</reasoning>"));
        assert!(text.contains("one or two short sentences"));
    }

    #[test]
    fn clean_collapses_and_caps() {
        assert_eq!(
            clean_thinking_summary("Summary: \"Reads the parser,\n\n then fixes it.\""),
            "Reads the parser, then fixes it."
        );
        let capped = clean_thinking_summary(&"word ".repeat(200));
        assert!(capped.len() <= THINKING_SUMMARY_MAX_CHARS + '\u{2026}'.len_utf8());
        assert!(capped.ends_with('\u{2026}'));
    }
}
