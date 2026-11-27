//! `get_task_output` views over a saved log: a pipeline stage, a head, a tail and a grep.

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::implementations::grok_build::bash::command_plan::{stage_file, stage_status_file};
use crate::util::truncate::{format_bytes, tail_bytes, truncate_str};

/// What to show of one log.
#[derive(Debug, Clone, Default)]
pub(crate) struct LogView {
    pub(crate) head: Option<usize>,
    pub(crate) tail: Option<usize>,
    pub(crate) grep: Option<Regex>,
}

/// The lines a [`LogView`] selected, with the counts its header reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewText {
    pub(crate) body: String,
    pub(crate) total_lines: usize,
    pub(crate) matched: Option<usize>,
    pub(crate) shown: usize,
    pub(crate) first_line: Option<usize>,
}

impl LogView {
    /// Read `path` line by line and keep only what the view asks for.
    pub(crate) fn read(&self, path: &Path) -> std::io::Result<ViewText> {
        let file = std::fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(file);
        let mut buf = Vec::new();
        let mut total_lines = 0usize;
        let mut matched = 0usize;
        let mut head: Vec<(usize, String)> = Vec::new();
        let mut tail: VecDeque<(usize, String)> = VecDeque::new();
        let head_cap = self.head.unwrap_or(usize::MAX);
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            total_lines += 1;
            let raw = String::from_utf8_lossy(&buf);
            let line = strip_ansi_escapes::strip_str(raw.trim_end_matches(['\n', '\r']));
            if let Some(re) = &self.grep
                && !re.is_match(&line)
            {
                continue;
            }
            matched += 1;
            match self.tail {
                Some(n) if self.head.is_none() => {
                    if n == 0 {
                        continue;
                    }
                    if tail.len() == n {
                        tail.pop_front();
                    }
                    tail.push_back((total_lines, line));
                }
                _ => {
                    if head.len() < head_cap {
                        head.push((total_lines, line));
                    }
                }
            }
        }
        let mut picked: Vec<(usize, String)> = if self.tail.is_some() && self.head.is_none() {
            tail.into_iter().collect()
        } else {
            head
        };
        if let (Some(_), Some(n)) = (self.head, self.tail) {
            // head then tail: the last N of the first M.
            let skip = picked.len().saturating_sub(n);
            picked.drain(..skip);
        }
        let numbered = self.grep.is_some();
        let mut body = String::new();
        for (n, line) in &picked {
            if numbered {
                body.push_str(&format!("{n}: "));
            }
            body.push_str(line);
            body.push('\n');
        }
        Ok(ViewText {
            body,
            total_lines,
            matched: self.grep.is_some().then_some(matched),
            shown: picked.len(),
            first_line: picked.first().map(|(n, _)| *n),
        })
    }

    /// One line that says which part of the log the body holds.
    pub(crate) fn header(&self, text: &ViewText) -> String {
        let of = match (&self.grep, text.matched) {
            (Some(re), Some(m)) => {
                format!("{m} of {} lines match /{}/", text.total_lines, re.as_str())
            }
            _ => format!("{} lines", text.total_lines),
        };
        match (text.first_line, self.grep.is_some()) {
            (None, _) => format!("[{of}; nothing to show]"),
            (Some(_), true) => format!("[{of}; showing {}]", text.shown),
            (Some(first), false) => {
                format!(
                    "[lines {first}-{} of {of}]",
                    first + text.shown.saturating_sub(1)
                )
            }
        }
    }

    /// True when the reader wants the end of the log kept over the start.
    pub(crate) fn keeps_end(&self) -> bool {
        self.tail.is_some() && self.head.is_none()
    }
}

/// Cut `body` to `max_bytes`, keeping its end for a tail view and its start
/// for anything else.
pub(crate) fn fit_body(body: &str, max_bytes: usize, keep_end: bool) -> (String, bool) {
    if body.len() <= max_bytes {
        return (body.to_string(), false);
    }
    let cut = format!(
        "[{} of view output cut to fit; narrow it with head, tail or grep]",
        format_bytes((body.len() - max_bytes) as u64)
    );
    if keep_end {
        (format!("{cut}\n{}", tail_bytes(body, max_bytes)), true)
    } else {
        (format!("{}\n{cut}", truncate_str(body, max_bytes)), true)
    }
}

