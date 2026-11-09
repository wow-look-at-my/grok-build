//! [`AgentStatusBar`] collects items as `Line<'static>` spans, lays them out right-aligned with faint `│` separators, and renders into a buffer row.
//! Returns hit-test areas keyed by item ID.
//!
//! # Example
//!
//! ```ignore
//! let mut status = AgentStatusBar::new(&theme);
//! status.push("context", context_line);
//! status.push("queue", queue_line);
//! let areas = status.render(buf, status_bar_rect);
//! let context_area = areas.get("context");
//! ```

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::context_bar::SEPARATOR;
use super::turn_status::SPINNER_DIVISOR;
use crate::app::agent::{GoalDisplayPhase, GoalDisplayState, GoalDisplayStatus};
use crate::app::agent_view::McpInitProgress;
use crate::theme::Theme;
use crate::views::tasks_pane::TaskStatusCounts;
use crate::views::todo_pane::TodoCounts;

/// A named status bar item.
struct StatusEntry {
    /// Identifier for hit-test lookup (e.g., "context", "queue").
    id: &'static str,
    /// Pre-built styled content.
    line: Line<'static>,
    /// Display width in columns.
    width: u16,
}

/// Builder for the agent status bar.
///
/// Collect items with [`push`], then call [`render`] to lay them out right-aligned with separators and get back hit-test areas.
pub struct AgentStatusBar<'a> {
    items: Vec<StatusEntry>,
    theme: &'a Theme,
    /// Padding from the right edge of the status bar area.
    right_pad: u16,
}

impl<'a> AgentStatusBar<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            items: Vec::new(),
            theme,
            right_pad: 0,
        }
    }

    /// Items are rendered left-to-right in push order, but the entire group is right-aligned within the status bar area.
    pub fn push(&mut self, id: &'static str, line: Line<'static>) {
        let width = line.width() as u16;
        self.items.push(StatusEntry { id, line, width });
    }

    /// Columns the group occupies: items plus the separators between them.
    fn width(&self) -> u16 {
        let items: u16 = self.items.iter().map(|e| e.width).sum();
        let seps = (self.items.len() as u16).saturating_sub(1);
        items + seps * SEPARATOR_WIDTH
    }

    /// Render all items right-aligned into the given area. Layout: `··· item0 │ item1 │ item2`;
    /// separators appear only between items, never before the first or after the last. Returns a map
    /// from item ID to screen `Rect` for hit-testing.
    pub fn render(self, buf: &mut Buffer, area: Rect) -> HashMap<&'static str, Rect> {
        if area.height == 0 || area.width == 0 || self.items.is_empty() {
            return HashMap::new();
        }

        buf.set_style(area, Style::default().bg(self.theme.bg_base));

        let sep = separator(self.theme);
        let sep_w = SEPARATOR_WIDTH;

        // Right-align: compute starting x
        let start_x = area
            .x
            .saturating_add(area.width.saturating_sub(self.right_pad + self.width()));

        let mut x = start_x;
        let mut areas = HashMap::new();

        for (i, entry) in self.items.iter().enumerate() {
            // Separator before every item except the first.
            if i > 0 {
                buf.set_span(x, area.y, &sep, sep_w);
                x += sep_w;
            }

            // Render item
            buf.set_line(x, area.y, &entry.line, entry.width);
            areas.insert(
                entry.id,
                Rect {
                    x,
                    y: area.y,
                    width: entry.width,
                    height: 1,
                },
            );
            x += entry.width;
        }

        areas
    }
}

/// ` │ ` a step fainter than the chips it divides — the divider between status-bar items and between the header's title and
/// location.
pub(crate) fn separator(theme: &Theme) -> Span<'static> {
    Span::styled(format!(" {SEPARATOR} "), theme.faint().bg(theme.bg_base))
}

/// Display width of [`separator`].
const SEPARATOR_WIDTH: u16 = 3;

pub(crate) fn task_status_line(
    counts: TaskStatusCounts,
    theme: &Theme,
    is_hovered: bool,
) -> Option<Line<'static>> {
    if counts == TaskStatusCounts::default() {
        return None;
    }

    let hover = if is_hovered {
        ratatui::style::Modifier::BOLD
    } else {
        ratatui::style::Modifier::empty()
    };
    let running_style = Style::default()
        .fg(theme.accent_running)
        .bg(theme.bg_base)
        .add_modifier(hover);
    let paused_style = Style::default()
        .fg(theme.warning)
        .bg(theme.bg_base)
        .add_modifier(hover);
    let mut spans = Vec::with_capacity(2);

    if counts.running > 0 {
        // Static, and the same diamond the task rows use
        // The header sits in view the whole session, so a spinner here reads as noise rather than progress
        spans.push(Span::styled(
            format!("{} {}", crate::glyphs::diamond_filled(), counts.running),
            running_style,
        ));
    }
    if counts.paused_workflows > 0 {
        let prefix = if counts.running > 0 { "  " } else { "" };
        spans.push(Span::styled(
            format!("{prefix}P {}", counts.paused_workflows),
            paused_style,
        ));
    }

    Some(Line::from(spans))
}

