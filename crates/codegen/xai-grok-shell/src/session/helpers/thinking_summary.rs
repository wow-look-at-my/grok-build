//! Prompt helpers for the summary under a long thinking block.

use std::collections::VecDeque;

use crate::sampling::ConversationResponse;
use crate::session::helpers::chat::floor_char_boundary;
use xai_grok_tools::util::{ceil_char_boundary, truncate_bytes};

pub(crate) const THINKING_SUMMARY_MAX_CHARS: usize = 320;
/// Room for the answer plus the thinking of a model that cannot turn it off.
pub(crate) const THINKING_SUMMARY_MAX_OUTPUT_TOKENS: u32 = 4096;
const INPUT_HEAD_CHARS: usize = 16_000;
const INPUT_TAIL_CHARS: usize = 32_000;
/// Upper bound on the per-session summary history, so a long session cannot
/// grow it without limit. Only the newest entries are ever selected.
pub(crate) const THINKING_SUMMARY_HISTORY_CAPACITY: usize = 64;

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
    let head = truncate_bytes(thinking, INPUT_HEAD_CHARS);
    let tail_start = ceil_char_boundary(thinking, thinking.len() - INPUT_TAIL_CHARS);
    #[allow(clippy::string_slice)] // `ceil_char_boundary` returns a char boundary
    let tail = &thinking[tail_start..];
    Some(format!(
        "{head}\n\n[... middle of the reasoning omitted ...]\n\n{tail}"
    ))
}

/// The prior summaries a call at `now_ms` may see: everything inside
/// `window_secs` of it, together with the most recent `min_count`. The union
/// keeps a few predecessors in view through a slow stretch where the time
/// window alone would select nothing, and covers a rapid-fire burst without
/// having to count it. Oldest first. A zero window and a zero count select
/// nothing.
pub(crate) fn select_prior_summaries(
    entries: &VecDeque<(i64, String)>,
    now_ms: i64,
    window_secs: u32,
    min_count: u32,
) -> Vec<String> {
    if entries.is_empty() || (window_secs == 0 && min_count == 0) {
        return Vec::new();
    }
    let window_ms = i64::from(window_secs) * 1000;
    let count_floor = entries.len().saturating_sub(min_count as usize);
    entries
        .iter()
        .enumerate()
        .filter(|(idx, (ts, _))| {
            let in_window = window_secs > 0 && now_ms.saturating_sub(*ts) <= window_ms;
            in_window || *idx >= count_floor
        })
        .map(|(_, (_, text))| text.clone())
        .collect()
}

/// The thinking summaries one session has produced, oldest first, keyed by the
/// `stream_start_ms` of the call each describes, plus how far back the next
/// call may draw on them. Per-actor: a subagent and its parent each keep their
/// own chain, so one session's summaries never reach another's prompt.
pub(crate) struct ThinkingSummaryHistory {
    window_secs: u32,
    min_count: u32,
    entries: parking_lot::Mutex<VecDeque<(i64, String)>>,
}

impl Default for ThinkingSummaryHistory {
    fn default() -> Self {
        Self::new(
            crate::agent::config::UiConfig::THINKING_SUMMARY_HISTORY_WINDOW_SECS_DEFAULT,
            crate::agent::config::UiConfig::THINKING_SUMMARY_HISTORY_MIN_COUNT_DEFAULT,
        )
    }
}

impl ThinkingSummaryHistory {
    pub(crate) fn new(window_secs: u32, min_count: u32) -> Self {
        Self {
            window_secs,
            min_count,
            entries: parking_lot::Mutex::new(VecDeque::new()),
        }
    }

    /// The summaries the call at `stream_start_ms` should be given, oldest first.
    pub(crate) fn prior_for(&self, stream_start_ms: i64) -> Vec<String> {
        select_prior_summaries(
            &self.entries.lock(),
            stream_start_ms,
            self.window_secs,
            self.min_count,
        )
    }

    /// Record a produced summary, keeping the buffer bounded.
    pub(crate) fn record(&self, stream_start_ms: i64, summary: String) {
        let mut entries = self.entries.lock();
        entries.push_back((stream_start_ms, summary));
        while entries.len() > THINKING_SUMMARY_HISTORY_CAPACITY {
            entries.pop_front();
        }
    }
}

