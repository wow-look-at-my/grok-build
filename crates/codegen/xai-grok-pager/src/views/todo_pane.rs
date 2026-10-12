//! Renders `TodoItem`s from `xai-grok-tools` in a `ListPane`.
//!
//! Wraps the canonical `TodoItem` type with a `ListItem` implementation that provides status-icon prefixes and styled content.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use xai_grok_shell::tools::{TodoItem, TodoStatus};

use super::list_pane::ListItem;

#[derive(Debug, Clone, Copy)]
pub struct TodoStatusStyle {
    pub icon_fg: Color,
    pub text_style: Style,
}

#[derive(Debug, Clone, Copy)]
pub struct TodoPaneStyle {
    pub pending: TodoStatusStyle,
    pub in_progress: TodoStatusStyle,
    pub completed: TodoStatusStyle,
    pub cancelled: TodoStatusStyle,
}

impl Default for TodoPaneStyle {
    fn default() -> Self {
        // The theme quantizes colors for terminal compatibility
        let theme = crate::theme::Theme::current();

        Self {
            pending: TodoStatusStyle {
                icon_fg: theme.text_primary,
                text_style: Style::default().fg(theme.text_primary),
            },
            in_progress: TodoStatusStyle {
                icon_fg: theme.warning,
                text_style: Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD),
            },
            completed: TodoStatusStyle {
                icon_fg: theme.accent_success,
                text_style: Style::default().fg(theme.gray_bright),
            },
            cancelled: TodoStatusStyle {
                icon_fg: theme.accent_error,
                text_style: Style::default()
                    .fg(theme.gray_bright)
                    .add_modifier(Modifier::CROSSED_OUT),
            },
        }
    }
}

/// A `TodoItem` wrapped for display in a `ListPane`.
///
/// Caches the styled `Line` for `content()` and generates a status-icon `prefix()` per frame.
#[derive(Debug, Clone)]
pub struct TodoListEntry {
    /// Unique ID (index in the todo list, or a stable ID from the model).
    pub id: u64,
    pub item: TodoItem,
    styled: Line<'static>,
    status_style: TodoStatusStyle,
}

impl TodoListEntry {
    pub fn new(id: u64, item: TodoItem, style: &TodoPaneStyle) -> Self {
        let status_style = match item.status {
            TodoStatus::Pending => style.pending,
            TodoStatus::InProgress => style.in_progress,
            TodoStatus::Completed => style.completed,
            TodoStatus::Cancelled => style.cancelled,
        };
        let styled = Line::from(Span::styled(item.content.clone(), status_style.text_style));
        Self {
            id,
            item,
            styled,
            status_style,
        }
    }

    fn icon(&self) -> &'static str {
        match self.item.status {
            TodoStatus::Pending => "□",
            TodoStatus::InProgress => "▶",
            TodoStatus::Completed => crate::glyphs::check_mark(),
            TodoStatus::Cancelled => crate::glyphs::ballot_x(),
        }
    }
}

impl ListItem for TodoListEntry {
    fn content(&self) -> &Line<'_> {
        &self.styled
    }

    fn prefix(&self) -> Option<Line<'_>> {
        let icon = self.icon();
        Some(Line::from(vec![
            Span::styled(icon, Style::default().fg(self.status_style.icon_fg)),
            Span::raw(" "),
        ]))
    }

    fn stable_id(&self) -> u64 {
        self.id
    }

    fn search_text(&self) -> &str {
        &self.item.content
    }
}

