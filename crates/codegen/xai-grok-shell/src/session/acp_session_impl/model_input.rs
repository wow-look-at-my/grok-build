use super::*;
use xai_grok_sampling_types::SyntheticReason;

impl SessionActor {
    /// Push a user-role item the shell wrote, and show it in the scrollback.
    pub(super) fn push_model_input(&self, item: ConversationItem) {
        self.announce_model_input(&item);
        self.chat_state_handle.push_user_message(item);
    }

    /// Show `item` in the scrollback and store the row in `updates.jsonl`, so a resume replays it.
    pub(super) fn announce_model_input(&self, item: &ConversationItem) {
        let Some(update) = model_input_update(item) else {
            return;
        };
        let notification = XaiSessionNotification {
            session_id: self.session_info.id.clone(),
            update,
            meta: Some(self.build_notification_meta()),
        };
        let _ = self
            .notifications
            .persistence_tx
            .send(PersistenceMsg::Update(
                crate::session::storage::SessionUpdate::Xai(Box::new(notification.clone())),
            ));
        let Ok(params) =
            serde_json::to_value(&notification).and_then(|v| serde_json::value::to_raw_value(&v))
        else {
            return;
        };
        self.notifications
            .gateway
            .forward_fire_and_forget(acp::ExtNotification::new(
                "x.ai/session_notification",
                params.into(),
            ));
    }
}

/// The row for a shell-authored item, or `None` for an item the user already sees or wrote.
pub(super) fn model_input_update(item: &ConversationItem) -> Option<XaiSessionUpdate> {
    let source = model_input_source(item.synthetic_reason()?)?;
    let text = item.text_content();
    if text.trim().is_empty() {
        return None;
    }
    Some(XaiSessionUpdate::ModelInput {
        source: source.to_string(),
        text,
    })
}

/// The user-facing name of what wrote an item. `None` for text the user typed or already has on screen.
fn model_input_source(reason: &SyntheticReason) -> Option<&'static str> {
    Some(match reason {
        SyntheticReason::Human
        | SyntheticReason::Interjection
        | SyntheticReason::DirectBash
        | SyntheticReason::Primary
        | SyntheticReason::ProjectInstructions
        | SyntheticReason::SessionPrefix
        | SyntheticReason::GoalSetup
        | SyntheticReason::HistoryFlattened
        | SyntheticReason::CompactionMeta => return None,
        SyntheticReason::SystemReminder => "system reminder",
        SyntheticReason::LengthContinue => "output-limit continue",
        SyntheticReason::AutoContinue => "auto-continue",
        SyntheticReason::AutoRecovery => "auto-recovery",
        SyntheticReason::AgentMessage | SyntheticReason::ParentHumanMessage => "agent message",
        SyntheticReason::TaskCompleted => "background task",
        SyntheticReason::SubagentCompleted => "subagent",
        SyntheticReason::TodoAdded => "todo capture",
        SyntheticReason::NotificationDrain => "notifications",
        SyntheticReason::GoalSummary => "goal",
        SyntheticReason::GoalClassifierNudge => "goal check",
        SyntheticReason::SchedulerFired => "scheduled task",
        SyntheticReason::StopHookFeedback => "stop gate",
        SyntheticReason::WorkingDirectorySwitch => "working directory",
        SyntheticReason::Unknown => "shell",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_gate_feedback_is_shown_in_full() {
        let item = ConversationItem::stop_hook_feedback("CI is red\nfix it");
        assert_eq!(
            model_input_update(&item),
            Some(XaiSessionUpdate::ModelInput {
                source: "stop gate".into(),
                text: "CI is red\nfix it".into(),
            })
        );
    }

    #[test]
    fn goal_directive_is_shown() {
        let item = ConversationItem::goal_summary("keep going");
        assert!(matches!(
            model_input_update(&item),
            Some(XaiSessionUpdate::ModelInput { source, .. }) if source == "goal"
        ));
    }

    #[test]
    fn text_the_user_typed_gets_no_row() {
        assert_eq!(model_input_update(&ConversationItem::user("hi")), None);
    }

    #[test]
    fn an_empty_item_gets_no_row() {
        assert_eq!(
            model_input_update(&ConversationItem::stop_hook_feedback("  ")),
            None
        );
    }
}
