//! `/vim-mode`: toggle vim-style scrollback keybindings.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct VimModeCommand;

impl SlashCommand for VimModeCommand {
    slash_meta! {
        name: "vim-mode",
        description: "Toggle vim-style scrollback keybindings (j/k, h/l, g/G, y/Y, …)",
        usage: "/vim-mode",
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::ToggleVimMode)
    }
}
