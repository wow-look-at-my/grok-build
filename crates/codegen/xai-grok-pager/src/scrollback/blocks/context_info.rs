//! ContextInfoBlock: the typed `/context` display rendered in scrollback.
//!
//! Holds the raw [`ContextInfo`] snapshot and the active model name, and rebuilds the styled output on every `output()` call.
//! This is the same pattern as [`super::SessionEventBlock`]: keep typed data, format at render time.
//! `Theme::current()` is re-resolved on every redraw, so switching themes after running `/context` updates the colors immediately.
//! Formatting at capture time would have baked in stale colors.

use std::collections::BTreeSet;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::context_ledger_view::{LedgerAction, LedgerRow};
use crate::appearance::AppearanceConfig;
use crate::render::wrapping::word_wrap_lines;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{AccentStyle, BlockContext, BlockLine, BlockOutput};
use crate::theme::{Theme, quantize};
use xai_grok_shell::session::{ContextInfo, ProviderUsage};

/// Categorical bar: each cell uses its category's glyph and color, filled left-to-right in legend order; free capacity is muted outlines.
/// The informational rows never enter the bar.
#[derive(Debug, Clone)]
pub struct ContextInfoBlock {
    /// The captured context-window snapshot.
    pub snapshot: ContextInfo,
    /// Active model name at the time of capture (display-only).
    pub model: String,
}

/// Shape of the categorical bar: how the 100 cells are laid out. Both shapes hold the same 100 cells, so the visual
/// breakdown (which categories occupy which share of the bar) is identical. Only the aspect ratio changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BarLayout {
    /// Cells per row.
    row_len: usize,
    /// Number of rows.
    rows: usize,
}

impl BarLayout {
    /// 5 rows × 20 cells.
    /// Renders in ~39 columns (20 glyphs + 19 separator spaces).
    const WIDE: Self = Self {
        row_len: 20,
        rows: 5,
    };

    /// 10 rows × 10 cells.
    /// Renders in ~19 columns (10 glyphs + 9 separator spaces).
    const NARROW: Self = Self {
        row_len: 10,
        rows: 10,
    };

    /// Terminal width (in columns) at which the bar switches from [`Self::WIDE`] to [`Self::NARROW`]. The wide layout
    /// needs 39 columns just for the bar. 50 leaves ~11 columns of margin. 50 is also roughly where the legend rows ·
    /// 12 tools`) start to word-wrap. The breakpoint therefore matches the rest of the block's responsive behavior.
    const NARROW_BREAKPOINT: u16 = 50;

    /// Choose a layout that fits the available terminal width.
    fn for_width(width: u16) -> Self {
        if width < Self::NARROW_BREAKPOINT {
            Self::NARROW
        } else {
            Self::WIDE
        }
    }

    /// Total cells in the bar; always 100 for both shipped layouts.
    /// Kept as a method so a future layout with a different cell count doesn't silently misalign with the legend percentages.
    const fn total(self) -> usize {
        self.row_len * self.rows
    }
}

/// One legend or informational row, before column formatting.
struct LegendRow {
    glyph: &'static str,
    color: Color,
    label: String,
    tokens: u64,
    detail: Option<String>,
}

/// Column widths for the legend and informational rows, measured from the rows that actually render.
/// Token counts, percentages, and detail counts then stay aligned no matter the labels or magnitudes.
struct RowLayout {
    label_width: usize,
    tokens_width: usize,
    percent_width: usize,
    count_width: usize,
}

impl RowLayout {
    /// Measure column widths over every row that will render.
    /// Widths are in codepoints, not bytes.
    fn measure<'a>(rows: impl Iterator<Item = &'a LegendRow> + Clone, total: u64) -> Self {
        Self {
            label_width: rows
                .clone()
                .map(|r| r.label.chars().count())
                .max()
                .unwrap_or(0)
                + 1,
            tokens_width: rows
                .clone()
                .map(|r| fmt_tok(r.tokens).chars().count())
                .max()
                .unwrap_or(0),
            percent_width: rows
                .clone()
                .map(|r| Self::percent(r.tokens, total).chars().count())
                .max()
                .unwrap_or(0),
            count_width: rows
                .filter_map(|r| r.detail.as_deref())
                .filter_map(|d| d.split(' ').next().map(|n| n.chars().count()))
                .max()
                .unwrap_or(0),
        }
    }

    /// The parenthesized share of the window, e.g. `"(0.6%)"`.
    fn percent(tokens: u64, total: u64) -> String {
        format!("({})", percent_of_window(tokens, total))
    }

    /// The row's numeric cells: tokens and percent, each right-aligned.
    fn cells(&self, tokens: u64, total: u64) -> String {
        format!(
            "{:>tokens_width$} tokens   {:>percent_width$}",
            fmt_tok(tokens),
            Self::percent(tokens, total),
            tokens_width = self.tokens_width,
            percent_width = self.percent_width,
        )
    }

    /// The detail suffix with the leading count right-aligned so the nouns line up: `" · 25 tools"` over `" ·  1 server"`.
    /// Details follow the `TokenUsageCategory::detail` count-then-noun convention; text with no space renders unaligned.
    fn detail_suffix(&self, detail: &str) -> String {
        match detail.split_once(' ') {
            Some((count, rest)) => {
                format!(
                    " \u{00b7} {count:>count_width$} {rest}",
                    count_width = self.count_width
                )
            }
            None => format!(" \u{00b7} {detail}"),
        }
    }

    /// Render one row.
    /// WIDE: a single column-aligned line.
    /// NARROW: glyph and label on the first line, numeric data indented by one space on the second so it clusters under the label.
    fn render(
        &self,
        row: &LegendRow,
        bar: BarLayout,
        total: u64,
        label_style: Style,
        muted: Style,
    ) -> Vec<Line<'static>> {
        let glyph = Span::styled(format!("{} ", row.glyph), Style::default().fg(row.color));
        let suffix = row.detail.as_deref().map(|d| self.detail_suffix(d));
        if bar == BarLayout::NARROW {
            let first = Line::from(vec![glyph, Span::styled(row.label.clone(), label_style)]);
            let mut second = vec![
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{} tokens   {}",
                        fmt_tok(row.tokens),
                        Self::percent(row.tokens, total)
                    ),
                    muted,
                ),
            ];
            if let Some(extra) = suffix {
                second.push(Span::styled(extra, muted));
            }
            vec![first, Line::from(second)]
        } else {
            let mut spans = vec![
                glyph,
                Span::styled(
                    format!(
                        "{:<label_width$}",
                        row.label,
                        label_width = self.label_width
                    ),
                    label_style,
                ),
                Span::raw(" "),
                Span::styled(self.cells(row.tokens, total), muted),
            ];
            if let Some(extra) = suffix {
                spans.push(Span::styled(extra, muted));
            }
            vec![Line::from(spans)]
        }
    }
}

