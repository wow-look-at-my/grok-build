//! `/todo` -- capture a request as todo items without interrupting the agent.
//!
//! Returns `CommandResult::Action(Action::SendTodo { .. })` so the dispatch layer
//! fires it as an ACP ext method (`x.ai/todo`) that bypasses the prompt queue.
//! The shell forks a short-lived side agent that may read, and may append to
//! the todo list, and may do nothing else.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct TodoCommand;

impl SlashCommand for TodoCommand {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Add to the todo list without interrupting"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/todo <what to add>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<what to add>")
    }

    /// The capture agent appends through the session's own `todo_write`; an
    /// agent without it has no list to append to.
    fn required_tools(&self) -> &[&str] {
        &["todo_write"]
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::Action(Action::SendTodo {
            request: args.trim().to_string(),
            urgent: false,
        })
    }

    fn run_with_token(&self, ctx: &mut CommandExecCtx, token: &str, args: &str) -> CommandResult {
        if !is_urgent_token(token) {
            return self.run(ctx, args);
        }
        CommandResult::Action(Action::SendTodo {
            request: args.trim().to_string(),
            urgent: true,
        })
    }
}

/// Whether the typed name asks for an urgent capture.
///
/// Exactly `TODO`, all caps, with nothing else to it. `/Todo` and `/ToDo` are
/// shift-key noise, not a request to jump the queue, so only the deliberate
/// all-caps spelling counts.
fn is_urgent_token(token: &str) -> bool {
    token.trim_start_matches('/') == "TODO"
}

#[cfg(test)]
mod tests {
    use super::is_urgent_token;

    #[test]
    fn only_all_caps_todo_is_urgent() {
        assert!(is_urgent_token("TODO"));
        assert!(is_urgent_token("/TODO"));
        for token in ["todo", "Todo", "ToDo", "tODO", "toDO"] {
            assert!(!is_urgent_token(token), "{token} must not be urgent");
        }
    }

    #[test]
    fn a_longer_name_is_not_the_urgent_todo() {
        for token in ["TODOS", "XTODO", "TODO2"] {
            assert!(!is_urgent_token(token), "{token} must not be urgent");
        }
    }
}