/// Format a token count compactly: `500`, `1.5k`, `50k`, `1.5M`.
pub(crate) fn format_tokens_compact(tokens: i64) -> String {
    let sign = if tokens < 0 { "-" } else { "" };
    let abs = tokens.unsigned_abs();
    if abs >= 1_000_000 {
        let m = abs as f64 / 1_000_000.0;
        format!("{sign}{}", format!("{m:.1}M").replace(".0M", "M"))
    } else if abs >= 1_000 {
        let k = abs as f64 / 1_000.0;
        format!("{sign}{}", format!("{k:.1}k").replace(".0k", "k"))
    } else {
        tokens.to_string()
    }
}

/// The word the goal chip adds to the todo count. `None` while the goal runs.
/// Every pause reason reads "Paused": the goal detail names the reason.
fn goal_state_word(goal: &GoalDisplayState) -> Option<&'static str> {
    match goal.status {
        GoalDisplayStatus::Active if goal.verifying_completion => Some("Verifying"),
        GoalDisplayStatus::Active if goal.planning || goal.phase == GoalDisplayPhase::Planning => {
            Some("Planning")
        }
        GoalDisplayStatus::Active => None,
        GoalDisplayStatus::UserPaused
        | GoalDisplayStatus::BackOffPaused
        | GoalDisplayStatus::NoProgressPaused
        | GoalDisplayStatus::InfraPaused
        | GoalDisplayStatus::Blocked => Some("Paused"),
        GoalDisplayStatus::Failed => Some("Failed"),
        GoalDisplayStatus::Interrupted => Some("Interrupted"),
        GoalDisplayStatus::BudgetLimited => Some("Stopped"),
        GoalDisplayStatus::Complete => Some("Done"),
    }
}

/// Live phase suffix for an Active goal, shared by the status chip and the goal-detail modal so they cannot disagree.
/// The transient `verifying_completion` overlay wins, then `planning`, then the steady-state phase.
pub fn active_phase_label(goal: &GoalDisplayState) -> String {
    if goal.verifying_completion {
        let attempts = classifier_attempts_label(goal);
        // Omit the "(n/m)" suffix until the first counter arrives so the chip reads "Verifying" instead of a confusing "Verifying (0/0)"
        return if attempts.is_empty() {
            "Verifying".into()
        } else {
            format!("Verifying ({attempts})")
        };
    }
    if goal.planning {
        return "Planning".into();
    }
    match goal.phase {
        GoalDisplayPhase::Idle => "Idle".into(),
        GoalDisplayPhase::Planning => "Planning".into(),
        GoalDisplayPhase::Executing => "Executing".into(),
    }
}

/// Format the classifier "attempts: n/m" counter for both the status chip and the modal so the two displays cannot drift.
/// Returns the empty string when both fields are absent / zero: no classifier run has been reserved yet, so there is no meaningful counter.
/// Callers render it only when non-empty: the chip drops the `(n/m)` suffix, the modal falls back to a hyphen.
pub fn classifier_attempts_label(goal: &GoalDisplayState) -> String {
    let attempt = goal.classifier_runs_attempted.unwrap_or(0);
    let max = goal.classifier_max_runs.unwrap_or(0);
    if attempt == 0 && max == 0 {
        return String::new();
    }
    format!("{attempt}/{max}")
}

