//! `/timestamps`: toggle timestamp display on messages.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct TimestampsCommand;

impl SlashCommand for TimestampsCommand {
    slash_meta! {
        name: "timestamps",
        description: "Toggle message timestamps on/off",
        usage: "/timestamps",
        arg_placeholder: "on/off",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        let new = !crate::appearance::cache::load_timestamps();
        CommandResult::Action(Action::SetTimestamps(new))
    }
}
