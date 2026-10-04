//! `/voice` toggles dictation: it starts recording now, and Esc or Enter stops (Enter also sends). Nothing is written to `config.toml`.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand, slash_meta};

pub struct VoiceCommand;

impl SlashCommand for VoiceCommand {
    slash_meta! {
        name: "voice",
        usage: "/voice",
        // Dictation targets a prompt box: the agent prompt in a live session.
        session_scoped: true,
        offered_when_session_less: true,
    }

    fn description(&self) -> &str {
        // Chord is Ctrl+Space or F8 Without key releases hold-to-talk is
        // impossible.
        if crate::app::kitty_releases_reported() {
            "Dictation (Ctrl+Space/F8; Esc/Enter to stop)"
        } else {
            "Toggle dictation (Ctrl+Space/F8; Esc/Enter to stop)"
        }
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        // Toggle, mirroring the voice key: starts dictation, or stops it if already recording (Esc/Enter also stop)
        CommandResult::Action(Action::VoiceToggle)
    }
}
