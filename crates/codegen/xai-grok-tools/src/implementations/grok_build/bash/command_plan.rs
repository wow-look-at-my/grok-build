//! Command shapes for the experimental split-and-tee mode.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Semi,
    Newline,
    And,
    Or,
    Background,
    Pipe,
    PipeAll,
    CaseEnd,
}

#[derive(Debug, Clone)]
struct Word {
    text: String,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
enum Tok {
    Word(Word),
    Op(Op),
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: src.char_indices().collect(),
            i: 0,
        }
    }

    fn peek(&self, off: usize) -> Option<char> {
        self.chars.get(self.i + off).map(|c| c.1)
    }

    fn byte(&self, i: usize) -> usize {
        self.chars.get(i).map_or(self.src.len(), |c| c.0)
    }

    fn tokens(mut self) -> Option<Vec<Tok>> {
        let mut out = Vec::new();
        loop {
            while let Some(c) = self.peek(0) {
                if c == ' ' || c == '\t' {
                    self.i += 1;
                } else if c == '\\' && self.peek(1) == Some('\n') {
                    self.i += 2;
                } else {
                    break;
                }
            }
            let Some(c) = self.peek(0) else { break };
            let next = self.peek(1);
            let op = match (c, next) {
                ('\n', _) => Some((Op::Newline, 1)),
                (';', Some(';')) => Some((Op::CaseEnd, 2)),
                (';', _) => Some((Op::Semi, 1)),
                ('&', Some('&')) => Some((Op::And, 2)),
                ('&', Some('>')) => None,
                ('&', _) => Some((Op::Background, 1)),
                ('|', Some('|')) => Some((Op::Or, 2)),
                ('|', Some('&')) => Some((Op::PipeAll, 2)),
                ('|', _) => Some((Op::Pipe, 1)),
                // A subshell, a group or a function definition.
                ('(' | ')', _) => return None,
                _ => None,
            };
            if let Some((op, len)) = op {
                out.push(Tok::Op(op));
                self.i += len;
                continue;
            }
            if c == '#' {
                while let Some(c) = self.peek(0) {
                    if c == '\n' {
                        break;
                    }
                    self.i += 1;
                }
                continue;
            }
            out.push(Tok::Word(self.word()?));
        }
        Some(out)
    }

    fn word(&mut self) -> Option<Word> {
        let start_i = self.i;
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | ';' | '(' | ')' => break,
                '&' => {
                    let prev = self
                        .i
                        .checked_sub(1)
                        .and_then(|p| self.chars.get(p))
                        .map(|c| c.1);
                    if matches!(prev, Some('>' | '<')) || self.peek(1) == Some('>') {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                '|' => {
                    let prev = self
                        .i
                        .checked_sub(1)
                        .and_then(|p| self.chars.get(p))
                        .map(|c| c.1);
                    if prev == Some('>') {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                '\\' => self.i += 2,
                '\'' => self.single_quoted()?,
                '"' => self.double_quoted()?,
                '`' => self.backticked()?,
                '$' => match self.peek(1) {
                    Some('(') => {
                        self.i += 1;
                        self.nested('(', ')')?;
                    }
                    Some('{') => {
                        self.i += 1;
                        self.nested('{', '}')?;
                    }
                    Some('\'') => {
                        self.i += 1;
                        self.ansi_quoted()?;
                    }
                    _ => self.i += 1,
                },
                '<' | '>' if self.peek(1) == Some('(') => {
                    self.i += 1;
                    self.nested('(', ')')?;
                }
                '<' if self.peek(1) == Some('<') => {
                    // `<<<` is a here-string and stays on its line. A heredoc
                    // body spans lines, so no line split is safe.
                    if self.peek(2) != Some('<') {
                        return None;
                    }
                    self.i += 3;
                }
                _ => self.i += 1,
            }
        }
        let end_i = self.i.min(self.chars.len());
        let (start, end) = (self.byte(start_i), self.byte(end_i));
        Some(Word {
            text: self.src[start..end].to_string(),
            start,
            end,
        })
    }

    fn single_quoted(&mut self) -> Option<()> {
        self.i += 1;
        loop {
            match self.peek(0)? {
                '\'' => {
                    self.i += 1;
                    return Some(());
                }
                _ => self.i += 1,
            }
        }
    }

    fn ansi_quoted(&mut self) -> Option<()> {
        self.i += 1;
        loop {
            match self.peek(0)? {
                '\\' => self.i += 2,
                '\'' => {
                    self.i += 1;
                    return Some(());
                }
                _ => self.i += 1,
            }
        }
    }

    fn backticked(&mut self) -> Option<()> {
        self.i += 1;
        loop {
            match self.peek(0)? {
                '\\' => self.i += 2,
                '`' => {
                    self.i += 1;
                    return Some(());
                }
                _ => self.i += 1,
            }
        }
    }

    fn double_quoted(&mut self) -> Option<()> {
        self.i += 1;
        loop {
            match self.peek(0)? {
                '\\' => self.i += 2,
                '"' => {
                    self.i += 1;
                    return Some(());
                }
                '`' => self.backticked()?,
                '$' if self.peek(1) == Some('(') => {
                    self.i += 1;
                    self.nested('(', ')')?;
                }
                '$' if self.peek(1) == Some('{') => {
                    self.i += 1;
                    self.nested('{', '}')?;
                }
                _ => self.i += 1,
            }
        }
    }

    /// Skip from `open` at the cursor to its matching `close`.
    fn nested(&mut self, open: char, close: char) -> Option<()> {
        let mut depth = 0usize;
        loop {
            let c = self.peek(0)?;
            match c {
                '\\' => {
                    self.i += 2;
                    continue;
                }
                '\'' => {
                    self.single_quoted()?;
                    continue;
                }
                '"' => {
                    self.double_quoted()?;
                    continue;
                }
                '`' => {
                    self.backticked()?;
                    continue;
                }
                _ => {}
            }
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    self.i += 1;
                    return Some(());
                }
            }
            self.i += 1;
        }
    }
}