/// Index of the first bar row: `Context`, a blank, the totals, the model and a blank come before it.
const BAR_FIRST_LINE: usize = 5;

/// Columns between the bar and the right column in the two-column modal view.
const COLUMN_GAP: usize = 3;

const CACHE_READ_LABEL: &str = "Cache read";
const CACHE_WRITE_LABEL: &str = "Cache write";
const INPUT_LABEL: &str = "Input";
const OUTPUT_LABEL: &str = "Output";
const FREE_LABEL: &str = "Free";

/// The category colors. The bar and the legend use the same ones, so both read side by side.
struct Palette {
    cache_read: Color,
    cache_write: Color,
    input: Color,
    output: Color,
    empty: Color,
}

impl Palette {
    fn new(theme: &Theme) -> Self {
        Self {
            cache_read: quantize(theme.gray_bright),
            cache_write: quantize(theme.accent_skill),
            input: quantize(theme.text_primary),
            output: quantize(theme.accent_assistant),
            empty: quantize(theme.gray_dim),
        }
    }
}

/// One contiguous run of bar cells for one category, in legend order.
/// A category too small to round to one cell has an empty range.
#[derive(Debug, Clone)]
struct BarSegment {
    label: &'static str,
    glyph: &'static str,
    color: Color,
    tokens: u64,
    cells: Range<usize>,
}

/// Where the bar sits in the modal's context-tab lines, for mouse hit tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BarGeometry {
    pub first_line: usize,
    pub rows: usize,
    pub row_len: usize,
}

impl BarGeometry {
    /// The cell under the content-relative column `x` on content line `line`.
    /// Column `2i` is cell `i`. The space after a cell selects that cell too.
    pub(crate) fn cell_at(self, x: usize, line: usize) -> Option<usize> {
        let row = line.checked_sub(self.first_line)?;
        if row >= self.rows || x + 1 >= self.row_len * 2 {
            return None;
        }
        Some(row * self.row_len + x / 2)
    }
}

/// The modal's context-tab lines, plus the data its mouse handler needs.
pub(crate) struct ContextModalView {
    pub lines: Vec<Line<'static>>,
    pub bar: BarGeometry,
    /// The cell range and label of each segment that has at least one cell.
    pub segments: Vec<(Range<usize>, &'static str)>,
    /// The clickable drill-down lines, by line index.
    pub actions: Vec<(usize, LedgerAction)>,
}

impl ContextInfoBlock {
    pub fn new(snapshot: ContextInfo, model: impl Into<String>) -> Self {
        Self {
            snapshot,
            model: model.into(),
        }
    }

    /// The modal view. When the width holds the bar and the totals side by side, the totals go in a right column.
    /// `selected` names a clicked segment: its cells stay lit and its details replace the totals.
    pub(crate) fn modal_view(
        &self,
        theme: &Theme,
        width: u16,
        selected: Option<&str>,
        expanded: &BTreeSet<LedgerRow>,
    ) -> ContextModalView {
        let width = width as usize;
        let wide = BarLayout::WIDE;
        let bar_width = wide.row_len * 2 - 1;
        let wide_stats = self.stats_lines(theme, wide);
        let stats_width = wide_stats.iter().map(Line::width).max().unwrap_or(0);
        let two_columns = width >= bar_width + COLUMN_GAP + stats_width;
        let bar = if two_columns {
            wide
        } else {
            BarLayout::for_width(u16::try_from(width).unwrap_or(u16::MAX))
        };
        let segments = self.segments(theme, bar);
        let selected = selected
            .and_then(|label| segments.iter().find(|s| s.label == label))
            .filter(|s| !s.cells.is_empty());

        let hint = if selected.is_some() {
            "Click the lit range again to show the totals."
        } else {
            "Click a diamond to inspect its range."
        };
        let hint = Line::from(Span::styled(hint, theme.muted()));

        let mut left = self.header_lines(theme);
        left.extend(bar_lines(&segments, bar, selected.map(|s| s.label), theme));
        left.push(Line::from(""));
        // Without a selection the request list follows the totals. Its clickable lines are offset to where it lands.
        let mut actions = Vec::new();
        let with_requests = |mut lines: Vec<Line<'static>>,
                             width: usize,
                             base: usize,
                             actions: &mut Vec<(usize, LedgerAction)>| {
            lines.push(Line::from(""));
            let list =
                super::context_ledger_view::request_lines(&self.snapshot, expanded, theme, width);
            let offset = base + lines.len();
            actions.extend(list.actions.into_iter().map(|(i, a)| (i + offset, a)));
            lines.extend(list.lines);
            lines
        };
        let lines = if two_columns {
            left.extend(word_wrap_lines(vec![hint], bar_width));
            let right_width = width - bar_width - COLUMN_GAP;
            let right = match selected {
                Some(seg) => self.inspect_lines(theme, seg, bar, right_width),
                None => with_requests(wide_stats, right_width, 0, &mut actions),
            };
            zip_columns(left, bar_width + COLUMN_GAP, right)
        } else {
            left.extend(word_wrap_lines(vec![hint], width));
            left.push(Line::from(""));
            match selected {
                Some(seg) => left.extend(self.inspect_lines(theme, seg, bar, width)),
                None => {
                    let base = left.len();
                    let stats =
                        with_requests(self.stats_lines(theme, bar), width, base, &mut actions);
                    left.extend(stats);
                }
            }
            left
        };

        ContextModalView {
            lines,
            actions,
            bar: BarGeometry {
                first_line: BAR_FIRST_LINE,
                rows: bar.rows,
                row_len: bar.row_len,
            },
            segments: segments
                .iter()
                .filter(|s| !s.cells.is_empty())
                .map(|s| (s.cells.clone(), s.label))
                .collect(),
        }
    }

    /// Build the styled lines using the supplied theme and bar layout. Called from `output()` on every redraw so theme
    /// switches take effect without re-running `/context`. The theme is passed in rather than re-resolved here.
    fn build_lines(&self, theme: &Theme, bar: BarLayout) -> Vec<Line<'static>> {
        let segments = self.segments(theme, bar);
        let mut lines = self.header_lines(theme);
        lines.extend(bar_lines(&segments, bar, None, theme));
        lines.push(Line::from(""));
        lines.extend(self.stats_lines(theme, bar));
        lines
    }

