//! `/timeline`: toggle the timeline sidebar (per-turn tick rail).
//!
//! Computes the new value itself and dispatches the typed `Action::SetTimeline(bool)`, mirroring `/timestamps`.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct TimelineCommand;

impl SlashCommand for TimelineCommand {
    slash_meta! {
        name: "timeline",
        description: "Toggle the timeline sidebar",
        usage: "/timeline",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        let new = !crate::appearance::cache::load_show_timeline();
        CommandResult::Action(Action::SetTimeline(new))
    }
}
