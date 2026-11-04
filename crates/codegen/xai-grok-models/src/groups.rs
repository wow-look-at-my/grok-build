//! Model groups: several harness model slots that share one model and one settings row.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::slots::{HARNESS_MODEL_SLOTS, slot_by_id};

/// One `[model_groups.<id>]` table. Every field is optional. On a compiled
/// group, a field that is not set keeps the compiled value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelGroupConfig {
    /// Settings-modal row label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Settings-modal row description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Slot ids in the group. `members = []` unrolls a compiled group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<String>>,
    /// The model every member uses. Unset means each member falls back on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A compiled default group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultModelGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub members: &'static [&'static str],
}

/// The compiled groups, in settings-modal order.
pub const DEFAULT_MODEL_GROUPS: &[DefaultModelGroup] = &[
    DefaultModelGroup {
        id: "summarization",
        label: "Summarization model",
        description: "Model for the short summaries: session titles, prompt suggestions, \
		              recaps, turn summaries, thinking summaries and goal summaries.",
        members: &[
            "session_summary",
            "prompt_suggestion",
            "recap",
            "turn_summary",
            "thinking_summary",
            "goal_summarizer",
        ],
    },
    DefaultModelGroup {
        id: "dense_summarization",
        label: "Dense summarization model",
        description: "Model for the long summaries: web-search synthesis, context \
		              compaction and image descriptions.",
        members: &["web_search", "compaction", "image_description"],
    },
    DefaultModelGroup {
        id: "query",
        label: "Query model",
        description: "Model for the quick side queries: `/btw` side notes and `/todo` capture.",
        members: &["side_note", "todo_capture"],
    },
];

/// A group after the config is applied. Only a group with members is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGroup {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Slot ids, in `HARNESS_MODEL_SLOTS` order.
    pub members: Vec<&'static str>,
    /// The group's model, trimmed. `None` when unset or blank.
    pub model: Option<String>,
}

impl ModelGroup {
    /// True when `slot_id` is a member.
    pub fn contains(&self, slot_id: &str) -> bool {
        self.members.contains(&slot_id)
    }
}

