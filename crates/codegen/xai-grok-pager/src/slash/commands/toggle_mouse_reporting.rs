//! `/toggle-mouse-reporting`: flip terminal mouse capture from anywhere.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Toggle terminal mouse reporting (mouse capture).
pub struct ToggleMouseReportingCommand;

impl SlashCommand for ToggleMouseReportingCommand {
    slash_meta! {
        name: "toggle-mouse-reporting",
        description: "Toggle terminal mouse reporting (native click-drag copy/paste)",
        usage: "/toggle-mouse-reporting",
    }

    /// Only offered when the opt-in feature is enabled in config.
    fn visible(&self, _ctx: &AppCtx) -> bool {
        crate::app::mouse_reporting_toggle_enabled()
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if crate::app::mouse_reporting_toggle_enabled() {
            CommandResult::Action(Action::ToggleMouseCapture)
        } else {
            CommandResult::Message(
                "Mouse reporting toggle is off. Set `[ui] mouse_reporting_toggle = true` \
                 in ~/.grok/config.toml to enable it."
                    .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use std::sync::atomic::Ordering;

    fn set_enabled(on: bool) {
        crate::app::MOUSE_REPORTING_TOGGLE_ENABLED.store(on, Ordering::Release);
    }

    fn exec_ctx<'a>(models: &'a ModelState, bundle: &'a BundleState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: bundle,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    #[serial_test::serial(MOUSE_REPORTING_TOGGLE_ENABLED)]
    #[test]
    fn run_returns_toggle_action_when_enabled() {
        set_enabled(true);
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = exec_ctx(&models, &bundle);
        assert!(matches!(
            ToggleMouseReportingCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::ToggleMouseCapture)
        ));
        set_enabled(false);
    }

    #[serial_test::serial(MOUSE_REPORTING_TOGGLE_ENABLED)]
    #[test]
    fn run_returns_hint_message_when_disabled() {
        set_enabled(false);
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = exec_ctx(&models, &bundle);
        assert!(matches!(
            ToggleMouseReportingCommand.run(&mut ctx, ""),
            CommandResult::Message(_)
        ));
    }

    #[serial_test::serial(MOUSE_REPORTING_TOGGLE_ENABLED)]
    #[test]
    fn visible_tracks_config_flag() {
        let models = ModelState::default();
        let ctx = AppCtx {
            models: &models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        };
        set_enabled(true);
        assert!(ToggleMouseReportingCommand.visible(&ctx));
        set_enabled(false);
        assert!(!ToggleMouseReportingCommand.visible(&ctx));
    }
}
