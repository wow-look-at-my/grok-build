//! The live tail of a tool call's arguments, as the model writes them.
//!
//! A call's arguments arrive as raw JSON fragments split at arbitrary byte
//! offsets. Showing them verbatim puts `\n` and `\"` on screen and renders a
//! file write as one enormous line. This decodes the escapes as the fragments
//! land and keeps the last few lines, so the row shows what the model is
//! typing right now.
//!
//! The tail is bounded in both directions. Older lines are dropped, and a
//! single line stops growing past [`MAX_LINE_CHARS`], so a multi-megabyte
//! write costs a fixed amount however long it runs.

use std::collections::VecDeque;

/// How many trailing lines the preview keeps.
pub const MAX_TAIL_LINES: usize = 5;

/// How many characters one preview line keeps. The row is one terminal wide,
/// so anything past this is never drawn.
const MAX_LINE_CHARS: usize = 512;

/// How wide a tab is rendered. Ratatui draws a literal tab as one cell, which
/// collapses indented code into a ragged column.
const TAB_WIDTH: usize = 4;

/// What the decoder is waiting for in the middle of an escape sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    /// Not inside an escape.
    None,
    /// A `\` arrived and its escape character has not.
    Backslash,
    /// A `\u` arrived; `len` of the four hex digits are in `hex`.
    Unicode { hex: [u8; 4], len: usize },
}

/// A rolling, escape-decoded tail of a tool call's argument text.
#[derive(Debug)]
pub struct StreamingArgsTail {
    /// The trailing decoded lines, oldest first, newest last.
    lines: VecDeque<String>,
    /// Where an escape sequence split across two fragments left off.
    escape: EscapeState,
    /// A lone high surrogate waiting for its pair.
    pending_surrogate: Option<u16>,
}

impl Default for StreamingArgsTail {
    fn default() -> Self {
        Self {
            lines: VecDeque::from(vec![String::new()]),
            escape: EscapeState::None,
            pending_surrogate: None,
        }
    }
}

impl StreamingArgsTail {
    /// Take one raw argument fragment and fold it into the tail.
    pub fn push(&mut self, delta: &str) {
        for ch in delta.chars() {
            self.push_char(ch);
        }
    }

