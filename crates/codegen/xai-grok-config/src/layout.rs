//! The section layout of a JSON config file.

/// One row of the layout: the path in a JSON config file, and the internal path it maps to.
#[derive(Debug, Clone, Copy)]
pub struct LayoutRule {
    pub json: &'static [&'static str],
    pub internal: &'static [&'static str],
}

const fn rule(json: &'static [&'static str], internal: &'static [&'static str]) -> LayoutRule {
    LayoutRule { json, internal }
}

/// Every subtree that the JSON layout puts somewhere other than its internal path.
pub const LAYOUT: &[LayoutRule] = &[
    // models
    rule(&["models", "providers"], &["model_providers"]),
    rule(&["models", "definitions"], &["model"]),
    rule(&["models", "groups"], &["model_groups"]),
    rule(&["models", "auth_providers"], &["auth_provider"]),
    rule(&["models", "pricing"], &["pricing"]),
    // login
    rule(&["login"], &["grok_com_config"]),
    // ui
    rule(&["ui", "dashboard"], &["dashboard"]),
    rule(&["ui", "tips"], &["tips"]),
    rule(&["ui", "hints"], &["hints"]),
    rule(&["ui", "slash_command_tags"], &["slash_command_tags"]),
    rule(&["ui", "voice"], &["voice"]),
    // tools
    rule(&["tools", "bash"], &["toolset", "bash"]),
    rule(&["tools", "web_search"], &["toolset", "web_search"]),
    rule(&["tools", "web_fetch"], &["toolset", "web_fetch"]),
    rule(
        &["tools", "ask_user_question"],
        &["toolset", "ask_user_question"],
    ),
    rule(&["tools", "file_toolset"], &["toolset", "file_toolset"]),
    rule(&["tools", "hashline"], &["toolset", "hashline"]),
    rule(&["tools", "disable_web_search"], &["disable_web_search"]),
    rule(
        &["tools", "path_not_found_hints"],
        &["path_not_found_hints"],
    ),
    rule(
        &["tools", "shell_environment_policy"],
        &["shell_environment_policy"],
    ),
    // mcp
    rule(&["mcp", "servers"], &["mcp_servers"]),
    rule(&["mcp", "disabled_servers"], &["disabled_mcp_servers"]),
    rule(&["mcp", "disabled_tools"], &["disabled_mcp_tools"]),
    rule(&["mcp", "managed"], &["managed_mcps"]),
    // agents
    rule(&["agents", "selection"], &["agent"]),
    rule(&["agents", "subagents"], &["subagents"]),
    rule(&["agents", "goal"], &["goal"]),
    rule(&["agents", "auto_mode"], &["auto_mode"]),
    rule(&["agents", "default_auto_mode"], &["default_auto_mode"]),
    rule(&["agents", "workflows"], &["workflows"]),
    rule(&["agents", "prompt_suggestions"], &["prompt_suggestions"]),
    rule(&["agents", "doom_loop_recovery"], &["doom_loop_recovery"]),
    rule(
        &["agents", "long_reasoning_reminder"],
        &["long_reasoning_reminder"],
    ),
    rule(&["agents", "output_rate_floor"], &["output_rate_floor"]),
    rule(&["agents", "cursor_worker"], &["cursor_worker"]),
    // session
    rule(&["session", "compaction"], &["compaction"]),
    rule(&["session", "repo_changes_dedup"], &["repo_changes_dedup"]),
    rule(&["session", "storage"], &["storage"]),
    rule(&["session", "harness"], &["harness"]),
    rule(&["session", "relay"], &["relay"]),
    rule(&["session", "hub"], &["hub"]),
    // memory
    rule(&["memory", "v2"], &["memory_v2"]),
    // worktrees: the longer row comes first, so the `pool` subtree never lands in `worktree`
    rule(&["worktrees", "pool"], &["worktree_pool"]),
    rule(&["worktrees"], &["worktree"]),
    // extensions
    rule(&["extensions", "skills"], &["skills"]),
    rule(&["extensions", "plugins"], &["plugins"]),
    rule(&["extensions", "marketplace"], &["marketplace"]),
    rule(&["extensions", "paths"], &["paths"]),
    rule(&["extensions", "compat"], &["compat"]),
    // telemetry and privacy
    rule(&["telemetry", "feedback"], &["feedback"]),
    rule(&["telemetry", "diagnostics"], &["diagnostics"]),
    rule(&["privacy", "consent"], &["consent"]),
    // admin policy
    rule(&["policy", "allowed_mcp_servers"], &["allowed_mcp_servers"]),
    rule(&["policy", "denied_mcp_servers"], &["denied_mcp_servers"]),
    rule(
        &["policy", "allow_managed_mcp_servers_only"],
        &["allow_managed_mcp_servers_only"],
    ),
    rule(
        &["policy", "allow_managed_hooks_only"],
        &["allow_managed_hooks_only"],
    ),
    rule(
        &["policy", "enable_all_project_mcp_servers"],
        &["enable_all_project_mcp_servers"],
    ),
    rule(&["policy", "plugin_auto_update"], &["plugin_auto_update"]),
    rule(
        &["policy", "strict_known_marketplaces"],
        &["strict_known_marketplaces"],
    ),
    rule(
        &["policy", "extra_known_marketplaces"],
        &["extra_known_marketplaces"],
    ),
];

