use std::borrow::Cow;

/// Default wrap width for soft-wrapping (used by bash, task_output).
pub const DEFAULT_SOFT_WRAP_WIDTH: usize = 2_000;

/// Default preview shown before a truncation footer.
pub const PREVIEW_SIZE: usize = 2_000;

/// Marker appended by `truncate_str_with_marker` when content is cut.
pub(crate) const TRUNCATION_MARKER: &str = "…";

/// Truncate a line to at most `max_chars` characters, respecting UTF-8
/// boundaries. Content beyond `max_chars` is **discarded** and replaced with
/// a marker. Returns `Cow::Borrowed` if the line is already within the limit
/// (zero-copy fast path). Returns `Cow::Owned` with a truncation marker
/// appended if the line was cut.
pub fn truncate_line(line: &str, max_chars: usize) -> Cow<'_, str> {
    if line.len() <= max_chars {
        return Cow::Borrowed(line);
    }
    let char_count = line.chars().count();
    if char_count <= max_chars {
        return Cow::Borrowed(line);
    }
    let end_byte = line
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    #[allow(clippy::string_slice)] // `char_indices().nth` yields a char boundary
    let head = &line[..end_byte];
    Cow::Owned(format!("{head} [... truncated ({char_count} chars total)]"))
}

/// Soft-wrap a long line by inserting newlines every `wrap_width` characters.
/// **All content is preserved** — nothing is discarded.
///
/// Returns `Cow::Borrowed` if the line is already within `wrap_width` (zero-copy).
///
/// This is the correct strategy for bash and task_output, where the total output
/// is already size-bounded (30KB) and the model benefits from seeing all of it.
/// The problem with long lines isn't size — it's that the model has no structure
/// to anchor on. Wrapping adds that structure without losing content.
pub fn soft_wrap_line(line: &str, wrap_width: usize) -> Cow<'_, str> {
    // Fast path: same byte-length optimization as truncate_line (see comment there).
    if line.len() <= wrap_width {
        return Cow::Borrowed(line);
    }
    let char_count = line.chars().count();
    if char_count <= wrap_width {
        return Cow::Borrowed(line);
    }
    let num_wraps = char_count.saturating_sub(1) / wrap_width;
    let mut result = String::with_capacity(line.len() + num_wraps);
    let mut chars_on_current_line = 0;
    for ch in line.chars() {
        if chars_on_current_line >= wrap_width {
            result.push('\n');
            chars_on_current_line = 0;
        }
        result.push(ch);
        chars_on_current_line += 1;
    }
    Cow::Owned(result)
}

/// The longest prefix of `s` that fits in `max_bytes`, cut at a char boundary.
#[allow(clippy::string_slice)] // the index is `floor_char_boundary`'s output
pub fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    let end = s.floor_char_boundary(max_bytes);
    &s[..end]
}

/// Truncate a string to at most `max_bytes` bytes at a valid UTF-8 boundary.
/// Returns the string if it fits. No truncation marker is added.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    truncate_bytes(s, max_bytes)
}

/// The longest suffix of `s` that fits in `max_bytes`, cut at a char boundary.
#[allow(clippy::string_slice)] // the index is `ceil_char_boundary`'s output
pub fn tail_bytes(s: &str, max_bytes: usize) -> &str {
    let start = s.ceil_char_boundary(s.len().saturating_sub(max_bytes));
    &s[start..]
}

/// Text on hand, and the size of the output it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartialOutput<'a> {
    text: &'a str,
    total_bytes: usize,
}

impl<'a> PartialOutput<'a> {
    pub fn whole(text: &'a str) -> Self {
        Self {
            text,
            total_bytes: text.len(),
        }
    }

    /// Part of an output of `total_bytes`.
    pub fn part_of(text: &'a str, total_bytes: usize) -> Self {
        Self {
            text,
            total_bytes: total_bytes.max(text.len()),
        }
    }

