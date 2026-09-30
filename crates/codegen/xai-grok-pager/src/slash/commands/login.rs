use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct LoginCommand;

impl SlashCommand for LoginCommand {
    slash_meta! {
        name: "login",
        description: "Log in or re-authenticate with your account",
        usage: "/login",
    }

<<<<<<< HEAD
=======
    fn description(&self) -> &str {
        "Log in or re-authenticate with your account"
    }

    fn usage(&self) -> &str {
        "/login"
    }

>>>>>>> origin/master
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::Login)
    }
}
