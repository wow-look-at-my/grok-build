
// model_agent.rs holds MODEL_AGENT_FAMILIES.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// The families a `@mention` spawns. The shell resolves each to the newest model whose id names it.
pub const MODEL_AGENT_FAMILIES: &[&str] = &["sonnet", "haiku", "opus"];

/// The effort words the shell accepts (`ReasoningEffort`'s names).
const EFFORT_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];

pub struct ModelAgentCommand {
    family: &'static str,
    description: String,
    usage: String,
}

impl ModelAgentCommand {
    pub fn new(family: &'static str) -> Self {
        Self {
            family,
            description: format!("Spawn a {family} subagent (also @{family} [level] <task>)"),
            usage: format!("/{family} [level] <task>"),
        }
    }
}

impl SlashCommand for ModelAgentCommand {
    fn name(&self) -> &str {
        self.family
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn usage(&self) -> &str {
        &self.usage
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[level] <task>")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let (effort, prompt) = split_effort(args);
        if prompt.is_empty() {
            return CommandResult::Error(format!("Usage: {}", self.usage));
        }
        CommandResult::Action(Action::SpawnModelAgent {
            family: self.family.to_owned(),
            effort: effort.map(str::to_owned),
            prompt: prompt.to_owned(),
        })
    }
}

/// Split a leading effort word off the task. A first word that is not a level is part of
/// the task.
fn split_effort(args: &str) -> (Option<&'static str>, &str) {
    let args = args.trim();
    let (first, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    let level = EFFORT_LEVELS
        .iter()
        .find(|level| level.eq_ignore_ascii_case(first))
        .copied();
    match level {
        Some(level) => (Some(level), rest.trim()),
        None => (None, args),
    }
}

/// Rewrite a prompt that starts to its slash command. The mention must be the
/// first token and must stand alone: `@sonnet-notes.md` stays a file
/// reference. The rest of the text is kept as typed.
pub fn rewrite_model_mention(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let after_at = trimmed.strip_prefix('@')?;
    let end = after_at.find(char::is_whitespace).unwrap_or(after_at.len());
    let token = &after_at[..end];
    let family = MODEL_AGENT_FAMILIES
        .iter()
        .find(|family| family.eq_ignore_ascii_case(token))?;
    Some(format!("/{family}{}", &after_at[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_mention_becomes_its_command() {
        assert_eq!(
            rewrite_model_mention("@sonnet high fix the test").as_deref(),
            Some("/sonnet high fix the test")
        );
        assert_eq!(
            rewrite_model_mention("  @Opus review this").as_deref(),
            Some("/opus review this")
        );
        assert_eq!(rewrite_model_mention("@haiku").as_deref(), Some("/haiku"));
    }

    #[test]
    fn other_mentions_stay_prompts() {
        for text in [
            "@sonnet-notes.md summarize",
            "@src/main.rs explain",
            "ask @sonnet later",
            "sonnet do it",
            "@sonnets",
        ] {
            assert_eq!(rewrite_model_mention(text), None, "{text}");
        }
    }

    #[test]
    fn a_leading_level_is_the_effort() {
        assert_eq!(split_effort("high fix it"), (Some("high"), "fix it"));
        assert_eq!(split_effort("XHIGH  fix it "), (Some("xhigh"), "fix it"));
        assert_eq!(split_effort("max"), (Some("max"), ""));
    }

    #[test]
    fn a_first_word_that_is_not_a_level_is_the_task() {
        assert_eq!(split_effort("fix the high bar"), (None, "fix the high bar"));
        assert_eq!(split_effort("  "), (None, ""));
    }

    #[test]
    fn run_builds_the_spawn_action() {
        let command = ModelAgentCommand::new("sonnet");
        let (effort, prompt) = split_effort("low write the docs");
        assert_eq!((effort, prompt), (Some("low"), "write the docs"));
        assert_eq!(command.name(), "sonnet");
        assert_eq!(command.usage(), "/sonnet [level] <task>");
    }
}