    pub fn text(&self) -> &'a str {
        self.text
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

/// Truncate output to a UTF-8-safe preview plus a model-visible footer.
///
/// The cap decides whether truncation happens. When triggered, the returned
/// value contains the first `preview_bytes` bytes snapped to a char boundary
/// followed by `[Output truncated - <N> bytes total...]`, where `N` is the
/// size of the whole output, not of the part on hand.
pub fn truncate_with_preview(
    output: PartialOutput<'_>,
    max_bytes: usize,
    preview_bytes: usize,
    footer_hint: Option<&str>,
) -> (String, bool) {
    let PartialOutput { text, total_bytes } = output;
    let whole = total_bytes <= text.len();
    if whole && text.len() <= max_bytes {
        return (text.to_string(), false);
    }

    let footer = match footer_hint {
        Some(hint) => format!("[Output truncated - {total_bytes} bytes total. {hint}]"),
        None => format!("[Output truncated - {total_bytes} bytes total]"),
    };
    // Text that fits the limit can still be part of a larger output; the
    // reader still needs the total size and where to find the rest.
    if text.len() <= max_bytes {
        return (format!("{text}\n\n{footer}"), true);
    }
    let preview = truncate_str(text, preview_bytes.min(text.len()));
    (format!("{preview}\n\n{footer}"), true)
}

/// Truncate a string to at most `max_bytes` bytes at a valid UTF-8 boundary,
/// appending `TRUNCATION_MARKER` when truncation happens. Total byte length
/// of the returned string is always `<= max_bytes`. Returns `Cow::Borrowed`
/// when the input already fits (no marker added -- only signal truncation
/// when truncation happened). Returns `Cow::Owned` with the marker appended
/// when content was cut. When `max_bytes == TRUNCATION_MARKER.len()`, returns
/// the marker so the truncation signal is preserved.
pub fn truncate_str_with_marker(s: &str, max_bytes: usize) -> Cow<'_, str> {
    if s.len() <= max_bytes {
        return Cow::Borrowed(s);
    }
    if TRUNCATION_MARKER.len() > max_bytes {
        tracing::debug!(
            max_bytes,
            marker_len = TRUNCATION_MARKER.len(),
            "truncate_str_with_marker: budget too small for marker; truncation will be silent",
        );
        return Cow::Borrowed(truncate_bytes(s, max_bytes));
    }
    let head = truncate_bytes(s, max_bytes - TRUNCATION_MARKER.len());
    Cow::Owned(format!("{head}{TRUNCATION_MARKER}"))
}

/// Find the largest byte index `<= index` that is a char boundary in `s`. An
/// `index` past the end of `s` returns `s.len()`.
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    s.floor_char_boundary(index)
}

/// Find the smallest byte index `>= index` that is a char boundary in `s`. An
/// `index` past the end of `s` returns `s.len()`.
pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    s.ceil_char_boundary(index)
}

/// Estimate the number of tokens in a string using the bytes/4 heuristic.
pub fn estimate_tokens(s: &str) -> usize {
    xai_token_estimation::estimate_tokens(s) as usize
}

/// Estimate the number of chars per token using the bytes/4 heuristic.
pub fn estimate_chars(s: u64) -> u64 {
    xai_token_estimation::estimate_chars(s)
}

/// Every output fits several columns.
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    const UNITS: &[&str] = &["KB", "MB", "GB", "TB", "PB"];
    let mut val = bytes as f64 / 1024.0;
    for unit in UNITS {
        if val < 1023.95 {
            return format!("{val:.1} {unit}");
        }
        val /= 1024.0;
    }
    format!("{val:.1} EB")
}

/// Apply soft-wrapping to every line in a multi-line string.
/// All content is preserved. Lines already within `wrap_width` are untouched.
pub fn soft_wrap_lines(text: &str, wrap_width: usize) -> String {
    let mut result = String::with_capacity(text.len() + 256);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        match soft_wrap_line(line, wrap_width) {
            Cow::Borrowed(s) => result.push_str(s),
            Cow::Owned(s) => result.push_str(&s),
        }
    }
    if text.ends_with('\n') && !text.is_empty() {
        result.push('\n');
    }
    result
}

/// Separator between the retained head and tail of a truncated output.
pub(crate) const FRONT_BACK_TRUNCATION_MARKER: &str = "\n\n... (output truncated) ...\n\n";