    /// The provider's counts for the last request, capped to the window. `None` before any request reported usage.
    fn window_tokens(&self) -> Option<WindowTokens> {
        let usage = self.snapshot.request_ledger.requests.last()?.usage;
        Some(window_tokens(&usage, self.snapshot.total))
    }

    /// `Context`, a blank, the totals, the model and a blank: [`BAR_FIRST_LINE`] lines.
    fn header_lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let primary = Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD);
        let total = self.snapshot.total;
        // The percent is recomputed from `used / total` for decimals.
        let totals = match self.window_tokens() {
            Some(t) => format!(
                "{} / {} tokens ({:.2}%)",
                fmt_tok_big(t.used),
                fmt_tok_big(total),
                precise_usage_percent(t.used, total),
            ),
            None => format!(
                "No usage reported yet \u{00b7} {} window",
                fmt_tok_big(total)
            ),
        };
        vec![
            Line::from(Span::styled("Context", primary)),
            Line::from(""),
            // `text_secondary` sets the totals apart from the muted rows.
            Line::from(Span::styled(
                totals,
                Style::default().fg(theme.text_secondary),
            )),
            Line::from(Span::styled(
                self.model.clone(),
                Style::default().fg(theme.gray_bright),
            )),
            Line::from(""),
        ]
    }

    /// The bar's categories in legend order. The ranges cover every cell exactly once.
    fn segments(&self, theme: &Theme, bar: BarLayout) -> Vec<BarSegment> {
        let total = self.snapshot.total;
        let tokens = self.window_tokens().unwrap_or_default();
        let palette = Palette::new(theme);
        // The `glyphs` helpers fall back to CP437-safe stand-ins on legacy Windows consoles.
        let filled = crate::glyphs::diamond_filled();

        let total_cells = bar.total();
        // Cumulative rounding: each boundary is rounded, so the runs sum to the used cells exactly.
        let cells_through = |tokens: u64| -> usize {
            if total == 0 {
                0
            } else {
                (((tokens as f64 / total as f64) * total_cells as f64).round() as usize)
                    .min(total_cells)
            }
        };
        let mut segments: Vec<BarSegment> = Vec::new();
        let mut push = |label, glyph, color, tokens, len: usize| {
            let start = segments.last().map_or(0, |s| s.cells.end);
            segments.push(BarSegment {
                label,
                glyph,
                color,
                tokens,
                cells: start..start + len,
            });
        };
        let mut acc = 0u64;
        let mut placed = 0usize;
        for (label, color, part) in [
            (CACHE_READ_LABEL, palette.cache_read, tokens.cache_read),
            (CACHE_WRITE_LABEL, palette.cache_write, tokens.cache_write),
            (INPUT_LABEL, palette.input, tokens.input),
            (OUTPUT_LABEL, palette.output, tokens.output),
        ] {
            acc += part;
            let end = cells_through(acc).max(placed);
            push(label, filled, color, part, end - placed);
            placed = end;
        }
        push(
            FREE_LABEL,
            crate::glyphs::diamond_hollow(),
            palette.empty,
            tokens.free(total),
            total_cells - placed,
        );
        debug_assert_eq!(segments.last().map_or(0, |s| s.cells.end), total_cells);
        segments
    }

    /// The legend of the last request's provider counts, the auto-compact line, the counters, and the compaction tip.
    fn stats_lines(&self, theme: &Theme, bar: BarLayout) -> Vec<Line<'static>> {
        let snapshot = &self.snapshot;
        let total = snapshot.total;
        let palette = Palette::new(theme);
        let muted = theme.muted();
        let filled = crate::glyphs::diamond_filled();

        let mut lines = Vec::new();
        match self.window_tokens() {
            Some(tokens) => {
                let row = |label: &str, color, tokens| LegendRow {
                    glyph: filled,
                    color,
                    label: label.to_string(),
                    tokens,
                    detail: None,
                };
                let legend_rows = [
                    row(CACHE_READ_LABEL, palette.cache_read, tokens.cache_read),
                    row(CACHE_WRITE_LABEL, palette.cache_write, tokens.cache_write),
                    row(INPUT_LABEL, palette.input, tokens.input),
                    row(OUTPUT_LABEL, palette.output, tokens.output),
                    LegendRow {
                        glyph: crate::glyphs::diamond_hollow(),
                        color: palette.empty,
                        label: FREE_LABEL.to_string(),
                        tokens: tokens.free(total),
                        detail: None,
                    },
                ];
                let layout = RowLayout::measure(legend_rows.iter(), total);
                let label_style = Style::default().fg(theme.text_secondary);
                for row in &legend_rows {
                    lines.extend(layout.render(row, bar, total, label_style, muted));
                }
                lines.push(Line::from(Span::styled(
                    "The provider's usage for the last request.",
                    muted,
                )));
            }
            None => lines.push(Line::from(Span::styled(
                "No request has reported usage since the session loaded or compacted.",
                muted,
            ))),
        }
        lines.push(Line::from(""));
        if let Some(line) = self.auto_compact_line(theme) {
            lines.push(line);
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!(
                "Turns: {} \u{00b7} Tool calls: {} \u{00b7} Compactions: {}",
                snapshot.turn_count, snapshot.tool_call_count, snapshot.compaction_count
            ),
            muted,
        )));

        // At or past the threshold the auto-compact line already says it, and a second warning would contradict it.
        if (80..snapshot.auto_compact_threshold_percent).contains(&snapshot.usage_pct) {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Tip: run /compact to free up context space.".to_string(),
                Style::default().fg(quantize(theme.warning)),
            )));
        }
        lines
    }

    /// `Auto-compact at N% · ~X tokens remaining`, or the warning when the threshold is reached. `None` for an empty window.
    fn auto_compact_line(&self, theme: &Theme) -> Option<Line<'static>> {
        let snapshot = &self.snapshot;
        if snapshot.total == 0 {
            return None;
        }
        let threshold_percent = snapshot.auto_compact_threshold_percent;
        // `div_ceil`: truncation can show `~0 tokens remaining` for a tiny window that is still below the threshold.
        let threshold_tokens = snapshot
            .total
            .saturating_mul(threshold_percent as u64)
            .div_ceil(100);
        let remaining = threshold_tokens.saturating_sub(snapshot.used);
        let (text, style) = if snapshot.usage_pct >= threshold_percent {
            (
                format!("Auto-compact triggers next turn (at {threshold_percent}%)"),
                Style::default().fg(quantize(theme.warning)),
            )
        } else {
            (
                format!(
                    "Auto-compact at {threshold_percent}% \u{00b7} ~{} tokens remaining",
                    fmt_tok_big(remaining)
                ),
                theme.muted(),
            )
        };
        Some(Line::from(Span::styled(text, style)))
    }

    /// The details of one clicked segment, word-wrapped to `width`.
    fn inspect_lines(
        &self,
        theme: &Theme,
        seg: &BarSegment,
        bar: BarLayout,
        width: usize,
    ) -> Vec<Line<'static>> {
        let snapshot = &self.snapshot;
        let muted = theme.muted();
        let used = self.window_tokens().map_or(0, |t| t.used);

        let mut shares = format!(
            "{} tokens \u{00b7} {} of window",
            fmt_tok(seg.tokens),
            percent_of_window(seg.tokens, snapshot.total)
        );
        if seg.label != FREE_LABEL && used > 0 {
            shares.push_str(&format!(
                " \u{00b7} {} of used",
                percent_of_window(seg.tokens, used)
            ));
        }
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("{} ", seg.glyph), Style::default().fg(seg.color)),
                Span::styled(
                    seg.label,
                    Style::default()
                        .fg(theme.text_primary)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(shares, muted)),
            Line::from(Span::styled(
                format!(
                    "Cells {}\u{2013}{} of {} \u{00b7} one cell \u{2248} {} tokens",
                    seg.cells.start + 1,
                    seg.cells.end,
                    bar.total(),
                    fmt_tok(snapshot.total / bar.total() as u64)
                ),
                muted,
            )),
            Line::from(""),
        ];
        lines.extend(word_wrap_lines(
            vec![Line::from(Span::styled(
                segment_description(seg.label),
                Style::default().fg(theme.text_secondary),
            ))],
            width,
        ));

        if seg.label == FREE_LABEL
            && let Some(line) = self.auto_compact_line(theme)
        {
            lines.push(Line::from(""));
            lines.push(line);
        }
        lines
    }
}

