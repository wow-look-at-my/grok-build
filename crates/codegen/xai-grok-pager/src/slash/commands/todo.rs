//! `/todo` -- capture a request as todo items without interrupting the agent.
//!
//! Returns `CommandResult::Action(Action::SendTodo(...))` so the dispatch layer
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
        CommandResult::Action(Action::SendTodo(args.trim().to_string()))
    }
}