/// Apply `[model_groups]` to the compiled groups. A slot that a user group
/// lists leaves every compiled group. A user group can take a slot without an
/// edit to the compiled group. User groups that list the same slot are an
/// error. So are an unknown slot id and a bad group id.
pub fn resolve_model_groups(
    user: &BTreeMap<String, ModelGroupConfig>,
) -> Result<Vec<ModelGroup>, String> {
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
    for (group_id, group) in user {
        validate_group_id(group_id)?;
        for member in group.members.iter().flatten() {
            let Some(slot) = slot_by_id(member) else {
                return Err(format!(
                    "[model_groups.{group_id}] lists unknown model slot '{member}'. \
					 Known slots: {}",
                    HARNESS_MODEL_SLOTS
                        .iter()
                        .map(|s| s.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            if let Some(other) = claimed.insert(slot.id, group_id) {
                if other == group_id {
                    return Err(format!(
                        "[model_groups.{group_id}] lists '{member}' more than once"
                    ));
                }
                return Err(format!(
                    "model slot '{member}' is in [model_groups.{other}] and \
					 [model_groups.{group_id}]. A slot can be in one group only"
                ));
            }
        }
    }

    let mut groups = Vec::new();
    for default in DEFAULT_MODEL_GROUPS {
        let user_group = user.get(default.id);
        let members: Vec<&str> = match user_group.and_then(|g| g.members.as_ref()) {
            Some(list) => list.iter().map(String::as_str).collect(),
            None => default
                .members
                .iter()
                .copied()
                .filter(|m| !claimed.contains_key(m))
                .collect(),
        };
        groups.push(build_group(
            default.id,
            user_group,
            default.label.to_owned(),
            default.description.to_owned(),
            &members,
        ));
    }
    for (group_id, group) in user {
        if DEFAULT_MODEL_GROUPS.iter().any(|d| d.id == group_id) {
            continue;
        }
        let members: Vec<&str> = group.members.iter().flatten().map(String::as_str).collect();
        groups.push(build_group(
            group_id,
            Some(group),
            format!("{} model", group_id.replace(['_', '-'], " ")),
            String::new(),
            &members,
        ));
    }
    groups.retain(|g| !g.members.is_empty());
    Ok(groups)
}

/// The model the group of `slot_id` sets, or `None` when the slot is in no
/// group or its group sets no model. An invalid `[model_groups]` answers
/// `None` here. Config load reports that error and stops the start.
pub fn group_model_for_slot(
    user: &BTreeMap<String, ModelGroupConfig>,
    slot_id: &str,
) -> Option<String> {
    let groups = resolve_model_groups(user).ok()?;
    group_for_slot(&groups, slot_id)?.model.clone()
}

/// The group that holds `slot_id`, if any.
pub fn group_for_slot<'a>(groups: &'a [ModelGroup], slot_id: &str) -> Option<&'a ModelGroup> {
    groups.iter().find(|g| g.contains(slot_id))
}

fn build_group(
    id: &str,
    user: Option<&ModelGroupConfig>,
    label: String,
    description: String,
    members: &[&str],
) -> ModelGroup {
    let wanted: BTreeSet<&str> = members.iter().copied().collect();
    let members: Vec<&'static str> = HARNESS_MODEL_SLOTS
        .iter()
        .map(|s| s.id)
        .filter(|id| wanted.contains(id))
        .collect();
    let non_blank = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let label = user.and_then(|g| non_blank(&g.label)).unwrap_or(label);
    let description = user
        .and_then(|g| non_blank(&g.description))
        .unwrap_or(description);
    let description = if description.is_empty() {
        format!(
            "Model for: {}.",
            members
                .iter()
                .filter_map(|m| slot_by_id(m))
                .map(|s| s.label)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        description
    };
    ModelGroup {
        id: id.to_owned(),
        label,
        description,
        members,
        model: user.and_then(|g| non_blank(&g.model)),
    }
}

fn validate_group_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        return Ok(());
    }
    Err(format!(
        "[model_groups.{id}]: a group id uses only a-z, 0-9, '_' and '-'"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(entries: &[(&str, ModelGroupConfig)]) -> BTreeMap<String, ModelGroupConfig> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn members(list: &[&str]) -> ModelGroupConfig {
        ModelGroupConfig {
            members: Some(list.iter().map(|s| (*s).to_owned()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn defaults_name_real_slots_and_no_slot_twice() {
        let mut seen = BTreeSet::new();
        for group in DEFAULT_MODEL_GROUPS {
            for member in group.members {
                assert!(
                    slot_by_id(member).is_some(),
                    "{} lists unknown {member}",
                    group.id
                );
                assert!(seen.insert(*member), "{member} is in two default groups");
            }
        }
    }

    #[test]
    fn an_empty_config_gives_the_compiled_groups() {
        let groups = resolve_model_groups(&BTreeMap::new()).expect("defaults are valid");
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["summarization", "dense_summarization", "query"]);
        let summarization = &groups[0];
        assert_eq!(summarization.label, "Summarization model");
        for slot in [
            "session_summary",
            "prompt_suggestion",
            "recap",
            "turn_summary",
            "thinking_summary",
            "goal_summarizer",
        ] {
            assert!(
                summarization.contains(slot),
                "summarization must hold {slot}"
            );
        }
        assert_eq!(
            group_for_slot(&groups, "compaction").map(|g| g.id.as_str()),
            Some("dense_summarization")
        );
        assert_eq!(
            group_for_slot(&groups, "todo_capture").map(|g| g.id.as_str()),
            Some("query")
        );
        assert!(group_for_slot(&groups, "goal_planner").is_none());
        assert!(groups.iter().all(|g| g.model.is_none()));
    }

    #[test]
    fn empty_members_unrolls_a_compiled_group() {
        let groups = resolve_model_groups(&user(&[("query", members(&[]))])).expect("valid");
        assert!(
            groups.iter().all(|g| g.id != "query"),
            "an empty group is hidden"
        );
        assert!(group_for_slot(&groups, "side_note").is_none());
    }

    #[test]
    fn a_user_group_takes_its_slots_out_of_the_compiled_groups() {
        let groups = resolve_model_groups(&user(&[("fast", members(&["recap", "goal_planner"]))]))
            .expect("valid");
        let fast = groups.iter().find(|g| g.id == "fast").expect("fast exists");
        assert_eq!(fast.members, ["recap", "goal_planner"]);
        assert_eq!(fast.label, "fast model");
        assert!(fast.description.contains("Recap model"));
        let summarization = groups.iter().find(|g| g.id == "summarization").unwrap();
        assert!(!summarization.contains("recap"));
        assert!(summarization.contains("turn_summary"));
    }

    #[test]
    fn a_model_only_override_keeps_the_compiled_members() {
        let cfg = ModelGroupConfig {
            model: Some(" some-model ".to_owned()),
            label: Some("Cheap".to_owned()),
            ..Default::default()
        };
        let groups = resolve_model_groups(&user(&[("query", cfg)])).expect("valid");
        let query = groups.iter().find(|g| g.id == "query").unwrap();
        assert_eq!(query.model.as_deref(), Some("some-model"));
        assert_eq!(query.label, "Cheap");
        assert_eq!(query.members, ["side_note", "todo_capture"]);
    }

    #[test]
    fn a_user_group_with_no_members_is_hidden() {
        let groups =
            resolve_model_groups(&user(&[("spare", ModelGroupConfig::default())])).expect("valid");
        assert!(groups.iter().all(|g| g.id != "spare"));
    }

    #[test]
    fn a_slot_in_two_user_groups_is_an_error() {
        let err = resolve_model_groups(&user(&[
            ("a", members(&["recap"])),
            ("b", members(&["recap"])),
        ]))
        .unwrap_err();
        assert!(err.contains("one group only"), "{err}");
    }

    #[test]
    fn a_slot_listed_twice_in_one_group_is_an_error() {
        let err = resolve_model_groups(&user(&[("a", members(&["recap", "recap"]))])).unwrap_err();
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn an_unknown_slot_is_an_error() {
        let err = resolve_model_groups(&user(&[("a", members(&["nope"]))])).unwrap_err();
        assert!(err.contains("unknown model slot 'nope'"), "{err}");
    }

    #[test]
    fn a_bad_group_id_is_an_error() {
        let err = resolve_model_groups(&user(&[("Bad Id", members(&["recap"]))])).unwrap_err();
        assert!(err.contains("a group id uses only"), "{err}");
    }

    #[test]
    fn the_toml_shape_parses() {
        let parsed: BTreeMap<String, ModelGroupConfig> = serde_json::from_str(
            r#"{"query": {"members": ["side_note"], "model": "m"}, "fast": {"label": "Fast"}}"#,
        )
        .expect("parses");
        assert_eq!(
            parsed["query"].members.as_deref(),
            Some(&["side_note".to_owned()][..])
        );
        assert_eq!(parsed["fast"].label.as_deref(), Some("Fast"));
    }
}