/// Words that start a compound command or change how the rest of a line
/// parses. A stage that starts with one of these is never rewritten.
const COMPOUND_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "select", "function", "coproc", "{", "}",
];

/// Commands whose effect does not reach the next tool call. The shell keeps
/// only the cwd, exported variables, options, functions and aliases between
/// calls, so a split after one of these runs the rest in a different shell.
const STATEFUL_WORDS: &[&str] = &[
    "exec",
    "local",
    "read",
    "readonly",
    "declare",
    "typeset",
    "trap",
    "source",
    ".",
    "set",
    "exit",
    "return",
    "wait",
    "pushd",
    "popd",
    "dirs",
    "eval",
    "let",
    "ulimit",
    "umask",
    "shift",
    "getopts",
    "mapfile",
    "readarray",
    "builtin",
    "enable",
    "hash",
    "jobs",
    "fg",
    "bg",
    "disown",
];

fn is_assignment(word: &str) -> bool {
    let name_len = word
        .char_indices()
        .take_while(|&(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        .count();
    if name_len == 0 {
        return false;
    }
    let rest = &word[name_len..];
    let rest = if rest.starts_with('[') {
        match rest.find(']') {
            Some(close) => &rest[close + 1..],
            None => return false,
        }
    } else {
        rest
    };
    rest.starts_with('=') || rest.starts_with("+=")
}

/// The first word that is not a variable assignment.
fn command_word(words: &[Word]) -> Option<&str> {
    words
        .iter()
        .map(|w| w.text.as_str())
        .find(|w| !is_assignment(w))
}

struct Stage {
    words: Vec<Word>,
    /// The operator that joins this stage to the next one.
    pipe_after: Option<Op>,
}

impl Stage {
    fn text<'s>(&self, src: &'s str) -> &'s str {
        let start = self.words.first().map_or(0, |w| w.start);
        let end = self.words.last().map_or(0, |w| w.end);
        &src[start..end]
    }
}

struct Segment {
    stages: Vec<Stage>,
    after_and: bool,
}

