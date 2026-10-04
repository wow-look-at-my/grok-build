//! Layer 2a: Screen state tracking via `alacritty_terminal` (ptyctl).

use ptyctl::styled::StyledLine;
use ptyctl::term::{ScreenOpts, ScreenOutput, SessionListener, Terminal};

/// Tracks the virtual terminal screen state by feeding raw PTY output through
/// an `alacritty_terminal`-based headless terminal.
pub struct ScreenTracker {
    terminal: Terminal,
    /// Receives terminal-generated replies (cursor-position reports, device attributes, color queries, …) the emulator emits.
    pty_write_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
}

impl ScreenTracker {
    pub fn new(rows: u16, cols: u16) -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let listener = SessionListener::new(tx);
        Self {
            terminal: Terminal::new(cols, rows, listener),
            pty_write_rx: rx,
        }
    }

    /// Feed raw PTY output bytes into the terminal emulator.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.feed(bytes);
    }

    /// Must be written back or terminal probes hang, such as the inline viewport's startup cursor-position query.
    pub fn drain_responses(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Ok(bytes) = self.pty_write_rx.try_recv() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Return structured screen contents (no escape codes).
    pub fn output(&self) -> ScreenOutput {
        self.terminal.screen_content(&ScreenOpts::default())
    }

    /// Return the full text contents of the screen (no escape codes).
    pub fn contents(&self) -> String {
        self.output().lines.join("\n")
    }

    pub fn contains(&self, text: &str) -> bool {
        self.contents().contains(text)
    }

    /// Return the current cursor position as `(row, col)` (0-indexed, matching the original vt100 convention used by existing tests).
    pub fn cursor_position(&self) -> (u16, u16) {
        let pos = self.terminal.cursor_position();
        // ptyctl cursor is 1-indexed; the harness API is 0-indexed.
        (
            (pos.row as u16).saturating_sub(1),
            (pos.col as u16).saturating_sub(1),
        )
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.terminal.resize(cols, rows);
    }

    /// Return the full screen with style information for visual artifacts.
    pub fn styled(&self) -> Vec<StyledLine> {
        self.terminal.screen_styled(&ScreenOpts::default())
    }

    pub fn html(&self) -> String {
        self.terminal.screen_html(&ScreenOpts::default())
    }

    /// Access the underlying ptyctl `Terminal` for advanced queries (styled output, scrollback, terminal modes, etc.).
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// Number of lines in the terminal's scrollback history: content that has scrolled *above* the visible screen.
    pub fn scrollback_count(&self) -> usize {
        self.terminal.scrollback_count()
    }

    /// The full scrollback history as text, oldest line first.
    pub fn scrollback_text(&self) -> String {
        let n = self.terminal.scrollback_count();
        self.terminal
            .scrollback_lines(n)
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Scrollback history plus the visible screen, joined oldest to newest:
    /// everything a user could see by scrolling up.
    pub fn full_text(&self) -> String {
        let sb = self.scrollback_text();
        let screen = self.contents();
        if sb.is_empty() {
            screen
        } else {
            format!("{sb}\n{screen}")
        }
    }

    /// Whether scrollback plus the visible screen contains `text`.
    pub fn full_contains(&self, text: &str) -> bool {
        self.full_text().contains(text)
    }

    /// Native select→copy of scrollback plus screen: wrap-joined, pads trimmed.
    pub fn native_copy_text(&self) -> String {
        self.terminal.native_copy_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines pushed above a small screen must be readable via the scrollback helpers.
    #[test]
    fn scrolled_off_lines_are_captured_by_scrollback_helpers() {
        let mut s = ScreenTracker::new(3, 20);
        for i in 1..=8 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(
            !s.contains("line1"),
            "line1 should have scrolled off-screen"
        );
        assert!(s.scrollback_count() >= 5, "expected scrolled-off history");
        assert!(s.scrollback_text().contains("line1"));
        assert!(s.full_contains("line1"));
        assert!(s.full_contains("line8"));
    }

    /// A DSR cursor-position query (`ESC[6n`) must produce a forwardable reply (a CPR `ESC[<row>;<col>R`).
    /// Inline-viewport tests rely on this so the startup cursor query completes.
    #[test]
    fn drain_responses_answers_cursor_position_query() {
        let mut s = ScreenTracker::new(24, 80);
        // Nothing queued before any query is fed.
        assert!(s.drain_responses().is_empty());

        s.feed(b"\x1b[6n");
        let reply = s.drain_responses();
        assert!(
            reply.starts_with(b"\x1b[") && reply.ends_with(b"R"),
            "expected a cursor-position report, got {:?}",
            String::from_utf8_lossy(&reply)
        );

        // Drained exactly once; no duplicate delivery on the next call
        assert!(s.drain_responses().is_empty());
    }

    #[test]
    fn native_copy_joins_wrapped_rows() {
        let mut s = ScreenTracker::new(3, 4);
        s.feed(b"abcde\r\nxy\r\n");
        let copy = s.native_copy_text();
        assert!(copy.contains("abcde"), "WRAPLINE rows must join: {copy:?}");
        assert!(
            !copy.contains("abcd\ne"),
            "soft wrap must not become a hard break: {copy:?}"
        );
        assert!(copy.contains("xy"), "{copy:?}");
    }
}
