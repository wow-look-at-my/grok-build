//! `/gboom` -- hidden easter egg. Launches a tiny raycaster shooter rendered through the kitty graphics protocol.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Hidden GBOOM easter egg.
pub struct GboomCommand;

impl SlashCommand for GboomCommand {
    slash_meta! {
        name: "gboom",
        // Never shown: the command is hidden from the dropdown.
        description: "Hidden easter egg",
        usage: "/gboom",
        // Needs an agent view to render in.
        session_scoped: true,
    }

    /// Easter egg: typeable, never listed.
    fn visible(&self, _ctx: &AppCtx) -> bool {
        false
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if !args.trim().is_empty() {
            // With arguments, behave as if the command didn't exist.
            return CommandResult::PassThrough(format!("/gboom {args}"));
        }
        CommandResult::Action(Action::OpenGboom)
    }
}