impl Segment {
    fn text<'s>(&self, src: &'s str) -> &'s str {
        let start = self
            .stages
            .first()
            .and_then(|s| s.words.first())
            .map_or(0, |w| w.start);
        let end = self
            .stages
            .last()
            .and_then(|s| s.words.last())
            .map_or(0, |w| w.end);
        &src[start..end]
    }
}

/// Parse a command into `;`/newline/`&&`-joined segments of pipelines.
/// `None` means the command holds a construct this module does not handle.
fn parse(src: &str) -> Option<Vec<Segment>> {
    let toks = Lexer::new(src).tokens()?;
    let mut segments = Vec::new();
    let mut stages: Vec<Stage> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut after_and = false;
    let mut pending_and = false;

    let mut close_segment = |stages: &mut Vec<Stage>, words: &mut Vec<Word>, after_and: bool| {
        if !words.is_empty() {
            stages.push(Stage {
                words: std::mem::take(words),
                pipe_after: None,
            });
        }
        if stages.is_empty() {
            return;
        }
        segments.push(Segment {
            stages: std::mem::take(stages),
            after_and,
        });
    };

    for tok in toks {
        match tok {
            Tok::Word(w) => {
                if pending_and {
                    after_and = true;
                    pending_and = false;
                }
                words.push(w);
            }
            Tok::Op(Op::Pipe | Op::PipeAll) if words.is_empty() => return None,
            Tok::Op(op @ (Op::Pipe | Op::PipeAll)) => stages.push(Stage {
                words: std::mem::take(&mut words),
                pipe_after: Some(op),
            }),
            // A line break after `&&` or `|` continues the list.
            Tok::Op(Op::Newline) if pending_and || (words.is_empty() && !stages.is_empty()) => {}
            Tok::Op(op @ (Op::Semi | Op::Newline | Op::And)) => {
                if pending_and || (words.is_empty() && !stages.is_empty()) {
                    return None;
                }
                if op == Op::And && words.is_empty() {
                    return None;
                }
                close_segment(&mut stages, &mut words, after_and);
                after_and = false;
                pending_and = op == Op::And;
            }
            Tok::Op(Op::Or | Op::Background | Op::CaseEnd) => return None,
        }
    }
    if pending_and || (words.is_empty() && !stages.is_empty()) {
        return None;
    }
    close_segment(&mut stages, &mut words, after_and);

    for seg in &segments {
        for stage in &seg.stages {
            let first = stage.words.first().map(|w| w.text.as_str());
            if first.is_some_and(|w| COMPOUND_WORDS.contains(&w)) {
                return None;
            }
            if command_word(&stage.words).is_some_and(|w| COMPOUND_WORDS.contains(&w)) {
                return None;
            }
        }
    }
    Some(segments)
}

/// One command of a split list, in run order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSegment {
    pub command: String,
    /// Joined to the segment with `&&`: it runs only if the segment ran and succeeded.
    pub only_if_previous_succeeded: bool,
}

/// Split `a; b && c` into the commands a separate tool call can run each.
///
/// each segment is safe to run in its own shell.
pub fn split_command_list(command: &str) -> Option<Vec<SplitSegment>> {
    let segments = parse(command)?;
    if segments.len() < 2 {
        return None;
    }
    for (idx, seg) in segments.iter().enumerate() {
        let first = seg.stages.first()?;
        let Some(cmd) = command_word(&first.words) else {
            // An unexported assignment is gone by the next call.
            return None;
        };
        if STATEFUL_WORDS.contains(&cmd) {
            return None;
        }
        let text = seg.text(command);
        if idx > 0
            && ["$?", "${?}", "$!", "${!}"]
                .iter()
                .any(|v| text.contains(v))
        {
            return None;
        }
    }
    Some(
        segments
            .iter()
            .map(|seg| SplitSegment {
                command: seg.text(command).to_string(),
                only_if_previous_succeeded: seg.after_and,
            })
            .collect(),
    )
}

