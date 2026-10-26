//! Todo detail popup: one todo's full text, status, priority and completion
//! requirement.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use xai_grok_shell::tools::{TodoItem, TodoPriority, TodoStatus};

use crate::render::SafeBuf;
use crate::theme::Theme;
use crate::views::goal_detail::{strip_control_chars, truncate_to_width, wrap_pause_message_lines};

/// Which todo the popup shows. `index` is a position in `TodoPane::todos()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TodoDetailState {
    pub index: usize,
    pub scroll: u16,
}

impl TodoDetailState {
    pub fn new(index: usize) -> Self {
        Self { index, scroll: 0 }
    }

    /// Move `step` places through `visible` (the todo indices the pane shows) and wrap at the ends.
    /// An index that is no longer visible moves to the first visible todo.
    pub fn cycle(&mut self, visible: &[usize], step: isize) {
        if visible.is_empty() {
            return;
        }
        let len = visible.len() as isize;
        let next = match visible.iter().position(|&i| i == self.index) {
            Some(pos) => (pos as isize + step).rem_euclid(len),
            None => 0,
        };
        self.index = visible[next as usize];
        self.scroll = 0;
    }
}

fn status_label(status: TodoStatus, theme: &Theme) -> (&'static str, Color) {
    match status {
        TodoStatus::Pending => ("Pending", theme.text_primary),
        TodoStatus::InProgress => ("In progress", theme.warning),
        TodoStatus::Completed => ("Completed", theme.accent_success),
        TodoStatus::Cancelled => ("Cancelled", theme.accent_error),
    }
}

fn priority_label(priority: TodoPriority) -> &'static str {
    match priority {
        TodoPriority::High => "High",
        TodoPriority::Medium => "Medium",
        TodoPriority::Low => "Low",
    }
}

/// The body rows of the popup, wrapped to `width` columns.
pub fn todo_detail_lines(item: &TodoItem, width: u16) -> Vec<Line<'static>> {
    let theme = Theme::current();
    let label = Style::default().fg(theme.gray);
    let heading = Style::default()
        .fg(theme.text_secondary)
        .add_modifier(Modifier::BOLD);
    let body = Style::default().fg(theme.text_primary);
    let dim = Style::default().fg(theme.gray_bright);
    let wrapped = |text: &str, style: Style| -> Vec<Line<'static>> {
        wrap_pause_message_lines(&strip_control_chars(text, true), width)
            .into_iter()
            .map(|row| Line::from(Span::styled(row, style)))
            .collect()
    };

    let (status_text, status_color) = status_label(item.status, &theme);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Status: ", label),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Priority: ", label),
            Span::styled(priority_label(item.priority), body),
        ]),
        Line::default(),
        Line::from(Span::styled("Task", heading)),
    ];
    lines.extend(wrapped(&item.content, body));

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Completion requirement", heading)));
    match item.verification.as_deref() {
        Some(condition) => {
            lines.extend(wrapped(condition, body));
            let (mark, text, color) = if item.verification_passed {
                (
                    crate::glyphs::check_mark(),
                    "An independent verifier confirmed this condition.",
                    theme.accent_success,
                )
            } else {
                (
                    "○",
                    "Not verified yet. Marking this todo completed runs an independent verifier against this condition.",
                    theme.warning,
                )
            };
            let mut first = true;
            for row in wrap_pause_message_lines(text, width.saturating_sub(2)) {
                let lead = if first { mark } else { " " };
                first = false;
                lines.push(Line::from(vec![
                    Span::styled(format!("{lead} "), Style::default().fg(color)),
                    Span::styled(row, dim),
                ]));
            }
        }
        None => lines.extend(wrapped(
            "None. This todo completes as soon as it is marked completed.",
            dim,
        )),
    }

    if let Some(meta) = item
        .meta
        .as_ref()
        .filter(|m| !m.as_object().is_some_and(|o| o.is_empty()))
    {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Metadata", heading)));
        let pretty = serde_json::to_string_pretty(meta).unwrap_or_else(|_| meta.to_string());
        for row in pretty.lines() {
            lines.extend(wrapped(row, dim));
        }
    }
    lines
}

/// Rows the popup frame adds around the body: the borders and the footer hint.
const CHROME_ROWS: u16 = 4;

/// Inner text width for a popup of outer width `w`: the borders plus one column of padding a side.
fn body_width(w: u16) -> u16 {
    w.saturating_sub(4)
}

fn popup_width(screen: Rect) -> u16 {
    ((screen.width as f32 * 0.7) as u16)
        .clamp(50, 110)
        .min(screen.width.saturating_sub(4))
}