/// The log file for `task_id`: the one a live task names, or the foreground
/// log `run_terminal_cmd` writes under the session folder.
pub(crate) fn foreground_log(session_folder: &Path, task_id: &str) -> Option<PathBuf> {
    // The id comes from the model. It must name a file in the log folder and
    // nothing outside it.
    let plain =
        !task_id.is_empty() && !task_id.starts_with('.') && !task_id.contains(['/', '\\', '\0']);
    plain.then(|| {
        session_folder
            .join("terminal")
            .join(format!("{task_id}.log"))
    })
}

/// The stage numbers that have a saved file next to `log`.
pub(crate) fn saved_stages(log: &Path) -> Vec<usize> {
    (1..).take_while(|&n| stage_file(log, n).exists()).collect()
}

/// Exit codes of the stages the model wrote, in order. The status file holds
/// one code per process, and every stage but the last is followed by its tee.
pub(crate) fn stage_exit_codes(log: &Path) -> Vec<i32> {
    let Ok(text) = std::fs::read_to_string(stage_status_file(log)) else {
        return Vec::new();
    };
    let codes: Vec<i32> = text.lines().filter_map(|l| l.trim().parse().ok()).collect();
    codes.iter().step_by(2).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, lines.join("\n").as_bytes()).unwrap();
        f
    }

    #[test]
    fn tail_keeps_the_last_lines_of_the_whole_file() {
        let lines: Vec<String> = (1..=100).map(|n| format!("line {n}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let f = log(&refs);
        let view = LogView {
            tail: Some(3),
            ..Default::default()
        };
        let text = view.read(f.path()).unwrap();
        assert_eq!(text.body, "line 98\nline 99\nline 100\n");
        assert_eq!(view.header(&text), "[lines 98-100 of 100 lines]");
    }

    #[test]
    fn head_and_tail_together_show_the_end_of_the_head() {
        let f = log(&["a", "b", "c", "d", "e"]);
        let view = LogView {
            head: Some(4),
            tail: Some(2),
            ..Default::default()
        };
        assert_eq!(view.read(f.path()).unwrap().body, "c\nd\n");
    }

    #[test]
    fn grep_numbers_matches_and_strips_colour() {
        let f = log(&[
            "ok 1",
            "\u{1b}[31mFAILED\u{1b}[0m test_a",
            "ok 2",
            "FAILED test_b",
        ]);
        let view = LogView {
            grep: Some(Regex::new("FAIL").unwrap()),
            tail: Some(1),
            ..Default::default()
        };
        let text = view.read(f.path()).unwrap();
        assert_eq!(text.body, "4: FAILED test_b\n");
        assert_eq!(view.header(&text), "[2 of 4 lines match /FAIL/; showing 1]");
    }

    #[test]
    fn fit_body_keeps_the_end_of_a_tail() {
        let (body, cut) = fit_body("0123456789", 4, true);
        assert!(cut);
        assert!(body.ends_with("\n6789"), "{body}");
        let (body, _) = fit_body("0123456789", 4, false);
        assert!(body.starts_with("0123\n"), "{body}");
    }

    #[test]
    fn a_task_id_cannot_leave_the_log_folder() {
        let root = Path::new("/s");
        assert_eq!(
            foreground_log(root, "call_1"),
            Some(PathBuf::from("/s/terminal/call_1.log"))
        );
        assert_eq!(foreground_log(root, "../x"), None);
        assert_eq!(foreground_log(root, "a/b"), None);
        assert_eq!(foreground_log(root, ""), None);
    }

    #[test]
    fn stage_codes_skip_the_tee_processes() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("c.log");
        std::fs::write(stage_status_file(&log), "101\n0\n1\n0\n0\n").unwrap();
        assert_eq!(stage_exit_codes(&log), vec![101, 1, 0]);
    }
}