/// What a segment holds, in plain words.
fn segment_description(label: &str) -> &'static str {
    match label {
        CACHE_READ_LABEL => {
            "Prompt tokens the provider served from its prompt cache: Messages `cache_read_input_tokens`, OpenAI `cached_tokens`."
        }
        CACHE_WRITE_LABEL => {
            "Prompt tokens the provider wrote to its prompt cache on this request: Messages `cache_creation_input_tokens`."
        }
        INPUT_LABEL => "Prompt tokens neither read from nor written to the cache.",
        OUTPUT_LABEL => "The tokens the model generated for its reply.",
        FREE_LABEL => "The part of the window that the next request can still use.",
        _ => "",
    }
}

/// The bar rows. With a selection, the selected cells stay lit and bold and every other cell goes dim.
fn bar_lines(
    segments: &[BarSegment],
    bar: BarLayout,
    selected: Option<&str>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let dim = quantize(theme.gray_dim);
    let cells: Vec<(&'static str, Style)> = segments
        .iter()
        .flat_map(|seg| {
            let style = match selected {
                None => Style::default().fg(seg.color),
                Some(label) if label == seg.label => {
                    // Free cells are dim already. Light them so the selection shows.
                    let color = if seg.label == FREE_LABEL {
                        quantize(theme.text_secondary)
                    } else {
                        seg.color
                    };
                    Style::default().fg(color).add_modifier(Modifier::BOLD)
                }
                Some(_) => Style::default().fg(dim),
            };
            seg.cells.clone().map(move |_| (seg.glyph, style))
        })
        .collect();
    debug_assert_eq!(cells.len(), bar.total());

    cells
        .chunks(bar.row_len)
        .take(bar.rows)
        .map(|row| {
            let mut spans = Vec::with_capacity(bar.row_len * 2);
            for (i, (glyph, style)) in row.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled((*glyph).to_string(), *style));
            }
            Line::from(spans)
        })
        .collect()
}

/// Put `right` beside `left`. Each left line is padded to `left_width` columns.
fn zip_columns(
    left: Vec<Line<'static>>,
    left_width: usize,
    right: Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let rows = left.len().max(right.len());
    let mut left = left.into_iter();
    let mut right = right.into_iter();
    (0..rows)
        .map(|_| {
            let l = left.next().unwrap_or_default();
            let Some(r) = right.next() else {
                return l;
            };
            let pad = left_width.saturating_sub(l.width());
            let mut spans = l.spans;
            spans.push(Span::raw(" ".repeat(pad)));
            spans.extend(r.spans);
            Line::from(spans)
        })
        .collect()
}

/// Format a token count compactly (`123`, `1.2k`, `999k`). The cutover from `{.1}k` to plain `{}k` happens at
/// 99_500 (not 100_000) to avoid a precision discontinuity.
pub(super) fn fmt_tok(n: u64) -> String {
    if n >= 99_500 {
        // Round half-up to the nearest 1k; equivalent to `(n + 500) / 1000` for u64, which avoids the f64 rounding artifact described above
        format!("{}k", (n + 500) / 1000)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}

/// Compute `used / total * 100` as f64 with safe handling of `total == 0`.
fn precise_usage_percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

/// Like [`fmt_tok`] but rolls over to `1.0m` at one million.
/// Used for the at-a-glance totals line so a 1M / 2M / 4M context window reads naturally as `1.0m` rather than `1000k`.
/// Per-category legend rows stay on [`fmt_tok`] so a fractional-million breakdown still shows the finer-grained `k` resolution.
fn fmt_tok_big(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}m", n as f64 / 1_000_000.0)
    } else {
        fmt_tok(n)
    }
}

/// The provider's counts for one request, capped so `used` never passes the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WindowTokens {
    cache_read: u64,
    cache_write: u64,
    input: u64,
    output: u64,
    used: u64,
}

impl WindowTokens {
    fn free(&self, total: u64) -> u64 {
        total.saturating_sub(self.used)
    }
}

/// Take each count in legend order until the window is full.
fn window_tokens(usage: &ProviderUsage, total: u64) -> WindowTokens {
    let mut left = total;
    let mut take = |n: u64| {
        let n = n.min(left);
        left -= n;
        n
    };
    let cache_read = take(usage.cache_read);
    let cache_write = take(usage.cache_write);
    let input = take(usage.input);
    let output = take(usage.output);
    WindowTokens {
        cache_read,
        cache_write,
        input,
        output,
        used: total - left,
    }
}

