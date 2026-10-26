//! Input for the todo detail popup (`crate::views::todo_detail`).
use super::AgentView;
use crate::app::app_view::InputOutcome;
use crate::key;
use crate::views::todo_detail::TodoDetailState;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

impl AgentView {
    pub(crate) fn open_todo_detail(&mut self, index: usize) {
        if index < self.todo.todos().len() {
            self.todo_detail = Some(TodoDetailState::new(index));
        }
    }

    /// The popup owns all input while it is open. Returns `None` when it is closed.
    pub(super) fn handle_todo_detail_input(&mut self, ev: &Event) -> Option<InputOutcome> {
        let detail = self.todo_detail.as_mut()?;
        if detail.index >= self.todo.todos().len() {
            self.todo_detail = None;
            return None;
        }
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key!('q', CONTROL).matches(key) {
                    return Some(InputOutcome::Unchanged);
                }
                let visible = self.todo.visible_indices();
                match key.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.todo_detail = None,
                    KeyCode::Left | KeyCode::Char('h') => detail.cycle(&visible, -1),
                    KeyCode::Right | KeyCode::Char('l') => detail.cycle(&visible, 1),
                    KeyCode::Up | KeyCode::Char('k') => {
                        detail.scroll = detail.scroll.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        detail.scroll = detail.scroll.saturating_add(1)
                    }
                    _ => {}
                }
                Some(InputOutcome::Changed)
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left)
                        if self.hit_todo_detail_close.contains(mouse.column, mouse.row) =>
                    {
                        self.todo_detail = None;
                    }
                    MouseEventKind::Moved => {
                        self.hit_todo_detail_close
                            .update_hover(mouse.column, mouse.row);
                    }
                    MouseEventKind::ScrollUp => detail.scroll = detail.scroll.saturating_sub(1),
                    MouseEventKind::ScrollDown => detail.scroll = detail.scroll.saturating_add(1),
                    _ => {}
                }
                Some(InputOutcome::Changed)
            }
            Event::Paste(_) => Some(InputOutcome::Changed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod todo_detail_overlay_tests {
    use super::super::test_fixtures::make_agent;
    use crate::actions::ActionRegistry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use xai_grok_shell::tools::{TodoItem, TodoPriority, TodoStatus};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn item(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            content: content.into(),
            priority: TodoPriority::Medium,
            status,
            meta: None,
            verification: None,
            verification_passed: false,
        }
    }

    #[test]
    fn arrows_cycle_the_open_popup_and_esc_closes_it() {
        let mut agent = make_agent();
        let reg = ActionRegistry::default();
        agent.todo.update_todos(vec![
            item("a", TodoStatus::Pending),
            item("b", TodoStatus::InProgress),
            item("c", TodoStatus::Completed),
        ]);
        agent.open_todo_detail(0);

        agent.handle_input(&key(KeyCode::Right), &reg);
        assert_eq!(agent.todo_detail.map(|d| d.index), Some(1));
        agent.handle_input(&key(KeyCode::Right), &reg);
        agent.handle_input(&key(KeyCode::Right), &reg);
        assert_eq!(
            agent.todo_detail.map(|d| d.index),
            Some(0),
            "right wraps past the last todo"
        );
        agent.handle_input(&key(KeyCode::Left), &reg);
        assert_eq!(agent.todo_detail.map(|d| d.index), Some(2));

        agent.handle_input(&key(KeyCode::Esc), &reg);
        assert!(agent.todo_detail.is_none());
    }

    #[test]
    fn cycling_skips_todos_the_pane_hides() {
        let mut agent = make_agent();
        let reg = ActionRegistry::default();
        agent.todo.update_todos(vec![
            item("a", TodoStatus::Pending),
            item("b", TodoStatus::Completed),
            item("c", TodoStatus::Pending),
        ]);
        agent.todo.toggle_show_done();
        agent.open_todo_detail(0);
        agent.handle_input(&key(KeyCode::Right), &reg);
        assert_eq!(agent.todo_detail.map(|d| d.index), Some(2));
    }

    #[test]
    fn a_shrunk_list_closes_the_popup() {
        let mut agent = make_agent();
        let reg = ActionRegistry::default();
        agent
            .todo
            .update_todos(vec![item("a", TodoStatus::Pending)]);
        agent.open_todo_detail(0);
        agent.todo.update_todos(Vec::new());
        agent.handle_input(&key(KeyCode::Right), &reg);
        assert!(agent.todo_detail.is_none());
    }
}
