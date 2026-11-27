//! Input for the route dialog (`crate::views::hops_dialog`).
use super::AgentView;
use crate::app::actions::Action;
use crate::app::app_view::InputOutcome;
use crate::key;
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

impl AgentView {
    /// The ids a gateway's own answer must not repeat: the hops and the provider block.
    pub(crate) fn route_chain_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self
            .route_view
            .iter()
            .flat_map(|view| view.links.iter().map(|link| link.id.as_str()))
            .collect();
        if let Some(provider) = self.session.models.current_model_provider()
            && !ids.contains(&provider)
        {
            ids.push(provider);
        }
        ids
    }

    /// The provider that serves the session's model calls, as it is drawn.
    pub(crate) fn serving_provider(&self) -> Option<crate::views::provider_brand::ProviderBrand> {
        let model_id = self.session.models.current_model_id_str()?;
        crate::views::provider_brand::serving_provider(
            self.session.tracker.upstream_provider(),
            model_id,
            &self.route_chain_ids(),
        )
    }

    /// The toggle of the selected row, when that row is an optional hop.
    fn hops_toggle_action(&self, index: usize) -> Option<Action> {
        let dialog = self.hops_dialog.as_ref()?;
        let view = dialog.view.as_ref()?;
        let link = view.links.get(index).filter(|link| link.optional)?;
        Some(Action::ToggleHop {
            provider_id: view.provider_id.clone(),
            hop_id: link.id.clone(),
            enabled: !link.enabled,
        })
    }

    /// The dialog owns all input while it is open. Returns `None` when it is closed.
    pub(super) fn handle_hops_dialog_input(&mut self, ev: &Event) -> Option<InputOutcome> {
        let dialog = self.hops_dialog.as_mut()?;
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key!('q', CONTROL).matches(key) {
                    return Some(InputOutcome::Unchanged);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.hops_dialog = None,
                    KeyCode::Up | KeyCode::Char('k') => dialog.move_selection(-1),
                    KeyCode::Down | KeyCode::Char('j') => dialog.move_selection(1),
                    KeyCode::Char('r') => return Some(InputOutcome::Action(Action::ProbeRoute)),
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        let selected = dialog.selected;
                        if let Some(action) = self.hops_toggle_action(selected) {
                            return Some(InputOutcome::Action(action));
                        }
                    }
                    _ => {}
                }
                Some(InputOutcome::Changed)
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if self.hit_hops_close.contains(mouse.column, mouse.row) {
                            self.hops_dialog = None;
                            return Some(InputOutcome::Changed);
                        }
                        let hit = self
                            .hops_dialog_rows
                            .iter()
                            .find(|(_, rect)| {
                                rect.contains(ratatui::layout::Position::new(
                                    mouse.column,
                                    mouse.row,
                                ))
                            })
                            .map(|(index, _)| *index);
                        if let Some(index) = hit {
                            dialog.selected = index;
                            if let Some(action) = self.hops_toggle_action(index) {
                                return Some(InputOutcome::Action(action));
                            }
                        }
                    }
                    MouseEventKind::Moved => {
                        self.hit_hops_close.update_hover(mouse.column, mouse.row);
                    }
                    MouseEventKind::ScrollUp => dialog.move_selection(-1),
                    MouseEventKind::ScrollDown => dialog.move_selection(1),
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
mod hops_overlay_tests {
    use super::super::test_fixtures::make_agent;
    use crate::actions::ActionRegistry;
    use crate::app::actions::Action;
    use crate::app::app_view::InputOutcome;
    use crate::views::hops_dialog::HopsDialogState;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use xai_grok_shell::agent::model_route::{RouteLink, RouteView};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn link(id: &str, kind: &str, optional: bool, enabled: bool) -> RouteLink {
        RouteLink {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: kind.to_owned(),
            optional,
            enabled,
            url: format!("http://{id}.lan"),
        }
    }

    fn dialog() -> HopsDialogState {
        HopsDialogState {
            view: Some(RouteView {
                provider_id: "openrouter".to_owned(),
                links: vec![
                    link("nas", "relay", true, true),
                    link("bifrost", "gateway", true, false),
                    link("openrouter", "provider", false, true),
                ],
                entry_url: "http://nas.lan/openrouter".to_owned(),
                error: None,
            }),
            model_label: "model".to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn space_on_an_optional_hop_asks_for_the_opposite_state() {
        let mut agent = make_agent();
        let registry = ActionRegistry::defaults();
        agent.hops_dialog = Some(dialog());

        let outcome = agent.handle_input(&key(KeyCode::Char(' ')), &registry);
        assert!(
            matches!(
                &outcome,
                InputOutcome::Action(Action::ToggleHop { provider_id, hop_id, enabled: false })
                    if provider_id == "openrouter" && hop_id == "nas"
            ),
            "{outcome:?}"
        );

        agent.handle_input(&key(KeyCode::Down), &registry);
        let outcome = agent.handle_input(&key(KeyCode::Enter), &registry);
        assert!(
            matches!(
                &outcome,
                InputOutcome::Action(Action::ToggleHop { hop_id, enabled: true, .. })
                    if hop_id == "bifrost"
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn the_provider_row_has_no_toggle() {
        let mut agent = make_agent();
        let registry = ActionRegistry::defaults();
        agent.hops_dialog = Some(dialog());
        agent.handle_input(&key(KeyCode::Down), &registry);
        agent.handle_input(&key(KeyCode::Down), &registry);
        agent.handle_input(&key(KeyCode::Down), &registry);
        assert_eq!(agent.hops_dialog.as_ref().unwrap().selected, 2);
        let outcome = agent.handle_input(&key(KeyCode::Char(' ')), &registry);
        assert!(matches!(outcome, InputOutcome::Changed), "{outcome:?}");
    }

    #[test]
    fn r_measures_again_and_esc_closes() {
        let mut agent = make_agent();
        let registry = ActionRegistry::defaults();
        agent.hops_dialog = Some(dialog());
        let outcome = agent.handle_input(&key(KeyCode::Char('r')), &registry);
        assert!(
            matches!(outcome, InputOutcome::Action(Action::ProbeRoute)),
            "{outcome:?}"
        );
        agent.handle_input(&key(KeyCode::Esc), &registry);
        assert!(agent.hops_dialog.is_none());
    }

    #[test]
    fn the_dialog_takes_keys_the_prompt_would_have_typed() {
        let mut agent = make_agent();
        let registry = ActionRegistry::defaults();
        agent.hops_dialog = Some(dialog());
        agent.handle_input(&key(KeyCode::Char('x')), &registry);
        assert!(agent.hops_dialog.is_some());
        assert!(
            agent.prompt.text().is_empty(),
            "the key did not reach the prompt"
        );
    }
}