/// Internal keys that an older config spelled differently. A migration moves them to the JSON path of the key they alias.
const INTERNAL_ALIASES: &[LayoutRule] = &[rule(&["login"], &["auth"])];

/// The root keys whose array elements are config patches. Each element is moved like a root document.
pub const PATCH_ARRAY_KEYS: &[&str] = &["version_overrides", "campaigns"];

/// Move a document from the JSON layout to the internal keys.
pub fn json_to_internal(doc: &mut toml::Value) {
    relayout(doc, Direction::ToInternal);
}

/// Move a document from the internal keys to the JSON layout.
pub fn internal_to_json(doc: &mut toml::Value) {
    relayout(doc, Direction::ToJson);
}

/// The JSON path for an internal dotted path, for messages that name a key.
pub fn internal_path_to_json(path: &str) -> String {
    let segs: Vec<&str> = path.split('.').collect();
    for r in LAYOUT.iter().chain(INTERNAL_ALIASES) {
        if segs.starts_with(r.internal) {
            let mut out: Vec<&str> = r.json.to_vec();
            out.extend(segs.iter().skip(r.internal.len()));
            return out.join(".");
        }
    }
    path.to_string()
}

/// The internal dotted path for a JSON dotted path. Inverse of [`internal_path_to_json`].
pub fn json_path_to_internal(path: &str) -> String {
    let segs: Vec<&str> = path.split('.').collect();
    for r in LAYOUT {
        if segs.starts_with(r.json) {
            let mut out: Vec<&str> = r.internal.to_vec();
            out.extend(segs.iter().skip(r.json.len()));
            return out.join(".");
        }
    }
    path.to_string()
}

#[derive(Clone, Copy)]
enum Direction {
    ToInternal,
    ToJson,
}

fn relayout(doc: &mut toml::Value, dir: Direction) {
    let Some(root) = doc.as_table_mut() else {
        return;
    };
    for key in PATCH_ARRAY_KEYS {
        if let Some(toml::Value::Array(items)) = root.get_mut(*key) {
            for item in items.iter_mut() {
                relayout_table(item, dir);
            }
        }
    }
    relayout_table(doc, dir);
}

fn relayout_table(doc: &mut toml::Value, dir: Direction) {
    if !doc.is_table() {
        return;
    }
    let rules: Vec<(&[&str], &[&str])> = match dir {
        Direction::ToInternal => LAYOUT.iter().map(|r| (r.json, r.internal)).collect(),
        Direction::ToJson => LAYOUT
            .iter()
            .chain(INTERNAL_ALIASES)
            .map(|r| (r.internal, r.json))
            .collect(),
    };
    // Take every subtree out first, then insert, so a row whose target is the source of another row cannot move a value twice
    let mut moved = Vec::new();
    for (from, to) in rules {
        if let Some(value) = take_path(doc, from) {
            moved.push((to, value));
            prune_empty_parent(doc, from);
        }
    }
    for (to, value) in moved {
        insert_merge(doc, to, value);
    }
}

fn take_path(doc: &mut toml::Value, path: &[&str]) -> Option<toml::Value> {
    let (last, parents) = path.split_last()?;
    let mut cur = doc;
    for seg in parents {
        cur = cur.as_table_mut()?.get_mut(*seg)?;
    }
    let table = cur.as_table_mut()?;
    let value = table.remove(*last)?;
    Some(value)
}

/// Remove the parent table of `path` when the move left it empty. A section that only held moved subtrees must not stay as an empty table.
fn prune_empty_parent(doc: &mut toml::Value, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        return;
    }
    let Some(root) = doc.as_table_mut() else {
        return;
    };
    let empty = root
        .get(*first)
        .and_then(toml::Value::as_table)
        .is_some_and(|t| t.is_empty());
    if empty {
        root.remove(*first);
    }
}

fn insert_merge(doc: &mut toml::Value, path: &[&str], value: toml::Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = doc;
    for seg in parents {
        let Some(table) = cur.as_table_mut() else {
            return;
        };
        let slot = table
            .entry(seg.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        if !slot.is_table() {
            *slot = toml::Value::Table(toml::map::Map::new());
        }
        cur = slot;
    }
    let Some(table) = cur.as_table_mut() else {
        return;
    };
    match table.get_mut(*last) {
        // Spellings of one section: the value already in place is the lower layer, so the moved value wins on a conflict
        Some(existing) => crate::loader::deep_merge_toml(existing, &value),
        None => {
            table.insert(last.to_string(), value);
        }
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