/// Truncate a string keeping the first half and last half of the character
/// budget, inserting a separator in the middle.
///
/// Returns `(result, was_truncated)`. When `s.len() <= max_chars` the
/// original string is returned unchanged and `was_truncated` is `false`.
pub fn truncate_front_and_back(s: &str, max_chars: usize) -> (String, bool) {
    if s.len() <= max_chars {
        return (s.to_string(), false);
    }
    let half = max_chars / 2;
    let front_end = s
        .char_indices()
        .nth(half)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let back_start = {
        let total_chars = s.chars().count();
        if total_chars <= half {
            0
        } else {
            s.char_indices()
                .nth(total_chars - half)
                .map(|(i, _)| i)
                .unwrap_or(0)
        }
    };
    let ellipsis = FRONT_BACK_TRUNCATION_MARKER;
    #[allow(clippy::string_slice)] // both offsets come from `char_indices().nth`
    let (head, tail) = (&s[..front_end], &s[back_start..]);
    let mut result = String::with_capacity(front_end + ellipsis.len() + (s.len() - back_start));
    result.push_str(head);
    result.push_str(ellipsis);
    result.push_str(tail);
    (result, true)
}

/// Truncate a string by keeping the first and last halves of a **character**
/// budget, inserting `"..."` in the middle. Used in the image-description
/// pipeline.
///
/// When `s.chars().count() <= max_chars` the input is returned unchanged.
/// Otherwise the result contains `⌊max_chars/2⌋` chars from the start,
/// the literal `"..."`, then `⌊max_chars/2⌋` chars from the end.
pub fn truncate_middle(s: &str, max_chars: usize) -> String {
    const MARKER: &str = "...";
    const MARKER_LEN: usize = MARKER.len();

    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    // The marker counts against the budget so the total never exceeds `max_chars`.
    let remaining = max_chars.saturating_sub(MARKER_LEN);
    let front_count = remaining / 2;
    let back_count = remaining - front_count;

    // Front: first `front_count` chars.
    let front_end = s
        .char_indices()
        .nth(front_count)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    // Back: last `back_count` chars.
    let back_start = if char_count <= back_count {
        0
    } else {
        s.char_indices()
            .nth(char_count - back_count)
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    #[allow(clippy::string_slice)] // both offsets come from `char_indices().nth`
    let (head, tail) = (&s[..front_end], &s[back_start..]);
    let mut result = String::with_capacity(front_end + MARKER_LEN + (s.len() - back_start));
    result.push_str(head);
    result.push_str(MARKER);
    result.push_str(tail);
    result
}

/// Truncate a multi-line string at line boundaries to fit within a character
/// budget.
///
/// Returns `(result, was_truncated)`. When the content already fits, the
/// joined+trimmed content is returned unchanged.
pub fn truncate_lines_to_char_budget(content: &str, budget: usize) -> (String, bool) {
    let trimmed = content.trim();
    if trimmed.len() <= budget {
        return (trimmed.to_string(), false);
    }
    // Cut the budget down to a char boundary before looking for the last complete line.
    let truncated = truncate_bytes(trimmed, budget);
    let last_nl = truncated.rfind('\n');
    match last_nl {
        Some(idx) => {
            #[allow(clippy::string_slice)] // a '\n' byte offset is a char boundary
            let head = &truncated[..idx];
            (head.trim().to_string(), true)
        }
        None => (
            "... [First line would be too large to fit within character budget] ...".to_string(),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_units() {
        let cases = [
            (0, "0 B"),
            (512, "512 B"),
            (1024, "1.0 KB"),
            (1536, "1.5 KB"),
            (1_048_575, "1.0 MB"),
            (1 << 50, "1.0 PB"),
            (u64::MAX, "16.0 EB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format_bytes(bytes), expected, "bytes = {bytes}");
        }
    }

    // ---- estimate_tokens ----

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_four_bytes() {
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn estimate_tokens_rounds_down() {
        assert_eq!(estimate_tokens("abc"), 0);
        assert_eq!(estimate_tokens("abcdefg"), 1);
    }

    #[test]
    fn estimate_tokens_large() {
        assert_eq!(estimate_tokens(&"x".repeat(20_000)), 5_000);
    }

    // ---- truncate_line ----

    #[test]
    fn truncate_short_line_borrowed() {
        let r = truncate_line("hello", 2_000);
        assert!(matches!(r, Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_exact_limit_not_truncated() {
        let line = "a".repeat(2_000);
        assert!(matches!(truncate_line(&line, 2_000), Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_over_limit() {
        let line = "a".repeat(3_000);
        let r = truncate_line(&line, 2_000);
        assert!(r.contains("[... truncated (3000 chars total)]"));
        assert_eq!(r.split(" [... truncated").next().unwrap().len(), 2_000);
    }

    #[test]
    fn truncate_utf8_safe() {
        let line = "😀".repeat(2_001);
        let r = truncate_line(&line, 2_000);
        assert_eq!(
            r.split(" [... truncated").next().unwrap().chars().count(),
            2_000
        );
    }

    #[test]
    fn truncate_multibyte_char_count_under() {
        let line = "é".repeat(1_999);
        assert!(matches!(truncate_line(&line, 2_000), Cow::Borrowed(_)));
    }

    // ---- soft_wrap_line ----

    #[test]
    fn wrap_short_line_borrowed() {
        assert!(matches!(soft_wrap_line("hello", 2_000), Cow::Borrowed(_)));
    }

    #[test]
    fn wrap_preserves_all_content() {
        let line = "a".repeat(5_000);
        let r = soft_wrap_line(&line, 2_000);
        assert!(!r.contains("truncated"));
        let unwrapped: String = r.chars().filter(|c| *c != '\n').collect();
        assert_eq!(unwrapped.len(), 5_000);
    }

    #[test]
    fn wrap_inserts_newlines_correctly() {
        let line = "a".repeat(5_000);
        let r = soft_wrap_line(&line, 2_000);
        let lines: Vec<&str> = r.split('\n').collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), 2_000);
        assert_eq!(lines[1].len(), 2_000);
        assert_eq!(lines[2].len(), 1_000);
    }

    #[test]
    fn wrap_utf8_safe() {
        let line = "😀".repeat(3_000);
        let r = soft_wrap_line(&line, 2_000);
        let lines: Vec<&str> = r.split('\n').collect();
        assert_eq!(lines[0].chars().count(), 2_000);
        assert_eq!(lines[1].chars().count(), 1_000);
    }

    // ---- truncate_str ----

    #[test]
    fn truncate_str_returns_original_when_fits() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("", 0), "");
    }

    #[test]
    fn truncate_str_ascii_exact_boundary() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn truncate_str_does_not_split_cjk() {
        // "日" is a few bytes (0xE6 0x97 0xA5).
        assert_eq!(truncate_str("日本語", 2), "");
        assert_eq!(truncate_str("日本語", 3), "日");
        assert_eq!(truncate_str("日本語", 5), "日");
        assert_eq!(truncate_str("日本語", 6), "日本");
    }

    #[test]
    fn truncate_str_does_not_split_emoji() {
        // 🚀 is a few bytes.
        assert_eq!(truncate_str("🚀🦀", 3), "");
        assert_eq!(truncate_str("🚀🦀", 4), "🚀");
        assert_eq!(truncate_str("🚀🦀", 7), "🚀");
        assert_eq!(truncate_str("🚀🦀", 8), "🚀🦀");
    }

    #[test]
    fn truncate_str_zero_budget_gives_empty() {
        assert_eq!(truncate_str("hello", 0), "");
        assert_eq!(truncate_str("日本語", 0), "");
    }

    // ---- truncate_str_with_marker ----

    #[test]
    fn truncate_with_marker_exact_boundary_no_marker() {
        // len == max_bytes: still fits, no marker.
        let r = truncate_str_with_marker("hello", 5);
        assert!(matches!(r, Cow::Borrowed(_)));
        assert_eq!(r, "hello");
    }

    #[test]
    fn truncate_with_marker_appends_marker_when_cut() {
        let r = truncate_str_with_marker("hello world", 10);
        assert_eq!(r, "hello w…");
        assert!(r.len() <= 10);
        assert!(matches!(r, Cow::Owned(_)));
    }

    #[test]
    fn truncate_with_marker_respects_utf8_boundary() {
        // "日" is a few bytes.
        let r = truncate_str_with_marker("日本語abc", 10);
        assert_eq!(r, "日本…");
        assert!(r.len() <= 10);
        // Result is valid UTF-8 (chars: 日, 本, …).
        assert_eq!(r.chars().count(), 3);
    }

    // ---- truncate_with_preview ----

    #[test]
    fn truncate_with_preview_short_output_unchanged() {
        let (result, truncated) = truncate_with_preview(PartialOutput::whole("hello"), 10, 5, None);
        assert_eq!(result, "hello");
        assert!(!truncated);
    }

    #[test]
    fn truncate_with_preview_caps_large_output() {
        let output = "x".repeat(5_000_000);
        let (result, truncated) =
            truncate_with_preview(PartialOutput::whole(&output), 4_000, 2_000, None);

        assert!(truncated);
        assert!(result.len() < 2_200, "result was {} bytes", result.len());
        assert!(result.starts_with(&"x".repeat(2_000)));
        assert!(result.contains("[Output truncated - 5000000 bytes total]"));
    }

    #[test]
    fn truncate_with_preview_utf8_boundary() {
        let output = "😀".repeat(1_500);
        let (result, truncated) =
            truncate_with_preview(PartialOutput::whole(&output), 4_000, 2_001, None);

        assert!(truncated);
        assert!(result.starts_with(&"😀".repeat(500)));
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_with_preview_with_footer_hint() {
        let output = "x".repeat(10_000);
        let (result, truncated) = truncate_with_preview(
            PartialOutput::whole(&output),
            4_000,
            2_000,
            Some("Use read_file for full content"),
        );

        assert!(truncated);
        assert!(result.contains("Use read_file for full content"));
    }

    /// A partial copy always states the size of the output it came from,
    /// whether or not the text on hand needed cutting.
    #[test]
    fn a_partial_copy_always_states_the_real_size() {
        // The text fits the limit: kept whole, footer added.
        let (result, truncated) = truncate_with_preview(
            PartialOutput::part_of("held", 5_000_000),
            4_000,
            2_000,
            Some("Use read_file for full content"),
        );
        assert!(truncated);
        assert!(result.starts_with("held"), "{result}");
        assert!(result.contains("5000000 bytes total"), "{result}");
        assert!(
            result.contains("Use read_file for full content"),
            "{result}"
        );

        // The text is over the limit: cut, and the footer keeps the total.
        let held = "x".repeat(10_000);
        let (result, truncated) =
            truncate_with_preview(PartialOutput::part_of(&held, 5_000_000), 4_000, 2_000, None);
        assert!(truncated);
        assert!(result.contains("5000000 bytes total"), "{result}");
    }

    #[test]
    fn truncate_with_preview_without_footer_hint() {
        let output = "x".repeat(10_000);
        let (result, truncated) =
            truncate_with_preview(PartialOutput::whole(&output), 4_000, 2_000, None);

        assert!(truncated);
        assert!(result.contains("[Output truncated - 10000 bytes total]"));
        assert!(!result.contains("full content"));
    }

    // ---- soft_wrap_lines ----

    #[test]
    fn wrap_lines_mixed() {
        let text = format!("short\n{}\nanother", "x".repeat(5_000));
        let result = soft_wrap_lines(&text, 2_000);
        let lines: Vec<&str> = result.split('\n').collect();
        assert_eq!(lines[0], "short");
        assert_eq!(lines[1].len(), 2_000); // first chunk of wrapped line
        assert_eq!(lines[4], "another");
        // Total content preserved
        let unwrapped: String = result.chars().filter(|c| *c != '\n').collect();
        let original: String = text.chars().filter(|c| *c != '\n').collect();
        assert_eq!(unwrapped, original);
    }

    #[test]
    fn wrap_lines_preserves_trailing_newline() {
        assert_eq!(soft_wrap_lines("hello\n", 2_000), "hello\n");
        assert_eq!(soft_wrap_lines("hello", 2_000), "hello");
    }

    // ---- truncate_front_and_back ----

    #[test]
    fn front_and_back_short_string_not_truncated() {
        let (result, truncated) = truncate_front_and_back("hello world", 100);
        assert_eq!(result, "hello world");
        assert!(!truncated);
    }

    #[test]
    fn front_and_back_keeps_both_ends() {
        let s = "a".repeat(100);
        let (result, truncated) = truncate_front_and_back(&s, 20);
        assert!(truncated);
        assert!(result.starts_with("aaaaaaaaaa"));
        assert!(result.ends_with("aaaaaaaaaa"));
        assert!(result.contains("... (output truncated) ..."));
    }

    #[test]
    fn front_and_back_exact_boundary() {
        let s = "a".repeat(20);
        let (result, truncated) = truncate_front_and_back(&s, 20);
        assert_eq!(result, s);
        assert!(!truncated);
    }

    // ---- truncate_lines_to_char_budget ----

    #[test]
    fn lines_budget_short_content_not_truncated() {
        let (result, truncated) = truncate_lines_to_char_budget("line1\nline2\nline3", 100);
        assert_eq!(result, "line1\nline2\nline3");
        assert!(!truncated);
    }

    #[test]
    fn lines_budget_truncates_at_line_boundary() {
        let content = "short\nmedium line\nthis is a longer line\nand another";
        let (result, truncated) = truncate_lines_to_char_budget(content, 25);
        assert!(truncated);
        assert!(!result.contains("this is a longer"));
        // Should end at a complete line
        assert!(result.ends_with("medium line") || result.ends_with("short"));
    }

    #[test]
    fn lines_budget_single_huge_line() {
        let content = "a".repeat(1000);
        let (result, truncated) = truncate_lines_to_char_budget(&content, 50);
        assert!(truncated);
        assert!(result.contains("character budget"));
    }

    // ---- truncate_bytes ----

    /// The budget lands inside the character; the prefix must stop before it.
    #[test]
    fn truncate_bytes_em_dash_straddling_the_offset() {
        let msg = format!("{}—{}", "a".repeat(199), "b".repeat(20));
        assert!(!msg.is_char_boundary(200), "budget must land mid-em-dash");

        let prefix = truncate_bytes(&msg, 200);

        assert_eq!(prefix, "a".repeat(199));
        assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
        assert!(msg.is_char_boundary(prefix.len()));
    }

    /// Same shape, one CJK character straddling the budget.
    #[test]
    fn truncate_bytes_cjk_straddling_the_offset() {
        let text = format!("{}日本{}", "a".repeat(199), "b".repeat(20));
        assert!(!text.is_char_boundary(200), "budget must land mid-日");

        let prefix = truncate_bytes(&text, 200);

        assert_eq!(prefix, "a".repeat(199));
        assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
        assert!(text.is_char_boundary(prefix.len()));
    }

    /// Every offset across a string holding one character of each UTF-8 width:
    /// 1-byte ASCII, a 3-byte em dash, a 3-byte CJK ideograph, a 4-byte emoji.
    #[test]
    fn truncate_bytes_table_of_offsets_across_multibyte_chars() {
        let s = "a—b日🚀c";
        let cases: &[(usize, &str)] = &[
            (0, ""),
            (1, "a"),
            (2, "a"), // mid —
            (3, "a"), // mid —
            (4, "a—"),
            (5, "a—b"),
            (6, "a—b"), // mid 日
            (7, "a—b"), // mid 日
            (8, "a—b日"),
            (9, "a—b日"),  // mid 🚀
            (10, "a—b日"), // mid 🚀
            (11, "a—b日"), // mid 🚀
            (12, "a—b日🚀"),
            (13, "a—b日🚀c"),
        ];
        for &(budget, expected) in cases {
            let prefix = truncate_bytes(s, budget);
            assert_eq!(prefix, expected, "budget {budget}");
            assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
            assert!(
                s.is_char_boundary(prefix.len()),
                "budget {budget} cut a char"
            );
        }
    }

    #[test]
    fn truncate_bytes_never_panics_for_any_offset() {
        let s = "a—b日🚀c".repeat(20);
        for budget in 0..=s.len() + 64 {
            let prefix = truncate_bytes(&s, budget);
            assert!(s.starts_with(prefix), "budget {budget} is not a prefix");
            assert!(
                s.is_char_boundary(prefix.len()),
                "budget {budget} cut a char"
            );
            assert!(prefix.len() <= budget.min(s.len()));
        }
    }

    #[test]
    fn truncate_bytes_whole_and_short_cases() {
        assert_eq!(truncate_bytes("hello", 10), "hello");
        assert_eq!(truncate_bytes("hello", 5), "hello");
        assert_eq!(truncate_bytes("hello", 0), "");
        assert_eq!(truncate_bytes("", 0), "");
        assert_eq!(truncate_bytes("", 100), "");
        assert_eq!(truncate_bytes("日本語", 1000), "日本語");
        assert_eq!(truncate_bytes("日本語", 2), "");
        assert_eq!(truncate_bytes("日本語", 3), "日");
        assert_eq!(truncate_bytes("🚀🦀", 7), "🚀");
    }

    // ---- tail_bytes ----

    /// The same crash seen from the other end: the last several bytes of a
    /// string whose character straddles the cut.
    #[test]
    fn tail_bytes_em_dash_straddling_the_offset() {
        let text = format!("{}—{}", "a".repeat(20), "b".repeat(20));
        assert!(
            !text.is_char_boundary(text.len() - 22),
            "cut lands mid-em-dash"
        );

        let suffix = tail_bytes(&text, 22);

        assert_eq!(suffix, "b".repeat(20));
        assert!(suffix.len() <= 22);
        assert!(std::str::from_utf8(suffix.as_bytes()).is_ok());
    }

    #[test]
    fn tail_bytes_cjk_straddling_the_offset() {
        let text = format!("{}日本{}", "a".repeat(10), "b".repeat(4));
        assert!(!text.is_char_boundary(text.len() - 9), "cut lands mid-日");

        let suffix = tail_bytes(&text, 9);

        assert_eq!(suffix, format!("本{}", "b".repeat(4)));
        assert!(text.ends_with(suffix));
        assert!(suffix.len() <= 9);
        assert!(std::str::from_utf8(suffix.as_bytes()).is_ok());
    }

    #[test]
    fn tail_bytes_never_panics_for_any_budget() {
        let s = "a—b日🚀c".repeat(8);
        for budget in 0..=s.len() + 64 {
            let suffix = tail_bytes(&s, budget);
            assert!(s.ends_with(suffix), "budget {budget} is not a suffix");
            assert!(s.is_char_boundary(s.len() - suffix.len()));
            assert!(suffix.len() <= budget);
        }
    }

    #[test]
    fn tail_bytes_whole_and_short_cases() {
        assert_eq!(tail_bytes("hello", 10), "hello");
        assert_eq!(tail_bytes("hello", 5), "hello");
        assert_eq!(tail_bytes("hello", 0), "");
        assert_eq!(tail_bytes("", 0), "");
        assert_eq!(tail_bytes("hello", 2), "lo");
        assert_eq!(tail_bytes("日本", 3), "本");
        assert_eq!(tail_bytes("日本", 1), "");
        // 🚀 is a few bytes: it does not fit a 3-byte tail, and the boundary snaps past it rather than over the budget.
        assert_eq!(tail_bytes("a🚀", 3), "");
        assert_eq!(tail_bytes("a🚀", 4), "🚀");
    }

    // ---- floor_char_boundary / ceil_char_boundary ----

    #[test]
    fn floor_boundary_ascii() {
        assert_eq!(floor_char_boundary("hello", 3), 3);
    }

    #[test]
    fn floor_boundary_mid_cjk() {
        // "日" = 3 bytes.
        assert_eq!(floor_char_boundary("日本", 1), 0);
        assert_eq!(floor_char_boundary("日本", 2), 0);
        assert_eq!(floor_char_boundary("日本", 3), 3);
    }

    #[test]
    fn floor_boundary_past_end() {
        assert_eq!(floor_char_boundary("hi", 100), 2);
    }

    #[test]
    fn ceil_boundary_ascii() {
        assert_eq!(ceil_char_boundary("hello", 3), 3);
    }

    #[test]
    fn ceil_boundary_mid_cjk() {
        // "日" = 3 bytes.
        assert_eq!(ceil_char_boundary("日本", 1), 3);
        assert_eq!(ceil_char_boundary("日本", 2), 3);
        assert_eq!(ceil_char_boundary("日本", 3), 3);
    }

    #[test]
    fn ceil_boundary_past_end() {
        assert_eq!(ceil_char_boundary("hi", 100), 2);
    }

    // ---- truncate_middle ----

    #[test]
    fn truncate_middle_short_string_unchanged() {
        assert_eq!(truncate_middle("hello", 10), "hello");
    }

    #[test]
    fn truncate_middle_exact_limit_unchanged() {
        let s = "a".repeat(20);
        assert_eq!(truncate_middle(&s, 20), s);
    }

    #[test]
    fn truncate_middle_keeps_both_ends() {
        let s = "abcdefghijklmnopqrstuvwxyz";
        let result = truncate_middle(s, 10);
        assert!(result.starts_with("abc"));
        assert!(result.ends_with("wxyz"));
        assert!(result.contains("..."));
        // Total must not exceed budget.
        assert!(
            result.chars().count() <= 10,
            "result exceeds budget: {} chars: {result}",
            result.chars().count()
        );
    }

    #[test]
    fn truncate_middle_respects_budget() {
        let s = "a".repeat(50_000);
        let result = truncate_middle(&s, 12_000);
        assert_eq!(
            result.chars().count(),
            12_000,
            "result should be exactly the budget"
        );
        assert!(result.contains("..."));
    }

    #[test]
    fn truncate_middle_utf8_safe() {
        let s = "😀".repeat(100);
        let result = truncate_middle(&s, 10);
        assert_eq!(result.chars().count(), 10);
        assert!(result.contains("..."));
    }
}
