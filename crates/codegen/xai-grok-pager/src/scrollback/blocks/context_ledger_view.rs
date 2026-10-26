//! The Overhead drill-down in the `/context` modal: tool definitions, one row per measured request, and the remainder.

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
    Unmeasured,
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

/// Build the drill-down for an Overhead of `overhead` tokens.
pub(crate) fn overhead_lines(
    snapshot: &ContextInfo,
    overhead: u64,
    expanded: &BTreeSet<LedgerRow>,
    theme: &Theme,
    width: usize,
) -> LedgerLines {
    let ledger = &snapshot.overhead_ledger;
    let muted = theme.muted();
    let label = Style::default().fg(theme.text_secondary);
    let mut out = LedgerLines::default();

    out.push(
        Line::from(Span::styled(
            "Where it comes from",
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        )),
        None,
    );
    let tools = snapshot.tool_definitions_tokens;
    out.push(
        fit(
            vec![
                Span::styled(format!("  {:<24}", "Tool definitions"), label),
                Span::styled(format!("{:>7}", fmt_signed(tools as i64)), label),
                Span::styled("  sent with every request".to_string(), muted),
            ],
            width,
        ),
        None,
    );

    let mut measured_gap = 0i64;
    for (i, request) in ledger.requests.iter().enumerate() {
        // The first request also carried the tool definitions, which have their own row.
        let gap = if i == 0 {
            request.gap() - tools as i64
        } else {
            request.gap()
        };
        measured_gap += gap;
        let row = LedgerRow::Request(i);
        let open = expanded.contains(&row);
        let ratio = if request.estimated > 0 {
            format!(
                " \u{00d7}{:.2}",
                request.measured as f64 / request.estimated as f64
            )
        } else {
            String::new()
        };
        out.push(
            fit(
                vec![
                    Span::styled(format!("{} ", disclosure(open)), muted),
                    Span::styled(
                        format!(
                            "{:<24}",
                            format!("Request {} \u{00b7} turn {}", i + 1, request.turn)
                        ),
                        label,
                    ),
                    Span::styled(format!("{:>7}", fmt_signed(gap)), gap_style(theme, gap)),
                    Span::styled(
                        format!(
                            "  {} measured / {} est{ratio} \u{00b7} {}",
                            fmt_tok(request.measured),
                            fmt_tok(request.estimated),
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

    // The remainder makes the rows sum to Overhead: the last reply's output against its estimate, and items not yet sent.
    let rest = overhead as i64 - tools as i64 - measured_gap;
    let open = expanded.contains(&LedgerRow::Unmeasured);
    let rest_label = if ledger.requests.is_empty() {
        "Not measured yet"
    } else {
        "Since the last request"
    };
    out.push(
        fit(
            vec![
                Span::styled(format!("{} ", disclosure(open)), muted),
                Span::styled(format!("{rest_label:<24}"), label),
                Span::styled(format!("{:>7}", fmt_signed(rest)), gap_style(theme, rest)),
                Span::styled(
                    format!("  {} not yet sent", count(ledger.unmeasured.len(), "item")),
                    muted,
                ),
            ],
            width,
        ),
        Some(LedgerAction::Toggle(LedgerRow::Unmeasured)),
    );
    if open {
        push_items(&mut out, &ledger.unmeasured, theme, width);
    }

    out.push(Line::from(""), None);
    let note = if ledger.requests.is_empty() {
        "No model request has been measured since the session loaded, compacted or rewound. \
         The overhead is carried over from before that."
    } else {
        "A gap is what the provider counted for a request minus the bytes/4 estimate of the items it added. \
         Click a row to list its items."
    };
    for line in
        crate::render::wrapping::word_wrap_lines(vec![Line::from(Span::styled(note, muted))], width)
    {
        out.push(line, None);
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

/// One line per item: position, kind, estimate and preview.
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
                    Span::styled(format!("{:>7}  ", fmt_tok(item.tokens)), muted),
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

fn gap_style(theme: &Theme, gap: i64) -> Style {
    if gap < 0 {
        theme.muted()
    } else {
        Style::default().fg(theme.text_primary)
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