use crossterm::event::{KeyCode, KeyEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;

use crate::appearance::LayoutConfig;
use crate::theme::ThemeKind;

use super::list_pane::{ListPane, ListPaneConfig, ListPaneState, ListPaneStyle, WrapMode};
use super::overlay::OverlayState;

/// Feeds the placeholder message shown when the pane is empty.
/// Counts ALL items regardless of the `show_done` filter.
#[derive(Debug, Clone, Copy, Default)]
pub struct TodoCounts {
    pub in_progress: usize,
    pub pending: usize,
    pub completed: usize,
    pub cancelled: usize,
}

impl TodoCounts {
    pub fn total(&self) -> usize {
        self.in_progress + self.pending + self.completed + self.cancelled
    }

    /// Tasks that count toward completion progress: every status except
    /// cancelled.
    pub fn total_excluding_cancelled(&self) -> usize {
        self.in_progress + self.pending + self.completed
    }
}

fn empty_placeholder_message(todos_empty: bool, counts: TodoCounts) -> String {
    if todos_empty {
        return "No todo items.".into();
    }
    match (counts.completed, counts.cancelled) {
        (_, 0) => "All done.".into(),
        (0, c) => format!("{c} cancelled."),
        (d, c) => format!("{d} done. {c} cancelled."),
    }
}

/// Whether the header has a `done/total` count to draw: there are items, and at least one of them
/// is not cancelled.
fn header_shown(counts: TodoCounts) -> bool {
    counts.total() > 0 && counts.total_excluding_cancelled() > 0
}

/// The header's `done/total` count: completed items over every task that can still be completed.
/// `None` when there are no items at all, or none that can be completed (every task cancelled).
fn header_count_line(counts: TodoCounts) -> Option<Line<'static>> {
    if !header_shown(counts) {
        return None;
    }
    let total = counts.total_excluding_cancelled();
    if total == 0 {
        return None;
    }
    let theme = crate::theme::Theme::current();
    let count_style = Style::default().fg(theme.text_secondary);
    Some(Line::from(vec![
        Span::styled(counts.completed.to_string(), count_style),
        Span::styled("/", Style::default().fg(theme.gray_dim)),
        Span::styled(total.to_string(), count_style),
        Span::styled(
            format!(" {}", crate::glyphs::check_mark()),
            Style::default().fg(theme.text_secondary),
        ),
    ]))
}

/// Lines the header (the `done/total` count) occupies at the top of the pane.
const HEADER_HEIGHT: u16 = 1;

/// Absolute maximum height (in lines) the todo pane will request.
const MAX_TODO_HEIGHT: u16 = 10;

/// Maximum fraction of total view height the todo pane may occupy.
const MAX_TODO_FRACTION: f32 = 0.15;

/// Owns the raw `TodoItem` data, the filtered `TodoListEntry` cache, `ListPaneState`, and style config.
/// `AgentView` holds a single `TodoPane` and delegates input and render to it.
pub struct TodoPane {
    /// Raw todo items from ACP Plan updates.
    todos: Vec<TodoItem>,
    /// Filtered and styled entries for `ListPane` rendering.
    /// Rebuilt from `todos` at the start of each `render()` call.
    entries: Vec<TodoListEntry>,
    /// List pane state (scroll, selection, search, layout cache).
    pub list_state: ListPaneState,
    /// Per-status visual style for icons and text.
    style: TodoPaneStyle,
    /// Visual style for the `ListPane` widget (selection bg, etc.).
    list_style: ListPaneStyle,
    /// Whether to show completed and cancelled items.
    show_done: bool,
    /// Shared visibility and focus state.
    pub overlay: OverlayState,
    /// Last theme kind seen; used to detect theme switches and restyle.
    last_theme: ThemeKind,
}

impl Default for TodoPane {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoPane {
    pub fn new() -> Self {
        let config = ListPaneConfig {
            follow_enabled: false,
            wrap_toggle_enabled: false,
            search_enabled: true,
            copy_enabled: true,
            show_selection_when_unfocused: false,
            visual_select_enabled: false,
            filter_enabled: true,
            goto_line_enabled: false,
        };
        let mut list_state = ListPaneState::new_with_config(WrapMode::NoWrap, false, config);
        list_state.set_clipboard_provider(Box::new(crate::clipboard::SystemClipboard));
        Self {
            todos: Vec::new(),
            entries: Vec::new(),
            list_state,
            style: TodoPaneStyle::default(),
            list_style: ListPaneStyle::default(),
            show_done: true,
            overlay: OverlayState::hidden(),
            last_theme: crate::theme::Theme::current_kind(),
        }
    }

    pub fn todos(&self) -> &[TodoItem] {
        &self.todos
    }