    /// The decoded tail, oldest line first. A trailing empty line is kept: it
    /// is where the next character goes, and dropping it makes a finished line
    /// look like it is still being written.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// Whether anything has been decoded yet.
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(String::is_empty)
    }

    fn push_char(&mut self, ch: char) {
        match self.escape {
            EscapeState::None => {
                if ch == '\\' {
                    self.escape = EscapeState::Backslash;
                    return;
                }
                self.emit(ch);
            }
            EscapeState::Backslash => {
                self.escape = EscapeState::None;
                match ch {
                    'n' => self.newline(),
                    't' => self.emit_tab(),
                    'r' => {}
                    'b' | 'f' => {}
                    'u' => {
                        self.escape = EscapeState::Unicode {
                            hex: [0; 4],
                            len: 0,
                        }
                    }
                    // `\"`, `\\`, `\/` and anything a provider invents: the
                    // character itself is what the model wrote.
                    other => self.emit(other),
                }
            }
            EscapeState::Unicode { mut hex, len } => {
                if !ch.is_ascii_hexdigit() {
                    // Not a `\uXXXX` after all. Nothing sane to decode, so drop
                    // the sequence and take this character as ordinary text.
                    self.escape = EscapeState::None;
                    self.pending_surrogate = None;
                    self.push_char(ch);
                    return;
                }
                hex[len] = ch as u8;
                let len = len + 1;
                if len < 4 {
                    self.escape = EscapeState::Unicode { hex, len };
                    return;
                }
                self.escape = EscapeState::None;
                let text = std::str::from_utf8(&hex).unwrap_or("");
                let Ok(unit) = u16::from_str_radix(text, 16) else {
                    return;
                };
                self.emit_utf16_unit(unit);
            }
        }
    }

    /// Fold one UTF-16 code unit in, pairing surrogates across fragments.
    fn emit_utf16_unit(&mut self, unit: u16) {
        if let Some(high) = self.pending_surrogate.take() {
            let pair = [high, unit];
            if let Some(Ok(ch)) = char::decode_utf16(pair).next() {
                self.emit(ch);
                return;
            }
            // The pair did not decode. Fall through and treat `unit` on its own.
        }
        if (0xD800..0xDC00).contains(&unit) {
            self.pending_surrogate = Some(unit);
            return;
        }
        if let Some(ch) = char::from_u32(u32::from(unit)) {
            self.emit(ch);
        }
    }

    fn emit(&mut self, ch: char) {
        if ch == '\n' {
            self.newline();
            return;
        }
        if ch == '\t' {
            self.emit_tab();
            return;
        }
        // A control character has no width and can move the cursor, so it is
        // never drawn. `\r` included: it arrives inside CRLF content.
        if ch.is_control() {
            return;
        }
        let Some(line) = self.lines.back_mut() else {
            return;
        };
        if line.chars().count() >= MAX_LINE_CHARS {
            return;
        }
        line.push(ch);
    }

    fn emit_tab(&mut self) {
        for _ in 0..TAB_WIDTH {
            self.emit(' ');
        }
    }

    fn newline(&mut self) {
        self.lines.push_back(String::new());
        while self.lines.len() > MAX_TAIL_LINES {
            self.lines.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_of(fragments: &[&str]) -> Vec<String> {
        let mut tail = StreamingArgsTail::default();
        for fragment in fragments {
            tail.push(fragment);
        }
        tail.lines().map(str::to_string).collect()
    }

    #[test]
    fn an_escaped_newline_starts_a_line() {
        assert_eq!(
            tail_of(&[r#"{"a":"one\ntwo"}"#]),
            vec![r#"{"a":"one"#.to_string(), r#"two"}"#.to_string(),]
        );
    }

    #[test]
    fn an_escape_split_across_fragments_still_decodes() {
        // The fragment boundary lands between the backslash and the `n`.
        assert_eq!(
            tail_of(&[r#"{"a":"one\"#, r#"ntwo"}"#]),
            vec![r#"{"a":"one"#.to_string(), r#"two"}"#.to_string(),]
        );
    }

    #[test]
    fn an_escaped_quote_reads_as_a_quote() {
        assert_eq!(
            tail_of(&[r#"{"a":"say \"hi\""}"#]),
            vec![r#"{"a":"say "hi""}"#.to_string()]
        );
    }

    #[test]
    fn a_unicode_escape_split_across_fragments_still_decodes() {
        assert_eq!(tail_of(&[r#"x\u00"#, r#"e9y"#]), vec!["xéy".to_string()]);
    }

    #[test]
    fn a_surrogate_pair_decodes_to_one_character() {
        assert_eq!(tail_of(&[r#"😀"#]), vec!["😀".to_string()]);
    }

    #[test]
    fn only_the_last_lines_are_kept() {
        let many = (0..40).map(|i| format!("line{i}")).collect::<Vec<_>>();
        let fragment = many.join(r"\n");
        let lines = tail_of(&[&fragment]);
        assert_eq!(lines.len(), MAX_TAIL_LINES);
        assert_eq!(lines.last().map(String::as_str), Some("line39"));
    }

    #[test]
    fn one_line_stops_growing_at_the_cap() {
        let huge = "a".repeat(MAX_LINE_CHARS * 3);
        let lines = tail_of(&[&huge]);
        assert_eq!(lines[0].chars().count(), MAX_LINE_CHARS);
    }

    #[test]
    fn a_tab_becomes_spaces_and_a_control_character_is_dropped() {
        assert_eq!(tail_of(&[r#"a\tb\u0007c"#]), vec!["a    bc".to_string()]);
    }

    #[test]
    fn a_carriage_return_never_reaches_the_screen() {
        assert_eq!(
            tail_of(&[r#"a\r\nb"#]),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn a_fresh_tail_is_empty() {
        assert!(StreamingArgsTail::default().is_empty());
        let mut tail = StreamingArgsTail::default();
        tail.push("x");
        assert!(!tail.is_empty());
    }
}
