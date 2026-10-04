//! `/compact-mode`: toggle compact display mode.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Toggle compact display mode via `/compact-mode`.
pub struct CompactModeCommand;

impl SlashCommand for CompactModeCommand {
    slash_meta! {
        name: "compact-mode",
        description: "Toggle compact UI (less padding, more content)",
        usage: "/compact-mode",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::ToggleCompactMode)
    }
}
