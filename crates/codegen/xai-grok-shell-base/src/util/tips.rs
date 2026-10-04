//! Tip of the Day: selection logic for tips served from remote settings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CURSOR_FILE: &str = "tip_cursor.json";

/// Persistent state for tip rotation.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TipState {
    cursor: u64,
}

fn cursor_path(grok_home: &Path) -> PathBuf {
    grok_home.join(CURSOR_FILE)
}

/// Load the cursor from `~/.grok/tip_cursor.json`.
fn load_cursor(grok_home: &Path) -> u64 {
    let text = match std::fs::read_to_string(cursor_path(grok_home)) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    serde_json::from_str::<TipState>(&text)
        .map(|s| s.cursor)
        .unwrap_or(0)
}

/// Save the cursor to `~/.grok/tip_cursor.json`. Silently ignores write errors.
fn save_cursor(grok_home: &Path, cursor: u64) {
    if let Ok(text) = serde_json::to_string(&TipState { cursor }) {
        let _ = std::fs::write(cursor_path(grok_home), text);
    }
}

/// Pick the next tip for this session and advance the persistent cursor. Each
/// call returns the tip at `cursor % tips.len()` and increments the cursor in
/// `~/.grok/tip_cursor.json`, so every session sees the next tip in sequence.
pub fn pick_and_advance(tips: &[String], grok_home: &Path) -> Option<String> {
    if tips.is_empty() {
        return None;
    }
    let cursor = load_cursor(grok_home);
    let tip = tips.get(cursor as usize % tips.len())?.clone();
    save_cursor(grok_home, cursor + 1);
    Some(tip)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── pick_and_advance ──────────────────────────────────────────────────────

    #[test]
    fn empty_list_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(pick_and_advance(&[], dir.path()), None);
    }

    #[test]
    fn empty_list_does_not_advance_cursor() {
        let dir = tempfile::tempdir().unwrap();
        pick_and_advance(&[], dir.path());
        assert_eq!(load_cursor(dir.path()), 0);
    }

    #[test]
    fn single_tip_always_returned() {
        let dir = tempfile::tempdir().unwrap();
        let tips = vec!["only".to_string()];
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("only"));
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("only"));
    }

    #[test]
    fn cycles_through_all_tips_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let tips = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("a"));
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("b"));
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("c"));
        // full cycle: wraps back to first
        assert_eq!(pick_and_advance(&tips, dir.path()).as_deref(), Some("a"));
    }

    #[test]
    fn cursor_persists_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let tips = vec!["x".to_string(), "y".to_string()];
        pick_and_advance(&tips, dir.path());
        assert_eq!(load_cursor(dir.path()), 1);
        pick_and_advance(&tips, dir.path());
        assert_eq!(load_cursor(dir.path()), 2);
    }

    #[test]
    fn missing_cursor_file_starts_at_zero() {
        let dir = tempfile::tempdir().unwrap();
        let tips = vec!["first".to_string(), "second".to_string()];
        assert_eq!(
            pick_and_advance(&tips, dir.path()).as_deref(),
            Some("first")
        );
    }

    #[test]
    fn handles_list_length_change_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let tips3 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        pick_and_advance(&tips3, dir.path());
        pick_and_advance(&tips3, dir.path());
        pick_and_advance(&tips3, dir.path());

        let tips5 = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
        ];
        assert_eq!(pick_and_advance(&tips5, dir.path()).as_deref(), Some("d"));
    }

    // ── load_cursor / save_cursor ─────────────────────────────────────────────

    #[test]
    fn load_cursor_returns_zero_for_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(cursor_path(dir.path()), b"not json").unwrap();
        assert_eq!(load_cursor(dir.path()), 0);
    }
}