/// A trailing `| tail` that runs as a view over the kept output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailView {
    /// `tail -n N`: the last N lines.
    Last(usize),
    /// `tail -n +N`: from line N to the end.
    From(usize),
}

impl TailView {
    pub fn apply<'s>(self, text: &'s str) -> &'s str {
        let lines: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .filter(|&i| i < text.len())
            .collect();
        let start_line = match self {
            TailView::Last(n) => lines.len().saturating_sub(n),
            TailView::From(n) => n.saturating_sub(1),
        };
        match lines.get(start_line) {
            Some(&at) => &text[at..],
            None => "",
        }
    }

    pub fn describe(self) -> String {
        match self {
            TailView::Last(n) => format!("tail -n {n}"),
            TailView::From(n) => format!("tail -n +{n}"),
        }
    }
}

fn parse_count(s: &str) -> Option<TailView> {
    match s.strip_prefix('+') {
        Some(n) => n.parse().ok().map(TailView::From),
        None => s.parse().ok().map(TailView::Last),
    }
}

/// Parse a stage that is a bare `tail` over its input.
fn tail_view(words: &[Word]) -> Option<TailView> {
    let words: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    match words.as_slice() {
        ["tail"] => Some(TailView::Last(10)),
        ["tail", "-n" | "--lines", n] => parse_count(n),
        ["tail", flag] => {
            if let Some(n) = flag.strip_prefix("--lines=") {
                parse_count(n)
            } else if let Some(n) = flag.strip_prefix("-n") {
                parse_count(n)
            } else if let Some(n) = flag.strip_prefix('-') {
                n.parse().ok().map(TailView::Last)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The file that holds the output of pipeline stage `stage` (1-based) of the
/// command whose output file is `log_file`.
pub fn stage_file(log_file: &Path, stage: usize) -> PathBuf {
    sibling(log_file, &format!("stage{stage}.log"))
}

/// The file that holds one exit code per pipeline stage, one per line.
pub fn stage_status_file(log_file: &Path) -> PathBuf {
    sibling(log_file, "stages")
}

fn sibling(log_file: &Path, suffix: &str) -> PathBuf {
    let stem = log_file
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    log_file.with_file_name(format!("{stem}.{suffix}"))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// One teed stage of a rewritten pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageCapture {
    /// 1-based position in the pipeline.
    pub index: usize,
    pub command: String,
    pub file: PathBuf,
}

/// How a pipeline runs so that no stage's output is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePlan {
    /// The command to run in place of the model's command.
    pub command: String,
    /// Every stage except the last. The last stage's output is the tool's own output file.
    pub stages: Vec<StageCapture>,
    /// The command text of the last stage that runs.
    pub final_stage: String,
    /// Present when a trailing `| tail` was taken off the pipeline.
    pub tail_view: Option<TailView>,
    pub status_file: Option<PathBuf>,
}

/// Rewrite a single pipeline so each stage's output lands in a file.
///
/// `a | b | tail -n 5` runs as `a | tee s1 | b`.
pub fn plan_pipeline_capture(command: &str, log_file: &Path) -> Option<CapturePlan> {
    let mut segments = parse(command)?;
    if segments.len() != 1 {
        return None;
    }
    let mut stages = segments.pop()?.stages;
    if stages.len() < 2 {
        return None;
    }
    let tail = stages.last().and_then(|s| tail_view(&s.words));
    if tail.is_some() {
        stages.pop();
    }

    let last = stages.len() - 1;
    let mut out = String::new();
    let mut captures = Vec::new();
    for (idx, stage) in stages.iter().enumerate() {
        let text = stage.text(command);
        if idx == last {
            out.push_str(text);
            break;
        }
        let file = stage_file(log_file, idx + 1);
        let pipe = if stage.pipe_after == Some(Op::PipeAll) {
            "|&"
        } else {
            "|"
        };
        out.push_str(&format!("{text} {pipe} tee -- {} | ", shell_quote(&file)));
        captures.push(StageCapture {
            index: idx + 1,
            command: text.to_string(),
            file,
        });
    }

    let status_file = (stages.len() > 1).then(|| stage_status_file(log_file));
    if let Some(status) = &status_file {
        // Read `$?` and the stage codes in the statement that follows the pipeline:
        // any later command resets both. `pipestatus` is the zsh spelling. `declare
        // +x` keeps an allexport shell from carrying the helpers into the next
        // call.
        out.push_str(&format!(
            "\n__grok_rc=$? __grok_ps=(\"${{PIPESTATUS[@]}}\" \"${{pipestatus[@]}}\"); \
			 builtin printf '%s\\n' \"${{__grok_ps[@]}}\" > {} 2>/dev/null; \
			 builtin unset __grok_ps; builtin declare +x __grok_rc 2>/dev/null; \
			 ( exit \"$__grok_rc\" )",
            shell_quote(status)
        ));
    }

    Some(CapturePlan {
        command: out,
        final_stage: stages[last].text(command).to_string(),
        stages: captures,
        tail_view: tail,
        status_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(cmd: &str) -> Option<Vec<(String, bool)>> {
        split_command_list(cmd).map(|v| {
            v.into_iter()
                .map(|s| (s.command, s.only_if_previous_succeeded))
                .collect()
        })
    }

    #[test]
    fn splits_semicolons_newlines_and_and_lists() {
        assert_eq!(
            split("cd crates; cargo build && cargo test\nls -la"),
            Some(vec![
                ("cd crates".into(), false),
                ("cargo build".into(), false),
                ("cargo test".into(), true),
                ("ls -la".into(), false),
            ])
        );
    }

    #[test]
    fn a_line_break_after_and_continues_the_list() {
        assert_eq!(
            split("make &&\n  make install"),
            Some(vec![("make".into(), false), ("make install".into(), true)])
        );
    }

    #[test]
    fn a_single_command_is_not_split() {
        assert_eq!(split("cargo test -p foo 2>&1 | tail -n 20"), None);
        assert_eq!(split("echo hi;"), None);
    }

    #[test]
    fn quoted_and_substituted_separators_do_not_split() {
        assert_eq!(split("echo 'a; b' \"c && d\""), None);
        assert_eq!(split("echo $(date; uptime)"), None);
        assert_eq!(split("echo `a; b` ${x:-a;b}"), None);
        assert_eq!(
            split("echo \"$(printf '%s;' a)\"; ls"),
            Some(vec![
                ("echo \"$(printf '%s;' a)\"".into(), false),
                ("ls".into(), false)
            ])
        );
    }

    #[test]
    fn redirections_with_ampersands_are_not_background_operators() {
        assert_eq!(
            split("make >build.log 2>&1; cat build.log &>/dev/null"),
            Some(vec![
                ("make >build.log 2>&1".into(), false),
                ("cat build.log &>/dev/null".into(), false),
            ])
        );
    }

    #[test]
    fn constructs_that_need_one_shell_are_never_split() {
        for cmd in [
            "a || b; c",
            "sleep 5 & echo started",
            "X=1; echo $X",
            "false; echo $?",
            "set -e; make; make test",
            "export A=1; source env.sh; run",
            "for f in *; do echo $f; done",
            "if true; then a; fi; b",
            "(cd x; make); ls",
            "{ a; b; }; c",
            "f() { echo; }; f",
            "cat <<EOF\na; b\nEOF\nls",
            "case $x in a) echo;; esac; ls",
            "a && ; b",
            "a &&",
            "| a; b",
        ] {
            assert_eq!(split(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn env_prefixed_commands_and_exports_split() {
        assert_eq!(
            split("export RUST_LOG=debug; RUST_BACKTRACE=1 cargo test"),
            Some(vec![
                ("export RUST_LOG=debug".into(), false),
                ("RUST_BACKTRACE=1 cargo test".into(), false),
            ])
        );
    }

    #[test]
    fn comments_are_dropped_and_do_not_hide_separators() {
        assert_eq!(
            split("# build first\ncargo build # the lib\ncargo test"),
            Some(vec![
                ("cargo build".into(), false),
                ("cargo test".into(), false)
            ])
        );
    }

    #[test]
    fn a_here_string_is_not_a_heredoc() {
        assert_eq!(
            split("grep x <<< 'a;b'; ls"),
            Some(vec![
                ("grep x <<< 'a;b'".into(), false),
                ("ls".into(), false)
            ])
        );
    }

    #[test]
    fn plans_a_tee_for_every_stage_but_the_last() {
        let log = Path::new("/s/terminal/call_1.log");
        let plan = plan_pipeline_capture("cargo test 2>&1 | grep FAIL | sort", log).unwrap();
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].command, "cargo test 2>&1");
        assert_eq!(
            plan.stages[0].file,
            Path::new("/s/terminal/call_1.stage1.log")
        );
        assert_eq!(plan.stages[1].command, "grep FAIL");
        assert_eq!(plan.final_stage, "sort");
        assert!(plan.command.starts_with(
			"cargo test 2>&1 | tee -- '/s/terminal/call_1.stage1.log' | grep FAIL | tee -- '/s/terminal/call_1.stage2.log' | sort\n"
		));
        assert_eq!(
            plan.status_file.as_deref(),
            Some(Path::new("/s/terminal/call_1.stages"))
        );
        assert_eq!(plan.tail_view, None);
    }

    #[test]
    fn a_trailing_tail_becomes_a_view() {
        let log = Path::new("/s/c.log");
        let plan = plan_pipeline_capture("cargo test 2>&1 | tail -n 20", log).unwrap();
        assert_eq!(plan.command, "cargo test 2>&1");
        assert!(plan.stages.is_empty());
        assert_eq!(plan.status_file, None);
        assert_eq!(plan.tail_view, Some(TailView::Last(20)));

        for (tail, view) in [
            ("tail", TailView::Last(10)),
            ("tail -5", TailView::Last(5)),
            ("tail -n5", TailView::Last(5)),
            ("tail --lines=7", TailView::Last(7)),
            ("tail -n +3", TailView::From(3)),
        ] {
            let plan = plan_pipeline_capture(&format!("make | {tail}"), log).unwrap();
            assert_eq!(plan.tail_view, Some(view), "{tail}");
        }
        // `tail -f` and a tail of a file are not views over the pipe.
        for tail in ["tail -f", "tail -n 5 other.log", "tail -c 100"] {
            let plan = plan_pipeline_capture(&format!("make | {tail}"), log).unwrap();
            assert_eq!(plan.tail_view, None, "{tail}");
        }
    }

    #[test]
    fn pipe_all_keeps_stderr_in_the_stage_file() {
        let plan = plan_pipeline_capture("make |& grep error", Path::new("/s/c.log")).unwrap();
        assert!(
            plan.command
                .starts_with("make |& tee -- '/s/c.stage1.log' | grep error")
        );
    }

    #[test]
    fn only_single_pipelines_are_planned() {
        let log = Path::new("/s/c.log");
        assert_eq!(plan_pipeline_capture("cargo test", log), None);
        assert_eq!(plan_pipeline_capture("a | b; c", log), None);
        assert_eq!(plan_pipeline_capture("a | b || c", log), None);
        assert_eq!(
            plan_pipeline_capture("a | while read l; do echo $l; done", log),
            None
        );
    }

    #[test]
    fn paths_with_quotes_are_quoted() {
        let plan = plan_pipeline_capture("a | b", Path::new("/it's here/c.log")).unwrap();
        assert!(
            plan.command
                .contains(r"tee -- '/it'\''s here/c.stage1.log'"),
            "{}",
            plan.command
        );
    }

    #[test]
    fn tail_view_keeps_the_requested_lines() {
        let text = "1\n2\n3\n4\n";
        assert_eq!(TailView::Last(2).apply(text), "3\n4\n");
        assert_eq!(TailView::Last(10).apply(text), text);
        assert_eq!(TailView::From(3).apply(text), "3\n4\n");
        assert_eq!(TailView::From(9).apply(text), "");
        assert_eq!(TailView::Last(1).apply("a\nb"), "b");
    }
}
