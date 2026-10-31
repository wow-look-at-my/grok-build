//! One model row per spawnable subagent type, read once from the agent definitions the session can spawn.

/// The setting-key prefix of a subagent model row.
pub const KEY_PREFIX: &str = "subagents.models.";

/// One spawnable subagent type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentType {
    pub name: String,
    pub description: String,
}

/// One row in the settings registry: the model a subagent type runs on.
#[derive(Debug)]
pub struct SubagentModelRow {
    /// The `[subagents.models]` key this row writes.
    pub name: &'static str,
    pub setting_key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
}

const KEYWORDS: &[&str] = &["subagent", "agent", "type", "model"];

/// Every spawnable subagent type: the built-in types, then the definitions
/// discovery finds for the workspace, then plugin-provided agents.
pub fn subagent_types() -> Vec<SubagentType> {
    let mut types: Vec<SubagentType> =
        xai_grok_agent::config::BuiltinAgentName::subagent_variants()
            .iter()
            .map(|builtin| {
                let definition = builtin.definition();
                SubagentType {
                    name: definition.name.clone(),
                    description: definition.description.clone(),
                }
            })
            .collect();
    // Tests read the built-in list only, so a developer's own agents cannot
    // change a test result.
    if cfg!(test) {
        return types;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    for definition in xai_grok_agent::discovery::discover(&cwd) {
        if definition.scope == xai_grok_agent::config::AgentScope::BuiltIn {
            continue;
        }
        push_unique(
            &mut types,
            SubagentType {
                name: definition.name.clone(),
                description: definition.description.clone(),
            },
        );
    }
    let registry = xai_grok_shell::util::config::load_cli_plugin_registry(&cwd);
    for agent in xai_grok_agent::discovery::plugin_agents(&registry) {
        push_unique(
            &mut types,
            SubagentType {
                name: agent.qualified_name.clone(),
                description: agent.definition.description.clone(),
            },
        );
    }
    types
}

fn push_unique(types: &mut Vec<SubagentType>, candidate: SubagentType) {
    if types.iter().any(|known| known.name == candidate.name) {
        return;
    }
    types.push(candidate);
}

/// Every subagent type with a settings row, in settings-modal order.
pub fn rows() -> &'static [SubagentModelRow] {
    static ROWS: std::sync::OnceLock<Vec<SubagentModelRow>> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| rows_from(&subagent_types()))
}

/// Build rows for an explicit type list. Every field the registry holds as
/// `&'static str` is allocated here because the type list is known only at
/// run time.
pub fn rows_from(types: &[SubagentType]) -> Vec<SubagentModelRow> {
    types
        .iter()
        .map(|kind| {
            let name: &'static str = Box::leak(kind.name.clone().into_boxed_str());
            let description: &'static str = Box::leak(
                format!("Model the `{name}` subagent runs on. Unset inherits the session model.")
                    .into_boxed_str(),
            );
            SubagentModelRow {
                name,
                setting_key: Box::leak(format!("{KEY_PREFIX}{name}").into_boxed_str()),
                label: Box::leak(format!("{name} subagent model").into_boxed_str()),
                description,
                keywords: KEYWORDS,
            }
        })
        .collect()
}

/// Whether a settings key names a subagent model row.
pub fn is_setting_key(key: &str) -> bool {
    key.starts_with(KEY_PREFIX)
}

/// The row a settings key names, or `None` for any other key.
pub fn row_for_setting_key(key: &str) -> Option<&'static SubagentModelRow> {
    find_by_key(rows(), key)
}

/// The subagent type a settings key names.
pub fn type_for_setting_key(key: &str) -> Option<&str> {
    key.strip_prefix(KEY_PREFIX)
}

/// The row a subagent type names, or `None` when the type has no row.
pub fn row_for_name(name: &str) -> Option<&'static SubagentModelRow> {
    rows().iter().find(|row| row.name == name)
}

fn find_by_key<'a>(rows: &'a [SubagentModelRow], key: &str) -> Option<&'a SubagentModelRow> {
    key.strip_prefix(KEY_PREFIX)?;
    rows.iter().find(|row| row.setting_key == key)
}

/// The `[subagents.models]` pins the config file holds.
pub fn pinned_models(config: &toml::Value) -> std::collections::BTreeMap<String, String> {
    let mut pins = std::collections::BTreeMap::new();
    let Some(models) = config
        .get("subagents")
        .and_then(|subagents| subagents.get("models"))
        .and_then(toml::Value::as_table)
    else {
        return pins;
    };
    for (name, value) in models {
        if let Some(model) = value.as_str()
            && !model.trim().is_empty()
        {
            pins.insert(name.clone(), model.trim().to_owned());
        }
    }
    pins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> SubagentType {
        SubagentType {
            name: name.to_owned(),
            description: String::new(),
        }
    }

    #[test]
    fn every_builtin_subagent_type_has_a_row() {
        let rows = rows();
        for builtin in xai_grok_agent::config::BuiltinAgentName::subagent_variants() {
            let name = builtin.definition().name;
            assert!(
                rows.iter().any(|row| row.name == name),
                "no model row for built-in subagent type {name}"
            );
        }
    }

    #[test]
    fn a_discovered_type_gets_its_own_row_and_key() {
        let types = [kind("general-purpose"), kind("seeded-agent")];
        let rows = rows_from(&types);
        let seeded = rows
            .iter()
            .find(|row| row.name == "seeded-agent")
            .expect("seeded type has a row");
        assert_eq!(seeded.setting_key, "subagents.models.seeded-agent");
        assert!(seeded.label.contains("seeded-agent"));
        assert!(!seeded.description.is_empty());
    }

    #[test]
    fn every_row_reads_back_its_own_key() {
        let types = [kind("explore"), kind("plugin:reviewer")];
        let rows = rows_from(&types);
        for row in &rows {
            let found = find_by_key(&rows, row.setting_key).expect("row is found by its key");
            assert_eq!(found.name, row.name);
        }
        assert!(find_by_key(&rows, "subagents.models.missing").is_none());
    }

    #[test]
    fn pins_are_read_from_the_subagents_table() {
        let root: toml::Value =
            toml::from_str("[subagents.models]\nexplore = \"fast-model\"\nblank = \"  \"\n")
                .unwrap();
        let pins = pinned_models(&root);
        assert_eq!(pins.get("explore").map(String::as_str), Some("fast-model"));
        assert!(!pins.contains_key("blank"));
    }
}