/// The centered rect the popup takes for `item` on `screen`.
pub fn todo_detail_area(screen: Rect, item: &TodoItem) -> Rect {
    let w = popup_width(screen);
    let body = todo_detail_lines(item, body_width(w)).len() as u16;
    let h = (body + CHROME_ROWS).min(screen.height.saturating_sub(2));
    Rect::new(
        screen.x + screen.width.saturating_sub(w) / 2,
        screen.y + screen.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// Largest scroll offset that still fills the body rows of `area`.
pub fn max_scroll(area: Rect, item: &TodoItem) -> u16 {
    let total = todo_detail_lines(item, body_width(area.width)).len() as u16;
    total.saturating_sub(area.height.saturating_sub(CHROME_ROWS))
}

/// Draw the popup. `position` is 1-based among `total` visible todos. Returns the close button rect.
pub fn render_todo_detail(
    buf: &mut Buffer,
    area: Rect,
    item: &TodoItem,
    position: usize,
    total: usize,
    scroll: u16,
    close_hovered: bool,
) -> Option<Rect> {
    let theme = Theme::current();
    if area.width < 20 || area.height < CHROME_ROWS + 1 {
        return None;
    }
    let base = Style::default().bg(theme.bg_base);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                cell.reset();
                cell.set_style(base);
            }
        }
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray).bg(theme.bg_base))
        .style(base);
    let inner = block.inner(area);
    ratatui::widgets::Widget::render(block, area, buf);

    let close_text = format!("[{}]", crate::glyphs::ballot_x());
    let close_w = unicode_width::UnicodeWidthStr::width(close_text.as_str()) as u16;
    let close_x = area.x + area.width.saturating_sub(close_w + 1);
    let title_cols = close_x.saturating_sub(area.x + 3) as usize;
    let title = truncate_to_width(&format!(" Todo {position}/{total} "), title_cols);
    buf.set_span_safe(
        area.x + 2,
        area.y,
        &Span::styled(
            title,
            Style::default()
                .fg(theme.accent_plan)
                .bg(theme.bg_base)
                .add_modifier(Modifier::BOLD),
        ),
        title_cols as u16,
    );
    let close_style = if close_hovered {
        Style::default()
            .fg(theme.text_primary)
            .bg(theme.bg_base)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.gray).bg(theme.bg_base)
    };
    buf.set_span_safe(
        close_x,
        area.y,
        &Span::styled(close_text, close_style),
        close_w,
    );

    let x = inner.x + 1;
    let w = body_width(area.width);
    let body_rows = inner.height.saturating_sub(2);
    let lines = todo_detail_lines(item, w);
    for (row, line) in lines
        .iter()
        .skip(scroll as usize)
        .take(body_rows as usize)
        .enumerate()
    {
        buf.set_line_safe(x, inner.y + row as u16, line, w);
    }

    let more = lines.len() > scroll as usize + body_rows as usize || scroll > 0;
    let mut hint = String::from("←/→ previous/next · Esc close");
    if more {
        hint.push_str(" · ↑/↓ scroll");
    }
    buf.set_span_safe(
        x,
        inner.y + inner.height - 1,
        &Span::styled(hint, Style::default().fg(theme.gray_dim).bg(theme.bg_base)),
        w,
    );
    Some(Rect::new(close_x, area.y, close_w, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, verification: Option<&str>, passed: bool) -> TodoItem {
        TodoItem {
            content: content.into(),
            priority: TodoPriority::High,
            status: TodoStatus::InProgress,
            meta: None,
            verification: verification.map(str::to_owned),
            verification_passed: passed,
        }
    }

    fn render(item: &TodoItem, position: usize, total: usize) -> Vec<String> {
        let screen = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(screen);
        let area = todo_detail_area(screen, item);
        render_todo_detail(&mut buf, area, item, position, total, 0, false);
        (0..screen.height)
            .map(|y| {
                (0..screen.width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn popup_shows_the_requirement_and_its_verdict() {
        let lines = render(
            &item("Ship the parser", Some("cargo test passes"), false),
            2,
            5,
        );
        let text = lines.join("\n");
        for want in [
            "Todo 2/5",
            "In progress",
            "High",
            "Ship the parser",
            "Completion requirement",
            "cargo test passes",
            "Not verified yet",
            "←/→ previous/next",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }

        let text = render(
            &item("Ship the parser", Some("cargo test passes"), true),
            1,
            1,
        )
        .join("\n");
        assert!(
            text.contains("verifier confirmed"),
            "passed verdict:\n{text}"
        );
    }

    #[test]
    fn popup_without_a_verifier_says_there_is_none() {
        let text = render(&item("Write docs", None, false), 1, 1).join("\n");
        assert!(text.contains("None. This todo completes"), "{text}");
    }

    #[test]
    fn popup_wraps_long_task_text_instead_of_cutting_it() {
        let long = "word ".repeat(60);
        let lines = todo_detail_lines(&item(&long, None, false), 40);
        let words: usize = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.matches("word").count())
                    .sum::<usize>()
            })
            .sum();
        assert_eq!(words, 60, "every word of the task text is drawn");
    }

    #[test]
    fn cycle_wraps_in_both_directions_over_visible_todos() {
        let visible = [0, 2, 5];
        let mut state = TodoDetailState::new(2);
        state.cycle(&visible, 1);
        assert_eq!(state.index, 5);
        state.cycle(&visible, 1);
        assert_eq!(state.index, 0, "right from the last wraps to the first");
        state.cycle(&visible, -1);
        assert_eq!(state.index, 5, "left from the first wraps to the last");
    }

    #[test]
    fn cycle_from_a_hidden_todo_lands_on_the_first_visible_one() {
        let mut state = TodoDetailState::new(3);
        state.scroll = 4;
        state.cycle(&[1, 4], 1);
        assert_eq!(state, TodoDetailState::new(1));
    }
}
