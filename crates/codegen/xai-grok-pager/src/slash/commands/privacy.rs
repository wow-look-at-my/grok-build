//! `/privacy` -- open the "Coding data, retention, and training" setting.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

const CODING_DATA_SHARING_KEY: &str = "coding_data_sharing";

/// Open settings on `coding_data_sharing`. Takes no arguments.
pub struct PrivacyCommand;

impl SlashCommand for PrivacyCommand {
    fn name(&self) -> &str {
        "privacy"
    }

    fn description(&self) -> &str {
        // Reads as the row it opens: "Coding data, retention, and training".
        "Open coding data, retention, and training settings"
    }

    fn usage(&self) -> &str {
        "/privacy"
    }

    /// Trailing text is ignored, not rejected: `/privacy opt-in` from muscle
    /// memory should land on the page, not error.
    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenSettingsFocus {
            key: CODING_DATA_SHARING_KEY,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `/privacy <args>`.
    fn run_privacy(args: &str) -> CommandResult {
        use crate::acp::model_state::ModelState;
        use crate::app::bundle::BundleState;

        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        PrivacyCommand.run(&mut ctx, args)
    }

    fn opens_settings_row(result: &CommandResult) -> bool {
        matches!(
            result,
            CommandResult::Action(Action::OpenSettingsFocus {
                key: CODING_DATA_SHARING_KEY
            })
        )
    }

    #[test]
    fn privacy_opens_settings_row() {
        let result = run_privacy("");
        assert!(
            opens_settings_row(&result),
            "`/privacy` must open the settings row, got {result:?}",
        );
    }

    /// The arguments this used to accept must not linger as hidden aliases
    /// that change a privacy preference straight from the prompt.
    #[test]
    fn arguments_are_ignored_not_honored() {
        assert!(
            !PrivacyCommand.takes_args(),
            "the dropdown must not offer an argument slot"
        );
        for args in [
            "   ", "opt-in", "opt-out", "in", "out", "share", "private", "status", "info",
            "garbage",
        ] {
            let result = run_privacy(args);
            assert!(
                opens_settings_row(&result),
                "`/privacy {args}` must just open the page, got {result:?}",
            );
        }
    }
}
