//! The model groups the settings modal shows, read once from `[model_groups]`. A grouped slot has no row of its own:
//! its group's row sets it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One group row, ready for the settings registry.
#[derive(Debug)]
pub struct ModelGroupRow {
    pub id: &'static str,
    /// `models.group.<id>`.
    pub setting_key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    /// Slot ids, in `HARNESS_MODEL_SLOTS` order. Never empty.
    pub members: Vec<&'static str>,
}

/// The setting-key prefix of a group row.
pub const GROUP_KEY_PREFIX: &str = "models.group.";

/// Every group with at least one member, in settings-modal order.
pub fn model_groups() -> &'static [ModelGroupRow] {
    static ROWS: OnceLock<Vec<ModelGroupRow>> = OnceLock::new();
    ROWS.get_or_init(|| load().into_iter().map(leak).collect())
}

/// The group a settings key names, or `None` for any other key.
pub fn group_for_setting_key(key: &str) -> Option<&'static ModelGroupRow> {
    key.strip_prefix(GROUP_KEY_PREFIX)?;
    model_groups().iter().find(|g| g.setting_key == key)
}

/// The group that holds `slot_id`, if any.
pub fn group_holding_slot(slot_id: &str) -> Option<&'static ModelGroupRow> {
    model_groups().iter().find(|g| g.members.contains(&slot_id))
}

/// The resolved groups. Unit tests read the compiled groups only, so the
/// developer's own config cannot change a test result.
fn load() -> Vec<xai_grok_models::ModelGroup> {
    let user = if cfg!(test) {
        BTreeMap::new()
    } else {
        user_groups()
    };
    match xai_grok_models::resolve_model_groups(&user) {
        Ok(groups) => groups,
        Err(error) => {
            tracing::error!(
                target: "settings",
                %error,
                "[model_groups] is invalid; the settings modal shows the compiled groups"
            );
            xai_grok_models::resolve_model_groups(&BTreeMap::new())
                .expect("the compiled groups are valid")
        }
    }
}

fn user_groups() -> BTreeMap<String, xai_grok_models::ModelGroupConfig> {
    let Ok(root) = xai_grok_shell::config::load_effective_config() else {
        return BTreeMap::new();
    };
    let Some(table) = root.get("model_groups") else {
        return BTreeMap::new();
    };
    match table.clone().try_into() {
        Ok(groups) => groups,
        Err(error) => {
            tracing::error!(
                target: "settings",
                %error,
                "[model_groups] does not parse; the settings modal shows the compiled groups"
            );
            BTreeMap::new()
        }
    }
}

fn leak(group: xai_grok_models::ModelGroup) -> ModelGroupRow {
    let mut keywords: Vec<&'static str> = vec!["group", "model"];
    for member in &group.members {
        let Some(slot) = xai_grok_models::slot_by_id(member) else {
            continue;
        };
        for kw in slot.keywords {
            if !keywords.contains(kw) {
                keywords.push(kw);
            }
        }
    }
    ModelGroupRow {
        setting_key: Box::leak(format!("{GROUP_KEY_PREFIX}{}", group.id).into_boxed_str()),
        id: Box::leak(group.id.into_boxed_str()),
        label: Box::leak(group.label.into_boxed_str()),
        description: Box::leak(group.description.into_boxed_str()),
        keywords: Box::leak(keywords.into_boxed_slice()),
        members: group.members,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_groups_have_rows_and_keys_that_round_trip() {
        let groups = model_groups();
        assert!(!groups.is_empty());
        for group in groups {
            assert!(!group.members.is_empty(), "{} has no members", group.id);
            assert_eq!(
                group_for_setting_key(group.setting_key).map(|g| g.id),
                Some(group.id)
            );
            for member in &group.members {
                assert_eq!(group_holding_slot(member).map(|g| g.id), Some(group.id));
                assert!(
                    xai_grok_models::slot_for_setting_key(group.setting_key).is_none(),
                    "a group key must never name a slot"
                );
            }
            for kw in group.keywords {
                assert_eq!(*kw, kw.to_lowercase(), "{} keyword {kw}", group.id);
            }
        }
        assert!(group_for_setting_key("models.recap").is_none());
        assert!(group_holding_slot("goal_planner").is_none());
    }
}