    /// Replace all todo items (called from ACP Plan handler).
    ///
    /// Does NOT auto-show the todo pane. Users toggle it with Ctrl-T.
    pub fn update_todos(&mut self, items: Vec<TodoItem>) {
        self.todos = items;
    }

    fn compute_counts(items: &[TodoItem]) -> TodoCounts {
        let mut c = TodoCounts::default();
        for item in items {
            match item.status {
                TodoStatus::InProgress => c.in_progress += 1,
                TodoStatus::Pending => c.pending += 1,
                TodoStatus::Completed => c.completed += 1,
                TodoStatus::Cancelled => c.cancelled += 1,
            }
        }
        c
    }

    /// Current status counts (across ALL items, ignoring `show_done`).
    pub fn counts(&self) -> TodoCounts {
        Self::compute_counts(&self.todos)
    }

    /// Visible when the overlay is shown.
    /// When empty, the pane shows a placeholder message instead of the list.
    pub fn is_visible(&self) -> bool {
        self.overlay.visible
    }

    /// Called after an overlay state change hides the pane.
    /// Closes the input bar if it's open mid-typing, but keeps any accepted search or filter across hide and show.
    pub fn on_state_change(&mut self) {
        if !self.overlay.visible {
            self.list_state.close_input_bar();
        }
    }

    /// Whether completed and cancelled items are shown.
    pub fn show_done(&self) -> bool {
        self.show_done
    }

    /// Toggle visibility of completed and cancelled items.
    pub fn toggle_show_done(&mut self) {
        self.show_done = !self.show_done;
    }

    /// Desired height in lines for layout computation.
    pub fn desired_height(&self, view_height: u16) -> u16 {
        if !self.overlay.visible {
            return 0;
        }
        let count = self.visible_count();
        if count == 0 {
            return 1;
        }
        let fraction_cap = (view_height as f32 * MAX_TODO_FRACTION).floor() as u16;
        let max = MAX_TODO_HEIGHT.min(fraction_cap).max(1);
        ((count as u16) + u16::from(header_shown(self.counts())))
            .min(max)
            .max(1)
    }

    /// Number of items that pass the `show_done` filter.
    fn visible_count(&self) -> usize {
        if self.show_done {
            self.todos.len()
        } else {
            self.todos
                .iter()
                .filter(|t| !matches!(t.status, TodoStatus::Completed | TodoStatus::Cancelled))
                .count()
        }
    }

    /// Rebuild `entries` from `todos`, filtered by `show_done`. Items preserve their original order
    /// from the agent (no reordering by status). IDs are based on the original index in `todos` so that
    /// `ListPaneState` can maintain selection across rebuilds.
    fn rebuild_entries(&mut self) {
        self.entries.clear();
        for (idx, item) in self.todos.iter().enumerate() {
            if !self.show_done
                && matches!(item.status, TodoStatus::Completed | TodoStatus::Cancelled)
            {
                continue;
            }
            self.entries
                .push(TodoListEntry::new(idx as u64, item.clone(), &self.style));
        }
    }

    /// Handle a key event when the todo pane is focused.
    ///
    /// Returns `true` if the event was consumed.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        // 'h' toggles show_done (only when not typing in the search or filter bar)
        if key.code == KeyCode::Char('h') && self.list_state.input_mode().is_none() {
            self.toggle_show_done();
            self.rebuild_entries();
            return true;
        }
        // Don't route to ListPaneState when empty.
        if self.entries.is_empty() {
            return false;
        }
        self.list_state.handle_key_event(key, &self.entries)
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        self.list_state.handle_paste(text, &self.entries)
    }

    /// Handle a mouse scroll event over the todo pane area. Caps scroll speed for small viewports. The
    /// app-level scroll accumulator can produce deltas of 3-5 lines, which would jump past most items
    /// in a 4-row pane.
    pub fn handle_scroll(&mut self, lines: i32, col: u16, row: u16) {
        let max = match self.list_state.viewport_height() {
            0..=5 => 1,
            6..=10 => 2,
            _ => lines.unsigned_abs() as i32,
        };
        let capped = lines.signum() * lines.abs().min(max);
        self.list_state
            .handle_scroll_event(capped, col, row, &self.entries);
    }

