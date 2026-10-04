//! `/dashboard`: open the Agent Dashboard view.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct DashboardCommand;

impl SlashCommand for DashboardCommand {
    slash_meta! {
        name: "dashboard",
        // `/sessions` stays as an alias after the sessions picker modal.
        aliases: ["agents-dashboard", "sessions"],
        description: "Open the Agent Dashboard",
        usage: "/dashboard",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenDashboard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::slash::command::{CommandExecCtx, CommandResult};

    #[test]
    fn run_returns_open_dashboard_action() {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        };
        let cmd = DashboardCommand;
        assert!(matches!(
            cmd.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenDashboard)
        ));
    }

    #[test]
    fn does_not_take_args() {
        let cmd = DashboardCommand;
        assert!(!cmd.takes_args());
    }

    /// `/sessions` (the removed picker modal) and `/agents-dashboard` both spell this command.
    #[test]
    fn aliases_include_sessions() {
        let cmd = DashboardCommand;
        assert_eq!(cmd.aliases(), &["agents-dashboard", "sessions"]);
    }
}
