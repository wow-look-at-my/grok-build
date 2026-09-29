//! Prompt helpers for the summary under a long thinking block.

use crate::sampling::ConversationResponse;
use crate::session::helpers::chat::floor_char_boundary;

pub(crate) const THINKING_SUMMARY_MAX_CHARS: usize = 320;
/// Room for the answer plus the thinking of a model that cannot turn it off.
pub(crate) const THINKING_SUMMARY_MAX_OUTPUT_TOKENS: u32 = 4096;
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

/// Every non-empty block gets a summary: thinking is drawn collapsed, so the
/// summary is the only part of a short block the user sees.
pub(crate) fn summarizable_thinking(thinking: &str) -> Option<String> {
    let thinking = thinking.trim();
    if thinking.is_empty() {
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
         Write a terse gist of what it worked out or decided: 5 to 20 words, \
         a fragment, like a commit subject.\n\n\
         Style:\n\
         - Lead with the verb or the finding. No subject: never \"The assistant\", \"I\", \"It\".\n\
         - Drop articles and filler. Name the concrete thing: file, function, bug, choice.\n\
         - One idea. No \"and then\" chains, no hedging, no restating the task.\n\n\
         Examples (bad -> good):\n\
         - \"The assistant analyzed the parser code and decided that it should fix the offset.\" \
           -> \"Offset bug is in the caller, not the lexer\"\n\
         - \"The assistant is considering whether to write a new retry loop or reuse the existing one, and decides to reuse it.\" \
           -> \"Reuse existing retry helper instead of new loop\"\n\
         - \"I need to figure out which config value takes precedence. After checking, the environment variable wins.\" \
           -> \"Env var overrides config file\"\n\
         - \"The assistant reads the failing test output to understand why the build is red.\" \
           -> \"Build red from missing mold linker\"\n\
         - \"The user wants a new flag, so the assistant plans to add it to the CLI args and wire it through.\" \
           -> \"Add --dry-run flag, thread it to executor\"\n\n\
         Plain text only. No preamble, labels, markdown or quotes. Do not call tools.\n\n\
         <reasoning>\n{thinking}\n</reasoning>"
    )
}

/// Clean the raw model output into a short plain-text summary.
pub(crate) fn clean_thinking_summary(raw: &str) -> String {
    let mut out = super::session_recap::clean_recap_text(drop_inline_thinking(raw));
    if out.len() > THINKING_SUMMARY_MAX_CHARS {
        let cut = floor_char_boundary(&out, THINKING_SUMMARY_MAX_CHARS);
        out.truncate(cut);
        out = out.trim_end().to_string();
        out.push('\u{2026}');
    }
    out
}

/// The text after a `<think>` block that a model wrote into its answer.
fn drop_inline_thinking(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("<think>") {
        return raw;
    }
    match trimmed.find("</think>") {
        Some(end) => &trimmed[end + "</think>".len()..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_empty_thinking_needs_no_summary() {
        assert_eq!(summarizable_thinking(""), None);
        assert_eq!(summarizable_thinking("  \n\t "), None);
        assert_eq!(
            summarizable_thinking("I will read the file.").as_deref(),
            Some("I will read the file.")
        );
    }

    #[test]
    fn long_thinking_is_passed_whole_under_the_budget() {
        let text = "word ".repeat(800);
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
        assert!(text.contains("5 to 20 words"));
        assert!(text.contains("never \"The assistant\""));
    }

    #[test]
    fn clean_drops_an_inline_think_block() {
        assert_eq!(
            clean_thinking_summary("<think>\nlong musing\n</think>\n\nOffset bug in caller"),
            "Offset bug in caller"
        );
        assert_eq!(clean_thinking_summary("<think>never closed"), "");
        assert_eq!(
            clean_thinking_summary("Mentions <think> mid-text"),
            "Mentions <think> mid-text"
        );
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
