//! `/memory`: open the memory browser modal.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct MemoryCommand;

impl SlashCommand for MemoryCommand {
    slash_meta! {
        name: "memory",
        aliases: ["mem"],
        description: "Browse, view, and manage your memories",
        usage: "/memory",
        session_scoped: true,
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim().is_empty() {
            CommandResult::Action(Action::OpenMemoryModal)
        } else {
            CommandResult::Error(
                "/memory takes no arguments. Open it, then press t to turn memory on or off and s for status."
                    .to_string(),
            )
        }
    }
}