    /// Handle a mouse click or drag event over the todo pane area.
    ///
    /// Returns `true` if the event was consumed.
    pub fn handle_mouse(&mut self, kind: MouseEventKind, col: u16, row: u16, area: Rect) -> bool {
        self.list_state
            .handle_mouse_event(kind, col, row, area, &self.entries)
    }

    /// Compute the inner content area with horizontal padding matching the scrollback's `HorizontalLayout`.
    /// The left pad is accent + block_pad_left; the right pad is block_pad_right.
    fn content_area(area: Rect, layout_cfg: &LayoutConfig) -> Rect {
        use crate::scrollback::layout::HorizontalLayout;
        let pad_left = HorizontalLayout::ACCENT + layout_cfg.block_pad_left;
        let pad_right = layout_cfg.block_pad_right;
        Rect {
            x: area.x + pad_left,
            y: area.y,
            width: area.width.saturating_sub(pad_left + pad_right),
            height: area.height,
        }
    }

    /// Split the inner area into the header row and the list body. The row is reserved only when
    /// there is a count to draw and room for an item below it; otherwise the items keep every line.
    fn split_header(inner: Rect, show_header: bool) -> (Option<Rect>, Rect) {
        if !show_header || inner.height <= HEADER_HEIGHT {
            return (None, inner);
        }
        let header = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: HEADER_HEIGHT,
        };
        let body = Rect {
            x: inner.x,
            y: inner.y + HEADER_HEIGHT,
            width: inner.width,
            height: inner.height - HEADER_HEIGHT,
        };
        (Some(header), body)
    }

    /// Render the todo pane into the given area. Rebuilds entries from `todos` each call, which is
    /// cheap at the typical count of a bounded number of items. Draws the `done/total` header line,
    /// then renders the `ListPane` widget in the rows below it, in a padded inner area matching the
    /// scrollback's horizontal layout.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        focused: bool,
        layout_cfg: &LayoutConfig,
    ) {
        // Detect a theme switch and refresh styles before rebuilding entries
        let current_theme = crate::theme::Theme::current_kind();
        if current_theme != self.last_theme {
            self.last_theme = current_theme;
            self.style = TodoPaneStyle::default();
            self.list_style = ListPaneStyle::default();
        }

        self.rebuild_entries();
        let inner = Self::content_area(area, layout_cfg);
        if self.entries.is_empty() {
            if inner.height > 0 && inner.width > 0 {
                let msg = empty_placeholder_message(self.todos.is_empty(), self.counts());
                let theme = crate::theme::Theme::current();
                let span = ratatui::text::Span::styled(
                    msg,
                    ratatui::style::Style::default().fg(theme.gray_bright),
                );
                buf.set_span(inner.x, inner.y, &span, inner.width);
            }
            return;
        }
        let header = header_count_line(self.counts());
        let (header_row, body) = Self::split_header(inner, header.is_some());
        if let (Some(header_row), Some(line)) = (header_row, header) {
            use crate::render::SafeBuf;
            buf.set_line_safe(header_row.x, header_row.y, &line, header_row.width);
        }
        self.list_state
            .prepare_layout(&self.entries, body.width, body.height);
        ListPane::new(&self.entries)
            .focused(focused)
            .style(self.list_style)
            .render(body, buf, &mut self.list_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::tools::TodoPriority;

    fn counts(completed: usize, cancelled: usize) -> TodoCounts {
        TodoCounts {
            completed,
            cancelled,
            ..TodoCounts::default()
        }
    }

    #[test]
    fn empty_todos_message() {
        assert_eq!(
            empty_placeholder_message(true, TodoCounts::default()),
            "No todo items."
        );
    }

    #[test]
    fn all_completed_is_all_done() {
        assert_eq!(empty_placeholder_message(false, counts(3, 0)), "All done.");
    }

    #[test]
    fn mixed_done_and_cancelled_summarizes_counts() {
        assert_eq!(
            empty_placeholder_message(false, counts(5, 1)),
            "5 done. 1 cancelled."
        );
    }

    #[test]
    fn only_cancelled() {
        assert_eq!(
            empty_placeholder_message(false, counts(0, 2)),
            "2 cancelled."
        );
    }

    fn item(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.into(),
            priority: TodoPriority::default(),
            status,
            meta: None,
        }
    }

    /// Renders a visible pane into a buffer. It returns each row as a string, the way the other
    /// pane tests in this crate read a rendered buffer.
    fn render_pane(pane: &mut TodoPane, width: u16, height: u16) -> Vec<String> {
        pane.overlay.show();
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        pane.render(
            area,
            &mut buf,
            false,
            &crate::appearance::LayoutConfig::default(),
        );
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
            })
            .collect()
    }

    /// Some of tasks done: the header draws `2/5` above the items.
    #[test]
    fn header_draws_completed_over_total() {
        let mut pane = TodoPane::new();
        pane.update_todos(vec![
            item("alpha", TodoStatus::Completed),
            item("beta", TodoStatus::Completed),
            item("gamma", TodoStatus::InProgress),
            item("delta", TodoStatus::Pending),
            item("epsilon", TodoStatus::Pending),
        ]);

        let lines = render_pane(&mut pane, 60, 8);
        assert_eq!(
            lines[0].trim(),
            format!("2/5 {}", crate::glyphs::check_mark()),
            "header row: {lines:?}",
        );
        assert!(
            lines[1].contains("alpha"),
            "items render below the header: {lines:?}",
        );
    }

    /// Cancelled tasks stay out of the denominator, so the count can still reach `N/N`.
    #[test]
    fn header_excludes_cancelled_from_the_total() {
        let mut pane = TodoPane::new();
        pane.update_todos(vec![
            item("alpha", TodoStatus::Completed),
            item("beta", TodoStatus::Completed),
            item("gamma", TodoStatus::Cancelled),
            item("delta", TodoStatus::Pending),
        ]);

        let lines = render_pane(&mut pane, 60, 8);
        assert!(
            lines[0].contains("2/3"),
            "cancelled tasks are excluded from the total, got: {lines:?}",
        );
    }

    /// Every task cancelled: nothing can be completed, so there is no count to draw.
    #[test]
    fn header_absent_when_all_cancelled() {
        let mut pane = TodoPane::new();
        pane.update_todos(vec![
            item("alpha", TodoStatus::Cancelled),
            item("beta", TodoStatus::Cancelled),
        ]);

        let lines = render_pane(&mut pane, 60, 8);
        assert!(
            !lines.iter().any(|l| l.contains('/')),
            "no done/total count when every task is cancelled: {lines:?}",
        );
        assert!(
            lines[0].contains("alpha"),
            "the items take the whole pane: {lines:?}",
        );
    }

    /// No items at all: the placeholder holds the pane and no count is drawn.
    #[test]
    fn header_absent_when_no_todos() {
        let mut pane = TodoPane::new();

        let lines = render_pane(&mut pane, 60, 8);
        assert!(
            lines[0].contains("No todo items."),
            "placeholder message expected: {lines:?}",
        );
        assert!(
            !lines.iter().any(|l| l.contains('/')),
            "no done/total count without items: {lines:?}",
        );
    }

    /// A one-line pane gives that line to the items rather than to the header.
    #[test]
    fn header_yields_the_last_line_to_items() {
        let mut pane = TodoPane::new();
        pane.update_todos(vec![item("alpha", TodoStatus::Pending)]);

        let lines = render_pane(&mut pane, 60, 1);
        assert!(
            lines[0].contains("alpha"),
            "a 1-row pane draws the item, not the header: {lines:?}",
        );
    }

    /// The header line is part of what the pane asks the layout for.
    #[test]
    fn desired_height_reserves_the_header_line() {
        let mut pane = TodoPane::new();
        pane.overlay.show();
        pane.update_todos(
            (0..5)
                .map(|i| item(&format!("task {i}"), TodoStatus::Pending))
                .collect(),
        );
        assert_eq!(pane.desired_height(100), 6, "5 items plus the header");
    }
}