/// The goal chip: `[Goal: 4/5]`, or `[Goal: Paused 4/5]` when the goal does
/// not run. The count is the todo list's `done/total`, cancelled items left
/// out. `None` for a running goal with no todo to count: such a goal is
/// invalid, and the caller reports it. Only a running goal is gold. Failed
/// and interrupted are a red chip. Every other state is gray.
pub fn goal_status_line(
    goal: &GoalDisplayState,
    counts: &TodoCounts,
    theme: &Theme,
    hovered: bool,
) -> Option<Line<'static>> {
    let total = counts.total_excluding_cancelled();
    let count = (total > 0).then(|| format!("{}/{total}", counts.completed));
    let text = match (goal_state_word(goal), count) {
        (Some(word), Some(count)) => format!("Goal: {word} {count}"),
        (None, Some(count)) => format!("Goal: {count}"),
        (Some(word), None) => format!("Goal: {word}"),
        (None, None) => return None,
    };
    let dim_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    let mut label_style = match goal.status {
        GoalDisplayStatus::Active => Style::default().fg(theme.accent_plan).bg(theme.bg_base),
        GoalDisplayStatus::Failed | GoalDisplayStatus::Interrupted => {
            Style::default().fg(theme.bg_base).bg(theme.accent_error)
        }
        _ => Style::default().fg(theme.gray).bg(theme.bg_base),
    };
    if hovered {
        label_style = label_style
            .add_modifier(ratatui::style::Modifier::BOLD)
            .add_modifier(ratatui::style::Modifier::UNDERLINED);
    }
    Some(Line::from(vec![
        Span::styled("[", dim_style),
        Span::styled(text, label_style),
        Span::styled("]", dim_style),
    ]))
}

/// The chip shows real server counts only. "Starting session…" is the turn-status row's job
/// (see [`crate::views::turn_status`]).
pub fn mcp_status_line(
    progress: &McpInitProgress,
    tick: u64,
    theme: &Theme,
) -> Option<Line<'static>> {
    if progress.total == 0 {
        return None;
    }
    let frames = crate::glyphs::braille_spinner_frames();
    let frame_idx = (tick / SPINNER_DIVISOR) as usize % frames.len();
    let style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    Some(Line::from(vec![
        Span::styled(
            format!("{} ", frames.get(frame_idx).copied().unwrap_or("")),
            style,
        ),
        Span::styled(
            format!("MCP ({}/{})", progress.connected, progress.total),
            style,
        ),
    ]))
}

