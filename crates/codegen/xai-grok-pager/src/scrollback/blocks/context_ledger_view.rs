//! The `/context` request list: the provider's counts for each request, and the items each added.

use std::collections::BTreeSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use xai_grok_shell::session::{ContextInfo, LedgerItem};

use super::context_info::fmt_tok;
use crate::render::line_utils::truncate_str;
use crate::theme::Theme;

/// A row that expands into its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum LedgerRow {
    Request(usize),
    Unsent,
}

/// What a click on a drill-down line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerAction {
    Toggle(LedgerRow),
    OpenTranscript(String),
}

/// The drill-down lines and the line index of each clickable one.
#[derive(Debug, Default)]
pub(crate) struct LedgerLines {
    pub lines: Vec<Line<'static>>,
    pub actions: Vec<(usize, LedgerAction)>,
}

impl LedgerLines {
    fn push(&mut self, line: Line<'static>, action: Option<LedgerAction>) {
        if let Some(action) = action {
            self.actions.push((self.lines.len(), action));
        }
        self.lines.push(line);
    }
}

/// The request list: one row per request with the provider's counts, each expandable into the items it added.
pub(crate) fn request_lines(
    snapshot: &ContextInfo,
    expanded: &BTreeSet<LedgerRow>,
    theme: &Theme,
    width: usize,
) -> LedgerLines {
    let ledger = &snapshot.request_ledger;
    let muted = theme.muted();
    let label = Style::default().fg(theme.text_secondary);
    let mut out = LedgerLines::default();

    out.push(
        Line::from(Span::styled(
            "Requests",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        )),
        None,
    );
    out.push(
        Line::from(Span::styled(
            "        prompt   growth   cache read / write / input \u{00b7} output",
            muted,
        )),
        None,
    );

    let mut prev_prompt = 0u64;
    for (i, request) in ledger.requests.iter().enumerate() {
        let u = request.usage;
        let growth = u.prompt() as i64 - prev_prompt as i64;
        prev_prompt = u.prompt();
        let row = LedgerRow::Request(i);
        let open = expanded.contains(&row);
        out.push(
            fit(
                vec![
                    Span::styled(format!("{} ", disclosure(open)), muted),
                    Span::styled(format!("#{:<4}", i + 1), label),
                    Span::styled(
                        format!("{:>7}", fmt_tok(u.prompt())),
                        Style::default().fg(theme.text_primary),
                    ),
                    Span::styled(format!("  {:>7}", fmt_signed(growth)), label),
                    Span::styled(
                        format!(
                            "   {} / {} / {} \u{00b7} {} \u{00b7} turn {} \u{00b7} {}",
                            fmt_tok(u.cache_read),
                            fmt_tok(u.cache_write),
                            fmt_tok(u.input),
                            fmt_tok(u.output),
                            request.turn,
                            count(request.items.len(), "item"),
                        ),
                        muted,
                    ),
                ],
                width,
            ),
            Some(LedgerAction::Toggle(row)),
        );
        if open {
            push_items(&mut out, &request.items, theme, width);
        }
    }
    if ledger.requests.is_empty() {
        out.push(
            Line::from(Span::styled(
                "  No request has reported usage since the session loaded or compacted.",
                muted,
            )),
            None,
        );
    }
    if !ledger.unsent.is_empty() {
        let open = expanded.contains(&LedgerRow::Unsent);
        out.push(
            fit(
                vec![
                    Span::styled(format!("{} ", disclosure(open)), muted),
                    Span::styled("Not sent yet", label),
                    Span::styled(format!("   {}", count(ledger.unsent.len(), "item")), muted),
                ],
                width,
            ),
            Some(LedgerAction::Toggle(LedgerRow::Unsent)),
        );
        if open {
            push_items(&mut out, &ledger.unsent, theme, width);
        }
    }
    if let Some(path) = &snapshot.transcript_path {
        out.push(Line::from(""), None);
        out.push(
            fit(
                vec![
                    Span::styled(
                        "[ Open transcript ]",
                        Style::default()
                            .fg(theme.text_primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {path}"), muted),
                ],
                width,
            ),
            Some(LedgerAction::OpenTranscript(path.clone())),
        );
    }
    out
}

/// One line per item: position, kind, byte size and preview.
fn push_items(out: &mut LedgerLines, items: &[LedgerItem], theme: &Theme, width: usize) {
    let muted = theme.muted();
    if items.is_empty() {
        out.push(Line::from(Span::styled("      no items", muted)), None);
    }
    for item in items {
        out.push(
            fit(
                vec![
                    Span::styled(format!("    #{:<4} ", item.index), muted),
                    Span::styled(
                        format!("{:<22}", truncate_str(&item.kind, 22)),
                        Style::default().fg(theme.text_secondary),
                    ),
                    Span::styled(format!("{:>8}  ", fmt_bytes(item.bytes)), muted),
                    Span::styled(item.preview.clone(), muted),
                ],
                width,
            ),
            None,
        );
    }
}

fn disclosure(open: bool) -> &'static str {
    if open { "\u{25be}" } else { "\u{25b8}" }
}

fn fmt_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Cut a line of spans to `width` columns.
fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut left = width;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        if left == 0 {
            break;
        }
        let text = truncate_str(&span.content, left);
        left = left.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
        out.push(Span::styled(text, span.style));
    }
    Line::from(out)
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn fmt_signed(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "+" };
    format!("{sign}{}", fmt_tok(n.unsigned_abs()))
}
