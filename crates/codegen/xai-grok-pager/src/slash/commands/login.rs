//! `/login` -- log in or re-authenticate with your account.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct LoginCommand;

impl SlashCommand for LoginCommand {
    fn name(&self) -> &str {
        "login"
    }

    fn description(&self) -> &str {
        "Log in to Grok or add a Codex account"
    }

    fn usage(&self) -> &str {
        "/login [codex]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[codex]")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        result_for_args(args)
    }
}

fn result_for_args(args: &str) -> CommandResult {
    match args.trim().to_ascii_lowercase().as_str() {
        "" => CommandResult::Action(Action::Login),
        "codex" | "chatgpt" | "openai" => CommandResult::Action(Action::LoginCodex),
        _ => CommandResult::Error("Usage: /login [codex]".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_args_keep_primary_login_flow() {
        assert!(matches!(
            result_for_args(""),
            CommandResult::Action(Action::Login)
        ));
    }

    #[test]
    fn codex_aliases_select_additive_provider_flow() {
        for value in ["codex", "ChatGPT", " openai "] {
            assert!(matches!(
                result_for_args(value),
                CommandResult::Action(Action::LoginCodex)
            ));
        }
    }

    #[test]
    fn unknown_provider_shows_usage() {
        assert!(matches!(
            result_for_args("unknown"),
            CommandResult::Error(message) if message == "Usage: /login [codex]"
        ));
    }
}
