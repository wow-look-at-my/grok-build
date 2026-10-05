//! Types are canonical in `xai-grok-tools`.
//! This module adds conversions between ACP plan entries and `TodoItem` since `xai-grok-tools` is protocol-agnostic.

pub use xai_grok_tools::implementations::grok_build::todo::TodoId;
pub use xai_grok_tools::implementations::grok_build::todo::TodoItem;
pub use xai_grok_tools::implementations::grok_build::todo::TodoPriority;
pub use xai_grok_tools::implementations::grok_build::todo::TodoState;
pub use xai_grok_tools::implementations::grok_build::todo::TodoStatus;

use agent_client_protocol as acp;

/// `PlanEntry.meta` key carrying an item's verifier prompt across ACP, which has no field for it.
pub const VERIFICATION_META_KEY: &str = "x.ai/verification";
/// `PlanEntry.meta` key carrying whether the item's verifier has passed.
pub const VERIFICATION_PASSED_META_KEY: &str = "x.ai/verificationPassed";

/// ACP has no `Cancelled` status, so cancelled items are stored as `Completed` with `{"cancelled": true}` in meta.
pub fn todo_item_from_plan_entry(entry: acp::PlanEntry) -> TodoItem {
    let status = match entry.status {
        acp::PlanEntryStatus::Pending => TodoStatus::Pending,
        acp::PlanEntryStatus::InProgress => TodoStatus::InProgress,
        acp::PlanEntryStatus::Completed => {
            if entry
                .meta
                .as_ref()
                .and_then(|m| m.get("cancelled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                TodoStatus::Cancelled
            } else {
                TodoStatus::Completed
            }
        }
        // TODO(acp-0.10): `PlanEntryStatus` is #[non_exhaustive].
        _ => TodoStatus::Pending,
    };
    let mut verification = None;
    let mut verification_passed = false;
    let mut meta = entry.meta.map(serde_json::Value::Object);
    if let Some(object) = meta.as_mut().and_then(|m| m.as_object_mut()) {
        verification = object
            .remove(VERIFICATION_META_KEY)
            .and_then(|v| v.as_str().map(str::to_owned));
        verification_passed = object
            .remove(VERIFICATION_PASSED_META_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if object.is_empty() {
            meta = None;
        }
    }
    TodoItem {
        content: entry.content,
        priority: match entry.priority {
            acp::PlanEntryPriority::High => TodoPriority::High,
            acp::PlanEntryPriority::Medium => TodoPriority::Medium,
            acp::PlanEntryPriority::Low => TodoPriority::Low,
            // TODO(acp-0.10): `PlanEntryPriority` is #[non_exhaustive].
            _ => TodoPriority::Medium,
        },
        status,
        meta,
        verification,
        verification_passed,
    }
}

/// Cancelled items become `Completed` with `{"cancelled": true}` in meta.
pub(crate) fn plan_entry_from_todo_item(item: TodoItem) -> acp::PlanEntry {
    let status = match item.status {
        TodoStatus::Pending => acp::PlanEntryStatus::Pending,
        TodoStatus::InProgress => acp::PlanEntryStatus::InProgress,
        TodoStatus::Completed => acp::PlanEntryStatus::Completed,
        TodoStatus::Cancelled => acp::PlanEntryStatus::Completed,
    };
    let mut meta = item.meta;
    if item.status == TodoStatus::Cancelled {
        let mut m = meta.unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = m.as_object_mut() {
            obj.insert("cancelled".into(), true.into());
        }
        meta = Some(m);
    }
    if let Some(verification) = item.verification {
        let mut m = meta.unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = m.as_object_mut() {
            obj.insert(VERIFICATION_META_KEY.into(), verification.into());
            obj.insert(
                VERIFICATION_PASSED_META_KEY.into(),
                item.verification_passed.into(),
            );
        }
        meta = Some(m);
    }
    acp::PlanEntry::new(
        item.content,
        match item.priority {
            TodoPriority::High => acp::PlanEntryPriority::High,
            TodoPriority::Medium => acp::PlanEntryPriority::Medium,
            TodoPriority::Low => acp::PlanEntryPriority::Low,
        },
        status,
    )
    .meta(meta.and_then(|v| v.as_object().cloned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(verification: Option<&str>, passed: bool) -> TodoItem {
        TodoItem {
            content: "Ship the parser".to_string(),
            priority: TodoPriority::Medium,
            status: TodoStatus::InProgress,
            meta: None,
            verification: verification.map(str::to_owned),
            verification_passed: passed,
        }
    }

    /// The verifier prompt and its passed flag survive the ACP round trip, so
    /// the todo pane sees what the tool stored.
    #[test]
    fn verification_survives_the_plan_entry_round_trip() {
        let entry = plan_entry_from_todo_item(item(Some("cargo test passes"), false));
        let back = todo_item_from_plan_entry(entry);
        assert_eq!(back.verification.as_deref(), Some("cargo test passes"));
        assert!(!back.verification_passed);
        assert!(
            back.meta.is_none(),
            "the verification keys are lifted out of meta: {:?}",
            back.meta
        );

        let entry = plan_entry_from_todo_item(item(Some("cargo test passes"), true));
        let back = todo_item_from_plan_entry(entry);
        assert!(back.verification_passed);
    }

    /// An item with no verifier carries no verification meta at all.
    #[test]
    fn no_verifier_leaves_meta_empty() {
        let entry = plan_entry_from_todo_item(item(None, false));
        assert!(entry.meta.is_none());
        let back = todo_item_from_plan_entry(entry);
        assert!(back.verification.is_none());
        assert!(back.meta.is_none());
    }
}