/// Format a category's share of the total context window as a percentage.
fn percent_of_window(part: u64, total: u64) -> String {
    if total == 0 {
        return "-".to_string();
    }
    // Tiny nonzero shares floor at 0.1% so the column stays clean.
    let p = ((part as f64 / total as f64) * 100.0).max(if part > 0 { 0.1 } else { 0.0 });
    if p < 10.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

impl BlockContent for ContextInfoBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        // Responsive bar shape: narrow terminals get a 10×10 bar so it doesn't get clipped or pushed under the legend
        let bar = BarLayout::for_width(ctx.width);
        let styled_lines = self.build_lines(&theme, bar);
        let wrapped = word_wrap_lines(styled_lines, ctx.width as usize);
        let all_lines: Vec<BlockLine> = wrapped
            .into_iter()
            .map(|line| BlockLine::styled(line).with_selection_range(Some(0)))
            .collect();

        // Apply max_lines budget if set
        let lines = if let Some(max) = ctx.max_lines {
            let max = max as usize;
            if all_lines.len() > max && max > 0 {
                let take_count = if max > 1 { max - 1 } else { 1 };
                let mut truncated: Vec<BlockLine> =
                    all_lines.into_iter().take(take_count).collect();
                if let Some(last) = truncated.last_mut() {
                    let content_end = last.content.spans.len();
                    last.content
                        .spans
                        .push(Span::styled(" \u{2026}".to_string(), theme.muted()));
                    last.selectable = crate::scrollback::types::Selectable::Spans(0..content_end);
                }
                truncated
            } else {
                all_lines
            }
        } else {
            all_lines
        };

        if lines.is_empty() {
            BlockOutput {
                lines: vec![BlockLine::styled(Line::from("")).with_selection_range(Some(0))],
            }
        } else {
            BlockOutput { lines }
        }
    }

    fn accent(&self, _ctx: &BlockContext) -> Option<AccentStyle> {
        None
    }

    fn has_vpad_for(&self, _appearance: &AppearanceConfig) -> bool {
        false // Compact like SystemMessageBlock
    }

    fn has_raw_mode(&self) -> bool {
        false
    }

    fn is_foldable(&self) -> bool {
        false
    }

    fn is_selectable(&self) -> bool {
        false
    }

    fn is_groupable(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::session::{ContextLedger, LedgerRequest};

    fn snapshot() -> ContextInfo {
        ContextInfo {
            used: 36_700,
            total: 1_000_000,
            system_prompt_tokens: 1_200,
            tool_definitions_count: 12,
            tool_definitions_tokens: 5_600,
            compaction_count: 0,
            turn_count: 5,
            tool_call_count: 12,
            message_count: 8,
            message_tokens: 29_900,
            message_breakdown: Default::default(),
            free_tokens: 963_300,
            usage_pct: 4,
            auto_compact_threshold_percent: 85,
            usage_categories: vec![],
            request_ledger: ContextLedger {
                requests: vec![LedgerRequest {
                    turn: 1,
                    usage: ProviderUsage {
                        cache_read: 29_900,
                        cache_write: 5_000,
                        input: 1_200,
                        output: 600,
                    },
                    items: vec![],
                }],
                unsent: vec![],
            },
            transcript_path: None,
        }
    }

    /// Theme handle used by the unit tests.
    /// `Theme::current()` is the same resolver `output()` calls.
    /// The active theme doesn't matter here: the tests assert on text content and span counts, not colors.
    fn test_theme() -> Theme {
        Theme::current()
    }

    /// Render a block and collapse a single line's spans into a flat string.
    fn line_text(lines: &[Line<'static>], idx: usize) -> String {
        let Some(line) = lines.get(idx) else {
            panic!("expected line {idx}, got {} lines", lines.len());
        };
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Render a block and collapse all spans into a flat newline-joined string, useful for `contains` assertions.
    fn all_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()).chain(["\n"]))
            .collect()
    }

    #[test]
    fn build_lines_contains_header_tokens_and_model() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        // Layout: Context / <blank> / tokens / model.
        assert_eq!(line_text(&lines, 0), "Context");
        assert_eq!(line_text(&lines, 1), "");
        let l2 = line_text(&lines, 2);
        assert!(l2.contains("tokens"));
        // Percent shows 2 decimal places (36.7k / 1m = 3.67%)
        assert!(l2.contains("(3.67%)"), "got: {l2:?}");
        assert_eq!(line_text(&lines, 3), "grok-4");
    }

    #[test]
    fn precise_usage_percent_handles_zero_total() {
        assert_eq!(precise_usage_percent(100, 0), 0.0);
    }

    #[test]
    fn precise_usage_percent_two_decimal_formatting() {
        // 36_700 / 1_000_000 = 3.67 (exact to 2 decimals)
        let s = format!("{:.2}", precise_usage_percent(36_700, 1_000_000));
        assert_eq!(s, "3.67");
    }

    #[test]
    fn build_lines_includes_compaction_tip_in_warning_band() {
        // 80..85 is the band where the tip appears
        // Auto-compact is close enough to mention but not so close that the "triggers next turn" line is also showing
        let mut snap = snapshot();
        snap.usage_pct = 80;
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let last = line_text(&lines, lines.len() - 1);
        assert!(last.contains("/compact"), "expected tip line, got {last:?}");
    }

    #[test]
    fn build_lines_omits_tip_below_threshold() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        assert!(!all_text(&lines).contains("/compact"));
    }

    #[test]
    fn build_lines_omits_tip_at_or_above_threshold() {
        // At/above the auto-compact threshold the "triggers next turn" line already says everything the tip would
        // The tip is suppressed to avoid stacking two contradicting warning-styled lines (manual /compact vs. auto-compact about to fire).
        let mut snap = snapshot();
        snap.usage_pct = 85; // the historical default (and value in snapshot() helper)
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        assert!(!all_text(&lines).contains("/compact"));
    }

    #[test]
    fn build_lines_shows_auto_compact_estimate_below_threshold() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let all = all_text(&lines);
        assert!(
            all.contains("Auto-compact at 85%") && all.contains("tokens remaining"),
            "expected `Auto-compact at 85% · ~X tokens remaining` line, got:\n{all}"
        );
    }

    #[test]
    fn build_lines_auto_compact_eta_uses_millions_for_wide_windows() {
        // 4M window at 0% used: remaining = ceil(4_000_000 * 85 / 100) = 3_400_000.
        // Renders via fmt_tok_big as "3.4m", not "3400k"
        let mut snap = snapshot();
        snap.total = 4_000_000;
        snap.used = 0;
        snap.usage_pct = 0;
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let all = all_text(&lines);
        assert!(
            all.contains("~3.4m tokens remaining"),
            "expected ETA to use millions, got:\n{all}"
        );
    }

    #[test]
    fn build_lines_auto_compact_eta_arithmetic_at_known_snapshot() {
        // 1M window, 36_700 used: ceil(850_000) - 36_700 = 813_300, rendered as "813k"
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let all = all_text(&lines);
        assert!(
            all.contains("~813k tokens remaining"),
            "expected `~813k tokens remaining`, got:\n{all}"
        );
    }

    #[test]
    fn fmt_tok_big_switches_to_millions_at_one_million() {
        assert_eq!(fmt_tok_big(999_999), fmt_tok(999_999)); // delegates below 1m
        assert_eq!(fmt_tok_big(1_000_000), "1.0m");
        assert_eq!(fmt_tok_big(1_500_000), "1.5m");
        assert_eq!(fmt_tok_big(2_345_678), "2.3m");
        assert_eq!(fmt_tok_big(10_000_000), "10.0m");
    }

    #[test]
    fn fmt_tok_boundaries() {
        assert_eq!(fmt_tok(0), "0");
        assert_eq!(fmt_tok(999), "999");
        assert_eq!(fmt_tok(1_000), "1.0k");
        // 99_499 is the last value below the `{:.1}k` cutover; it formats with one decimal
        // 99_500 crosses into the integer-rounded branch and emits `100k` (4 chars) rather than `{:.1}k`'s round-up of `99.5` to `100.0k` (6 chars)
        // See `fmt_tok` doc comment
        assert_eq!(fmt_tok(99_499), "99.5k");
        assert_eq!(fmt_tok(99_500), "100k");
        assert_eq!(fmt_tok(99_999), "100k");
        assert_eq!(fmt_tok(100_000), "100k");
        assert_eq!(fmt_tok(999_999), "1000k");
    }

    #[test]
    fn build_lines_summary_uses_millions_for_total() {
        let mut snap = snapshot();
        snap.total = 2_000_000;
        snap.used = 36_700;
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let l2 = line_text(&lines, 2);
        assert!(
            l2.contains("/ 2.0m tokens"),
            "expected `/ 2.0m tokens` in summary line, got {l2:?}"
        );
    }

    #[test]
    fn build_lines_shows_imminent_auto_compact_at_threshold() {
        let mut snap = snapshot();
        snap.usage_pct = 85;
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let all = all_text(&lines);
        assert!(
            all.contains("Auto-compact triggers next turn"),
            "expected `Auto-compact triggers next turn` line, got:\n{all}"
        );
    }

    // Bar partition tests. The bar lives at line indices 5.(5+layout.rows) (after header / blank / tokens / model /
    // blank). Each row is rendered as `glyph` spans separated by raw-space spans. To count cells per category, we walk
    // the bar lines for the given layout and count spans whose content matches each category's glyph.

    const SYSTEM_GLYPH_TEST: &str = "\u{25C6}";
    const TOOLS_GLYPH_TEST: &str = "\u{25C8}";
    const MESSAGES_GLYPH_TEST: &str = "\u{25C6}"; // same as system; distinguished by color in real render
    const FREE_GLYPH_TEST: &str = "\u{25C7}";

    /// `layout` tells the function how many bar rows to slice (5 for WIDE, 10 for NARROW).
    /// Without it the slice would be wrong for the narrow layout and the assertions would fail spuriously.
    fn count_bar_glyphs(
        lines: &[Line<'static>],
        layout: BarLayout,
    ) -> (usize, usize, usize, usize) {
        let mut diamonds = 0usize;
        let mut tools = 0usize;
        let mut free = 0usize;
        let bar_start = 5; // header / blank / tokens / model / blank
        let bar_end = bar_start + layout.rows;
        for line in lines.get(bar_start..bar_end).into_iter().flatten() {
            for span in &line.spans {
                let c = span.content.as_ref();
                if c == SYSTEM_GLYPH_TEST || c == MESSAGES_GLYPH_TEST {
                    // SYSTEM_GLYPH_TEST == MESSAGES_GLYPH_TEST; counted together.
                    diamonds += 1;
                } else if c == TOOLS_GLYPH_TEST {
                    tools += 1;
                } else if c == FREE_GLYPH_TEST {
                    free += 1;
                }
            }
        }
        (diamonds, tools, free, diamonds + tools + free)
    }

    #[test]
    fn window_tokens_fill_in_legend_order_and_cap_at_the_window() {
        let usage = ProviderUsage {
            cache_read: 400,
            cache_write: 300,
            input: 200,
            output: 100,
        };
        let t = window_tokens(&usage, 1_000);
        assert_eq!((t.used, t.free(1_000)), (1_000, 0));
        let t = window_tokens(&usage, 800);
        assert_eq!(
            (t.cache_read, t.cache_write, t.input, t.output, t.used),
            (400, 300, 100, 0, 800)
        );
        assert_eq!(t.free(800), 0);
    }

    #[test]
    fn legend_shows_the_provider_counts_and_no_estimates() {
        let all = all_text(
            &ContextInfoBlock::new(snapshot(), "grok-4")
                .build_lines(&test_theme(), BarLayout::WIDE),
        );
        for (label, tokens) in [
            ("Cache read", "29.9k"),
            ("Cache write", "5.0k"),
            ("Input", "1.2k"),
            ("Output", "600"),
            ("Free", "963k"),
        ] {
            assert!(
                all.lines().any(|l| l.contains(label) && l.contains(tokens)),
                "{label} row should read {tokens}:\n{all}"
            );
        }
        for gone in [
            "Overhead",
            "System prompt",
            "Messages",
            "Already counted above",
        ] {
            assert!(!all.contains(gone), "{gone} is an estimate:\n{all}");
        }
    }

    #[test]
    fn no_reported_request_shows_an_empty_bar_and_says_why() {
        let mut snap = snapshot();
        snap.request_ledger = Default::default();
        let lines =
            ContextInfoBlock::new(snap, "grok-4").build_lines(&test_theme(), BarLayout::WIDE);
        let all = all_text(&lines);
        assert!(all.contains("No usage reported yet"), "{all}");
        assert!(all.contains("No request has reported usage"), "{all}");
        let (diamonds, _, free, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!((diamonds, free, total), (0, 100, 100));
    }

    #[test]
    fn bar_used_band_matches_the_provider_total() {
        let mut snap = snapshot();
        snap.total = 100_000;
        snap.request_ledger.requests[0].usage = ProviderUsage {
            cache_read: 6_000,
            cache_write: 2_000,
            input: 1_500,
            output: 500,
        };
        let lines =
            ContextInfoBlock::new(snap, "grok-4").build_lines(&test_theme(), BarLayout::WIDE);
        let (diamonds, _, free, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!((diamonds, free, total), (10, 90, 100));
    }

    #[test]
    fn bar_all_used_when_completely_full() {
        let mut snap = snapshot();
        snap.total = 1_000;
        snap.request_ledger.requests[0].usage = ProviderUsage {
            cache_read: 2_000,
            ..Default::default()
        };
        let lines =
            ContextInfoBlock::new(snap, "grok-4").build_lines(&test_theme(), BarLayout::WIDE);
        let (diamonds, _, free, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!((diamonds, free, total), (100, 0, 100));
    }

    #[test]
    fn bar_all_free_when_total_is_zero() {
        let mut snap = snapshot();
        snap.total = 0;
        snap.used = 0;
        let lines =
            ContextInfoBlock::new(snap, "grok-4").build_lines(&test_theme(), BarLayout::WIDE);
        let (diamonds, tools, free, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!((diamonds, tools, free, total), (0, 0, 100, 100));
    }

    #[test]
    fn bar_total_cells_always_sum_to_one_hundred() {
        let lines =
            ContextInfoBlock::new(snapshot(), "grok-4").build_lines(&test_theme(), BarLayout::WIDE);
        let (_, _, _, total) = count_bar_glyphs(&lines, BarLayout::WIDE);
        assert_eq!(total, 100, "bar must always render exactly 100 cells");
    }
    #[test]
    fn bar_geometry_maps_columns_and_rows_to_cells() {
        let bar = BarGeometry {
            first_line: 5,
            rows: 5,
            row_len: 20,
        };
        assert_eq!(bar.cell_at(0, 5), Some(0));
        assert_eq!(bar.cell_at(1, 5), Some(0), "the gap after a cell picks it");
        assert_eq!(bar.cell_at(38, 9), Some(99));
        assert_eq!(bar.cell_at(39, 9), None, "past the last cell");
        assert_eq!(bar.cell_at(0, 4), None);
        assert_eq!(bar.cell_at(0, 10), None);
    }

    #[test]
    fn modal_view_falls_back_to_one_column_when_narrow() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let view = block.modal_view(&theme, 60, None, &BTreeSet::new());
        let all = all_text(&view.lines);
        let line_of = |needle: &str| all.lines().position(|l| l.contains(needle));
        assert!(
            line_of("Cache read") > line_of("Click a diamond"),
            "totals go under the bar:\n{all}"
        );
        assert_eq!(view.bar.row_len, BarLayout::WIDE.row_len);

        let wide = block.modal_view(&theme, 110, None, &BTreeSet::new());
        assert!(
            line_text(&wide.lines, 0).contains("Cache read"),
            "a wide view puts the totals beside the header"
        );
    }

    #[test]
    fn modal_view_segments_tile_the_bar_and_selection_dims_the_rest() {
        let mut snap = snapshot();
        snap.request_ledger.requests[0].usage = ProviderUsage {
            cache_read: 400_000,
            ..Default::default()
        };
        let block = ContextInfoBlock::new(snap, "grok-4");
        let theme = test_theme();
        let view = block.modal_view(&theme, 110, Some(FREE_LABEL), &BTreeSet::new());
        let covered: usize = view.segments.iter().map(|(r, _)| r.len()).sum();
        assert_eq!(covered, 100);
        let all = all_text(&view.lines);
        assert!(all.contains("Cells 41\u{2013}100 of 100"), "{all}");
        assert!(all.contains("Auto-compact at 85%"), "{all}");

        let dim = Some(quantize(theme.gray_dim));
        let first_cell = view
            .lines
            .get(BAR_FIRST_LINE)
            .and_then(|l| l.spans.first())
            .expect("bar row has cells");
        assert_eq!(first_cell.style.fg, dim, "unselected cells go dim");
    }

    #[test]
    fn modal_view_ignores_a_selection_that_has_no_cells() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let view = block.modal_view(&test_theme(), 110, Some("Reasoning"), &BTreeSet::new());
        assert!(all_text(&view.lines).contains("Click a diamond"));
    }

    #[test]
    fn percent_of_window_returns_dash_for_zero_total() {
        // Exercised the `total == 0` early return: pct can't be computed
        assert_eq!(percent_of_window(100, 0), "-");
        assert_eq!(percent_of_window(0, 0), "-");
    }

    #[test]
    fn percent_of_window_formatting() {
        assert_eq!(percent_of_window(1, 1_000_000), "0.1%");
        assert_eq!(percent_of_window(0, 1_000_000), "0.0%");
        assert_eq!(percent_of_window(50_000, 1_000_000), "5.0%");
        assert_eq!(percent_of_window(500_000, 1_000_000), "50%");
    }

    // The bar's shape (5×20 vs 10×10) is chosen by `BarLayout::for_width` based on terminal width
    // Narrow terminals thus get a square bar that still fits in their column budget

    #[test]
    fn bar_layout_for_width_picks_wide_at_breakpoint_and_above() {
        // At the breakpoint and above, the wide layout is selected.
        assert_eq!(
            BarLayout::for_width(BarLayout::NARROW_BREAKPOINT),
            BarLayout::WIDE
        );
        assert_eq!(BarLayout::for_width(80), BarLayout::WIDE);
        assert_eq!(BarLayout::for_width(200), BarLayout::WIDE);
        assert_eq!(BarLayout::for_width(u16::MAX), BarLayout::WIDE);
    }

    #[test]
    fn bar_layout_for_width_picks_narrow_below_breakpoint() {
        // Below the breakpoint, the narrow layout is selected so the bar fits without being clipped
        assert_eq!(
            BarLayout::for_width(BarLayout::NARROW_BREAKPOINT - 1),
            BarLayout::NARROW
        );
        assert_eq!(BarLayout::for_width(40), BarLayout::NARROW);
        assert_eq!(BarLayout::for_width(20), BarLayout::NARROW);
        assert_eq!(BarLayout::for_width(0), BarLayout::NARROW);
    }

    #[test]
    fn narrow_bar_renders_10_rows() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        // The bar starts at index 5 (header / blank / tokens / model / blank)
        // Each of the next 10 rows must be a non-empty bar row
        for (offset, line) in lines.get(5..15).into_iter().flatten().enumerate() {
            assert!(
                !line.spans.is_empty(),
                "narrow bar row {offset} must be non-empty"
            );
        }
        // The line right after the bar is the spacer blank
        let after_bar: String = lines
            .get(15)
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_else(|| panic!("expected line 15 after narrow bar"));
        assert_eq!(after_bar, "", "expected blank line after narrow bar");
    }

    #[test]
    fn narrow_bar_total_cells_still_100() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        let (_, _, _, total) = count_bar_glyphs(&lines, BarLayout::NARROW);
        assert_eq!(total, 100, "narrow bar must still render exactly 100 cells");
    }

    #[test]
    fn narrow_bar_each_row_has_at_most_10_cells() {
        // Sanity: no single bar row exceeds the narrow row_len
        // We count cell glyphs (not separator spaces) per row.
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        let bar = lines
            .get(5..15)
            .unwrap_or_else(|| panic!("expected 10 narrow bar rows, got {}", lines.len()));
        for (offset, line) in bar.iter().enumerate() {
            let cell_count = line
                .spans
                .iter()
                .filter(|s| {
                    let c = s.content.as_ref();
                    c == SYSTEM_GLYPH_TEST
                        || c == TOOLS_GLYPH_TEST
                        || c == MESSAGES_GLYPH_TEST
                        || c == FREE_GLYPH_TEST
                })
                .count();
            assert!(
                cell_count <= BarLayout::NARROW.row_len,
                "row {offset} has {cell_count} cells, expected <= {}",
                BarLayout::NARROW.row_len
            );
        }
    }

    #[test]
    fn wide_bar_each_row_has_at_most_20_cells() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let bar = lines
            .get(5..10)
            .unwrap_or_else(|| panic!("expected 5 wide bar rows, got {}", lines.len()));
        for (offset, line) in bar.iter().enumerate() {
            let cell_count = line
                .spans
                .iter()
                .filter(|s| {
                    let c = s.content.as_ref();
                    c == SYSTEM_GLYPH_TEST
                        || c == TOOLS_GLYPH_TEST
                        || c == MESSAGES_GLYPH_TEST
                        || c == FREE_GLYPH_TEST
                })
                .count();
            assert!(
                cell_count <= BarLayout::WIDE.row_len,
                "row {offset} has {cell_count} cells, expected <= {}",
                BarLayout::WIDE.row_len
            );
        }
    }

    /// Helper: find the legend row (or row 1, for the narrow layout) whose label text starts with `label_prefix`.
    /// Returns the matching `Line` so the caller can assert on its spans.
    fn find_legend_line<'a>(
        lines: &'a [Line<'static>],
        label_prefix: &str,
    ) -> Option<&'a Line<'static>> {
        lines.iter().find(|line| {
            line.spans
                .iter()
                .any(|s| s.content.as_ref().trim_start().starts_with(label_prefix))
        })
    }

    #[test]
    fn legend_label_uses_secondary_color_wide() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let row = find_legend_line(&lines, "Cache read").expect("legend row");
        // Span layout for WIDE: [glyph+space, label, " ", tokens, ...].
        // The label span content begins with "Cache read".
        let label_span = row
            .spans
            .iter()
            .find(|s| s.content.as_ref().starts_with("Cache read"))
            .expect("label span");
        assert_eq!(
            label_span.style.fg,
            Some(theme.text_secondary),
            "label should use text_secondary, got {:?}",
            label_span.style.fg
        );
    }

    #[test]
    fn legend_label_uses_secondary_color_narrow() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        let row = find_legend_line(&lines, "Cache read").expect("legend row 1");
        // Span layout for NARROW row 1: [glyph+space, label].
        let label_span = row
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Cache read")
            .expect("label span");
        assert_eq!(
            label_span.style.fg,
            Some(theme.text_secondary),
            "narrow label should use text_secondary, got {:?}",
            label_span.style.fg
        );
    }

    #[test]
    fn narrow_legend_wraps_to_two_lines_per_category() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        let categories = [
            ("Cache read", "29.9k"),
            ("Cache write", "5.0k"),
            ("Input", "1.2k"),
            ("Output", "600"),
            ("Free", "963k"),
        ];
        let mut idx = 16;
        for (label, expected_tokens) in categories {
            let row1: String = lines
                .get(idx)
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .unwrap_or_else(|| panic!("expected legend row {idx}"));
            let row2: String = lines
                .get(idx + 1)
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .unwrap_or_else(|| panic!("expected legend row {}", idx + 1));
            assert!(
                row1.contains(label),
                "expected row {idx} to contain `{label}`, got: {row1:?}"
            );
            assert!(
                row2.contains(expected_tokens),
                "expected row {} to contain `{expected_tokens}` for `{label}`, got: {row2:?}",
                idx + 1
            );
            idx += 2;
        }
    }

    #[test]
    fn narrow_legend_data_row_starts_with_one_space_indent() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::NARROW);
        // The data row for the first legend entry sits at index 17 (16 is the "Cache read" header row, 17 its data row)
        let Some(data_row) = lines.get(17) else {
            panic!("expected data row at index 17");
        };
        let first = data_row
            .spans
            .first()
            .expect("data row should have a leading indent span");
        assert_eq!(
            first.content.as_ref(),
            " ",
            "narrow data row must start with exactly one space, got {:?}",
            first.content.as_ref()
        );
    }

    #[test]
    fn wide_legend_remains_single_line_per_category() {
        let block = ContextInfoBlock::new(snapshot(), "grok-4");
        let theme = test_theme();
        let lines = block.build_lines(&theme, BarLayout::WIDE);
        let row_text = |i: usize| -> String {
            lines
                .get(i)
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .unwrap_or_else(|| panic!("expected legend row {i}"))
        };
        let l11 = row_text(11);
        assert!(
            l11.contains("Cache read") && l11.contains("29.9k"),
            "wide legend should keep label + tokens on one line, got: {l11:?}"
        );
        let l12 = row_text(12);
        assert!(
            l12.contains("Cache write") && l12.contains("5.0k"),
            "wide legend should keep label + tokens on one line, got: {l12:?}"
        );
        let l13 = row_text(13);
        assert!(
            l13.contains("Input") && l13.contains("1.2k"),
            "wide legend should keep label + tokens on one line, got: {l13:?}"
        );
        let l15 = row_text(15);
        assert!(
            l15.contains("Free") && l15.contains("963k"),
            "wide legend should keep label + tokens on one line, got: {l15:?}"
        );
    }
}
