//! `/hops` (aliases `/route`, `/proxies`) opens the route dialog.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

/// Show and toggle the proxy hops to the model's provider.
pub struct HopsCommand;

impl SlashCommand for HopsCommand {
    slash_meta! {
        name: "hops",
        aliases: ["route", "proxies"],
        description: "Show, measure and toggle the proxy hops to the model's provider",
        usage: "/hops",
        session_scoped: true,
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }
        CommandResult::Action(Action::OpenHopsDialog)
    }
}
