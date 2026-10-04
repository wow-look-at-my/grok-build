//! `/tutorial` -- open the onboarding tutorial overlay.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Open the onboarding tutorial.
pub struct TutorialCommand;

impl SlashCommand for TutorialCommand {
    slash_meta! {
        name: "tutorial",
        aliases: ["tour", "onboarding"],
        description: "Quick tips to get the most out of Grok Build",
        usage: "/tutorial",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenTutorial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    static DEFAULT_BUNDLE_STATE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    #[test]
    fn dispatches_open_tutorial() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            TutorialCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenTutorial)
        ));
    }
}