/// The single user message sent to the summary model. `prior` holds the recent
/// summaries for this session, oldest first, and is empty for the first one.
pub(crate) fn thinking_summary_instruction(thinking: &str, prior: &[String]) -> String {
    let mut out = String::from(
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
           -> \"Add --dry-run flag, thread it to executor\"\n",
    );
    if !prior.is_empty() {
        out.push_str(
            "\nEarlier summaries from this session are listed below, oldest first. They are \
             context for DIRECTION only: read them to see where the work is heading, and to \
             avoid repeating a point one of them already made.\n\
             Do not quote, copy, or lightly reword any of them. Each summary must describe \
             only the reasoning below, in this block's own words.\n\n\
             <previous_summaries>\n",
        );
        for line in prior {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("</previous_summaries>\n");
    }
    out.push_str(
        "\nPlain text only. No preamble, labels, markdown or quotes. Do not call tools.\n\n",
    );
    out.push_str("<reasoning>\n");
    out.push_str(thinking);
    out.push_str("\n</reasoning>");
    out
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
        let text = thinking_summary_instruction("check the parser first", &[]);
        assert!(text.contains("<reasoning>\ncheck the parser first\n</reasoning>"));
        assert!(text.contains("5 to 20 words"));
        assert!(text.contains("never \"The assistant\""));
        assert!(
            !text.contains("<previous_summaries>"),
            "the first summary of a session must carry no history block"
        );
    }

    #[test]
    fn instruction_frames_prior_summaries_as_direction_and_forbids_reuse() {
        let text = thinking_summary_instruction(
            "now weigh the retry budget",
            &[
                "Offset bug is in the caller".to_string(),
                "Reuse the retry helper".to_string(),
            ],
        );
        assert!(text.contains("<previous_summaries>"));
        assert!(text.contains("- Offset bug is in the caller"));
        assert!(text.contains("- Reuse the retry helper"));
        assert!(text.contains("</previous_summaries>"));
        assert!(
            text.contains("DIRECTION"),
            "the block must say what the prior summaries are for"
        );
        assert!(
            text.contains("avoid repeating"),
            "the block must ask the model not to repeat a prior point"
        );
        assert!(
            text.contains("Do not quote, copy, or lightly reword"),
            "the block must forbid echoing a prior summary's wording"
        );
        // The current reasoning still ends the message and is the only <reasoning> block.
        assert!(text.ends_with("<reasoning>\nnow weigh the retry budget\n</reasoning>"));
        assert_eq!(text.matches("<reasoning>").count(), 1);
    }

    #[test]
    fn selection_takes_the_union_of_the_window_and_the_count_floor() {
        let entries: VecDeque<(i64, String)> = (0..6)
            .map(|i| (1_000 * i, format!("summary {i}")))
            .collect();
        // Window of 2.5s at t=5000 keeps entries at 3000..=5000; the count
        // floor of 5 keeps everything from index 1 up. Union, oldest first.
        let selected = select_prior_summaries(&entries, 5_000, 2, 5);
        assert_eq!(
            selected,
            vec![
                "summary 1".to_string(),
                "summary 2".to_string(),
                "summary 3".to_string(),
                "summary 4".to_string(),
                "summary 5".to_string(),
            ]
        );
    }

    #[test]
    fn count_floor_carries_a_slow_session_where_the_window_is_empty() {
        let entries: VecDeque<(i64, String)> = (0..3)
            .map(|i| (i * 600_000, format!("summary {i}")))
            .collect();
        // Ten minutes between calls, a 2s window selects none by time; the
        // floor of 2 still carries the two newest.
        let selected = select_prior_summaries(&entries, 1_200_000, 2, 2);
        assert_eq!(
            selected,
            vec!["summary 1".to_string(), "summary 2".to_string()]
        );
    }

    #[test]
    fn zero_window_and_zero_count_select_nothing() {
        let entries: VecDeque<(i64, String)> =
            (0..4).map(|i| (100 * i, format!("summary {i}"))).collect();
        assert!(select_prior_summaries(&entries, 400, 0, 0).is_empty());
        assert!(select_prior_summaries(&VecDeque::new(), 400, 120, 5).is_empty());
    }

    #[test]
    fn history_records_in_order_and_bounds_itself() {
        let history = ThinkingSummaryHistory::new(120, 5);
        for i in 0..3 {
            history.record(1_000 * i, format!("summary {i}"));
        }
        assert_eq!(
            history.prior_for(2_000),
            vec![
                "summary 0".to_string(),
                "summary 1".to_string(),
                "summary 2".to_string(),
            ]
        );
        for i in 3..(THINKING_SUMMARY_HISTORY_CAPACITY + 10) {
            history.record(i as i64 * 1_000, format!("summary {i}"));
        }
        let all = history.prior_for((THINKING_SUMMARY_HISTORY_CAPACITY + 9) as i64 * 1_000);
        assert_eq!(all.len(), THINKING_SUMMARY_HISTORY_CAPACITY);
        assert_eq!(
            all.last().unwrap(),
            &format!("summary {}", THINKING_SUMMARY_HISTORY_CAPACITY + 9)
        );
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