#[cfg(test)]
#[path = "agent_status_task_tests.rs"]
mod task_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compact_sub_thousand() {
        assert_eq!(format_tokens_compact(0), "0");
        assert_eq!(format_tokens_compact(500), "500");
        assert_eq!(format_tokens_compact(999), "999");
    }

    #[test]
    fn tokens_compact_thousands() {
        assert_eq!(format_tokens_compact(1000), "1k");
        assert_eq!(format_tokens_compact(1500), "1.5k");
        assert_eq!(format_tokens_compact(12300), "12.3k");
        assert_eq!(format_tokens_compact(50000), "50k");
        assert_eq!(format_tokens_compact(100000), "100k");
    }

    #[test]
    fn tokens_compact_negative() {
        assert_eq!(format_tokens_compact(-500), "-500");
        assert_eq!(format_tokens_compact(-1500), "-1.5k");
        assert_eq!(format_tokens_compact(-1_000_000), "-1M");
    }

    #[test]
    fn tokens_compact_millions() {
        assert_eq!(format_tokens_compact(1_000_000), "1M");
        assert_eq!(format_tokens_compact(1_500_000), "1.5M");
        assert_eq!(format_tokens_compact(10_000_000), "10M");
    }

    fn make_goal(
        status: GoalDisplayStatus,
        phase: GoalDisplayPhase,
        idx: Option<u32>,
        total: u32,
        completed: u32,
    ) -> GoalDisplayState {
        GoalDisplayState {
            goal_id: "g-1".into(),
            objective: "Build widget".into(),
            status,
            phase,
            token_budget: Some(50_000),
            tokens_used: 12_300,
            elapsed_ms: 180_000,
            total_deliverables: total,
            completed_deliverables: completed,
            current_deliverable_id: idx,
            current_deliverable_title: Some("Add CSS vars".into()),
            current_subagent_role: None,
            total_worker_rounds: 3,
            total_verify_rounds: 1,
            live_subagent_tokens: None,
            live_tokens_by_model: Vec::new(),
            live_context_pct: None,
            live_turn_count: None,
            live_tool_call_count: None,
            last_event: None,
            last_event_detail: None,
            last_event_timestamp: None,
            token_baseline: 0,
            finished_subagent_tokens: 0,
            deliverables: vec![],
            pause_message: None,
            classifier_runs_attempted: None,
            classifier_max_runs: None,
            last_classifier_verdict: None,
            last_classifier_details_path: None,
            last_classifier_details_exists: false,
            verifying_completion: false,
            planning: false,
            received_at: std::time::Instant::now(),
            elapsed_floor_ms: 0,
        }
    }

    #[test]
    fn classifier_attempts_label_empty_when_both_absent_or_zero() {
        let mut g = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            None,
            0,
            0,
        );
        // Both None gives empty (no run reserved yet)
        assert_eq!(classifier_attempts_label(&g), "");
        // Explicit zeros also count as "no counter".
        g.classifier_runs_attempted = Some(0);
        g.classifier_max_runs = Some(0);
        assert_eq!(classifier_attempts_label(&g), "");
        // A configured cap (max > 0) makes the counter meaningful.
        g.classifier_max_runs = Some(3);
        assert_eq!(classifier_attempts_label(&g), "0/3");
        g.classifier_runs_attempted = Some(2);
        assert_eq!(classifier_attempts_label(&g), "2/3");
    }

    #[test]
    fn live_elapsed_ms_clamps_to_carried_floor() {
        // The displayed clock must never tick below the carried monotonic floor, even when the latest authoritative base is lower
        // (The pager's extrapolation outran the shell's flush point.)
        let mut g = make_goal(
            GoalDisplayStatus::UserPaused,
            GoalDisplayPhase::Idle,
            None,
            0,
            0,
        );
        g.elapsed_ms = 1_000;
        g.elapsed_floor_ms = 5_000;
        assert_eq!(g.live_elapsed_ms(), 5_000);
    }

    #[test]
    fn live_elapsed_ms_uses_live_value_when_above_floor() {
        let mut g = make_goal(
            GoalDisplayStatus::UserPaused,
            GoalDisplayPhase::Idle,
            None,
            0,
            0,
        );
        g.elapsed_ms = 9_000;
        g.elapsed_floor_ms = 5_000;
        assert_eq!(g.live_elapsed_ms(), 9_000);
    }

    fn counts(completed: usize, pending: usize) -> TodoCounts {
        TodoCounts {
            completed,
            pending,
            cancelled: 1,
            ..Default::default()
        }
    }

    fn chip(goal: &GoalDisplayState, counts: &TodoCounts) -> Option<String> {
        goal_status_line(goal, counts, &Theme::current(), false)
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
    }

    fn label_span(goal: &GoalDisplayState, theme: &Theme) -> Span<'static> {
        goal_status_line(goal, &counts(4, 1), theme, false)
            .expect("a goal with todos draws a chip")
            .spans
            .into_iter()
            .find(|s| s.content.contains("Goal:"))
            .expect("label span with the `Goal:` prefix")
    }

    #[test]
    fn a_running_goal_shows_only_the_todo_count() {
        let g = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            Some(1),
            4,
            1,
        );
        assert_eq!(
            chip(&g, &counts(4, 1)).as_deref(),
            Some("[Goal: 4/5]"),
            "no phase, tokens, budget, elapsed time or spinner; cancelled items stay out"
        );
    }

    #[test]
    fn a_goal_that_does_not_simply_run_keeps_one_state_word() {
        let mut verifying = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            None,
            0,
            0,
        );
        verifying.verifying_completion = true;
        verifying.classifier_runs_attempted = Some(1);
        verifying.classifier_max_runs = Some(3);
        assert_eq!(
            chip(&verifying, &counts(4, 1)).as_deref(),
            Some("[Goal: Verifying 4/5]"),
            "the attempt counter stays in the goal detail"
        );
        for (status, word) in [
            (GoalDisplayStatus::UserPaused, "Paused"),
            (GoalDisplayStatus::BackOffPaused, "Paused"),
            (GoalDisplayStatus::NoProgressPaused, "Paused"),
            (GoalDisplayStatus::InfraPaused, "Paused"),
            (GoalDisplayStatus::Blocked, "Paused"),
            (GoalDisplayStatus::Failed, "Failed"),
            (GoalDisplayStatus::Interrupted, "Interrupted"),
            (GoalDisplayStatus::BudgetLimited, "Stopped"),
            (GoalDisplayStatus::Complete, "Done"),
        ] {
            let mut g = make_goal(status, GoalDisplayPhase::Executing, None, 0, 0);
            g.verifying_completion = true;
            g.planning = true;
            assert_eq!(
                chip(&g, &counts(4, 1)),
                Some(format!("[Goal: {word} 4/5]")),
                "{status:?} shows its own word, not the in-flight one"
            );
        }
    }

    #[test]
    fn a_planning_goal_shows_planning_before_it_has_todos() {
        let mut g = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Idle,
            None,
            0,
            0,
        );
        g.planning = true;
        assert_eq!(
            chip(&g, &TodoCounts::default()).as_deref(),
            Some("[Goal: Planning]")
        );
    }

    #[test]
    fn a_running_goal_with_no_todos_draws_no_chip() {
        let g = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            None,
            0,
            0,
        );
        let only_cancelled = TodoCounts {
            cancelled: 2,
            ..Default::default()
        };
        assert_eq!(chip(&g, &TodoCounts::default()), None);
        assert_eq!(chip(&g, &only_cancelled), None);
    }

    #[test]
    fn a_finished_goal_whose_list_was_archived_still_says_done() {
        let g = make_goal(
            GoalDisplayStatus::Complete,
            GoalDisplayPhase::Idle,
            None,
            0,
            0,
        );
        let archived = TodoCounts {
            archived: 5,
            ..Default::default()
        };
        assert_eq!(chip(&g, &archived).as_deref(), Some("[Goal: Done]"));
    }

    #[test]
    fn only_a_running_goal_is_gold() {
        let t = Theme::groknight();
        let running = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            None,
            0,
            0,
        );
        assert_eq!(label_span(&running, &t).style.fg, Some(t.accent_plan));
        for status in [
            GoalDisplayStatus::UserPaused,
            GoalDisplayStatus::InfraPaused,
            GoalDisplayStatus::BudgetLimited,
            GoalDisplayStatus::Complete,
        ] {
            let span = label_span(&make_goal(status, GoalDisplayPhase::Idle, None, 0, 0), &t);
            assert_eq!(span.style.fg, Some(t.gray), "{status:?} is gray");
            assert_eq!(
                span.style.bg,
                Some(t.bg_base),
                "{status:?} has no chip background"
            );
        }
        for status in [GoalDisplayStatus::Failed, GoalDisplayStatus::Interrupted] {
            let span = label_span(&make_goal(status, GoalDisplayPhase::Idle, None, 0, 0), &t);
            assert_eq!(
                span.style.bg,
                Some(t.accent_error),
                "{status:?} is a red chip"
            );
        }
    }

    #[test]
    fn the_goal_chip_never_names_the_objective() {
        let g = make_goal(
            GoalDisplayStatus::Active,
            GoalDisplayPhase::Executing,
            None,
            1,
            0,
        );
        let text = chip(&g, &counts(0, 1)).unwrap();
        assert!(!text.contains("Add CSS vars"));
        assert!(!text.contains("Build widget"));
    }

    #[test]
    fn mcp_status_line_renders_compact_count() {
        // total > 0 renders the compact `MCP (connected/total)` chip.
        let progress = McpInitProgress {
            total: 4,
            connected: 1,
        };
        let t = Theme::current();
        let line = mcp_status_line(&progress, 0, &t).expect("total > 0 must render a line");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("MCP (1/4)"),
            "expected 'MCP (1/4)', got: {text:?}"
        );
    }

    #[test]
    fn mcp_status_line_uses_dim_directory_color() {
        // The chip must render in `theme.gray_dim` to match the directory path.
        let t = Theme::groknight();
        let progress = McpInitProgress {
            total: 2,
            connected: 0,
        };
        let line = mcp_status_line(&progress, 0, &t).expect("total > 0 must render a line");
        for span in &line.spans {
            assert_eq!(
                span.style.fg,
                Some(t.gray_dim),
                "MCP chip spans must use theme.gray_dim"
            );
        }
    }

    #[test]
    fn mcp_status_line_hidden_for_zero_total() {
        // A 0-server report renders no chip
        let progress = McpInitProgress {
            total: 0,
            connected: 0,
        };
        let t = Theme::current();
        assert!(mcp_status_line(&progress, 0, &t).is_none());
    }

    /// Separators appear only *between* items, never before the first item or after the last (no leading/trailing divider).
    #[test]
    fn status_bar_separators_only_between_items() {
        let theme = Theme::current();
        let mut bar = AgentStatusBar::new(&theme);
        bar.push("a", Line::from("AA"));
        bar.push("b", Line::from("BB"));
        bar.push("c", Line::from("CC"));

        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        bar.render(&mut buf, area);

        let row: String = (0..area.width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol()))
            .collect();
        let trimmed = row.trim();

        // Exactly two dividers (between the three items), none at the ends.
        assert_eq!(trimmed.matches(SEPARATOR).count(), 2, "row = {trimmed:?}");
        assert!(
            !trimmed.starts_with(SEPARATOR),
            "no leading divider, row = {trimmed:?}"
        );
        assert!(
            !trimmed.ends_with(SEPARATOR),
            "no trailing divider, row = {trimmed:?}"
        );
        assert_eq!(trimmed, format!("AA {SEPARATOR} BB {SEPARATOR} CC"));
    }

    /// A single item renders with no separators at all (it is both first and last).
    #[test]
    fn status_bar_single_item_has_no_separators() {
        let theme = Theme::current();
        let mut bar = AgentStatusBar::new(&theme);
        bar.push("only", Line::from("XX"));

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        bar.render(&mut buf, area);

        let row: String = (0..area.width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol()))
            .collect();
        assert_eq!(row.trim(), "XX");
        assert!(!row.contains(SEPARATOR));
    }
}
