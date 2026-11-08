//! The JSON Schema of `config.json`, built from the config types.

use serde_json::{Map, Value, json};
use xai_grok_config::layout::{LAYOUT, PATCH_ARRAY_KEYS};

use crate::agent::config::{Config, ConfigModelOverride};
use crate::agent::model_providers::ModelProviderConfig;

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The keys of a `version_overrides` element that gate it, not config.
const VERSION_OVERRIDE_META: &[&str] = &["minimum_version", "maximum_version"];

/// The schema of `config.json` in the JSON layout.
pub fn config_json_schema() -> Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let mut extra = Map::new();
    extra.insert(
        "model".into(),
        map_of(generator.subschema_for::<ConfigModelOverride>()),
    );
    extra.insert(
        "model_providers".into(),
        map_of(generator.subschema_for::<ModelProviderConfig>()),
    );
    extra.insert(
        "auth_provider".into(),
        map_of(generator.subschema_for::<xai_grok_config_types::AuthProviderConfig>()),
    );
    extra.insert("hooks".into(), hooks_schema(&mut generator));
    extra.insert(
        "voice".into(),
        generator
            .subschema_for::<xai_grok_voice::VoiceConfig>()
            .to_value(),
    );
    extra.insert(
        "privacy".into(),
        generator
            .subschema_for::<crate::util::config::PrivacyConfig>()
            .to_value(),
    );
    extra.insert(
        "consent".into(),
        generator
            .subschema_for::<crate::util::config::ConsentConfig>()
            .to_value(),
    );
    let root = generator.into_root_schema_for::<Config>().to_value();
    let Value::Object(mut root) = root else {
        panic!("the Config schema is not an object");
    };
    let mut defs = match root.remove("$defs") {
        Some(Value::Object(defs)) => defs,
        _ => Map::new(),
    };
    let props = object_at(&mut root, "properties");
    props.extend(extra);
    props.extend(raw_root_keys());
    // `auth` is a legacy spelling of `grok_com_config`; the JSON layout spells it `login`
    props.remove("auth");

    add_raw_section_keys(&mut root, &mut defs);
    strip_section_required(&mut root, &mut defs);
    for rule in LAYOUT {
        let moved = take_property(&mut root, &mut defs, rule.internal).unwrap_or_else(|| {
            panic!(
                "layout row {:?} names an internal key that the schema does not have",
                rule.internal
            )
        });
        put_property(&mut root, &mut defs, rule.json, moved);
    }

    let props = object_at(&mut root, "properties");
    sort_map(props);
    add_patch_arrays(props);
    props.insert(
        xai_grok_config::format::SCHEMA_KEY.into(),
        json!({ "description": "The schema of this file.", "type": "string" }),
    );
    sort_map(props);

    let mut out = Map::new();
    out.insert("$schema".into(), json!(DIALECT));
    out.insert("title".into(), json!("grok config"));
    out.insert(
        "description".into(),
        json!("The grok config file, `$GROK_HOME/config.json`."),
    );
    out.insert("type".into(), json!("object"));
    out.insert("properties".into(), Value::Object(props.clone()));
    out.insert("additionalProperties".into(), json!(false));
    let mut doc = Value::Object(out);
    prune_defs(&doc, &mut defs);
    sort_map(&mut defs);
    if !defs.is_empty()
        && let Value::Object(out) = &mut doc
    {
        out.insert("$defs".into(), Value::Object(defs));
    }
    doc
}

/// [`config_json_schema`] as the text of `schemas/config.schema.json`.
pub fn config_json_schema_text() -> String {
    let Value::Object(root) = config_json_schema() else {
        unreachable!("config_json_schema builds an object");
    };
    // The writer drops a root `$schema` member and writes the one it is given first
    xai_grok_config::format::to_config_json_string(&root, Some(DIALECT))
}

fn map_of(value: schemars::Schema) -> Value {
    json!({ "type": "object", "additionalProperties": value.to_value() })
}

fn string_map() -> Value {
    json!({ "type": "object", "additionalProperties": { "type": "string" } })
}

fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn hooks_schema(generator: &mut schemars::SchemaGenerator) -> Value {
    let group = generator
        .subschema_for::<xai_grok_hooks::config::MatcherGroup>()
        .to_value();
    let groups = json!({ "type": "array", "items": group });
    let mut events = Map::new();
    for event in xai_grok_hooks::event::HookEventName::ALL {
        events.insert(event.pascal_case().to_owned(), groups.clone());
    }
    json!({
        "description": "Hook commands by event name. Each event holds a list of matcher groups.",
        "type": "object",
        "properties": events,
        "additionalProperties": groups,
    })
}

/// Root keys that a raw-layer resolver reads. No `Config` field holds them.
fn raw_root_keys() -> Map<String, Value> {
    let bool_schema = |description: &str| json!({ "description": description, "type": "boolean" });
    let marketplace_source = json!({
        "description": "A marketplace source: `{\"source\": \"git\", \"url\"}`, `{\"source\": \"github\", \"repo\"}` or `{\"source\": \"local\", \"path\"}`. `ref` or `branch` names a git ref.",
        "type": "object",
        "properties": {
            "source": { "type": "string", "enum": ["git", "github", "local"] },
            "url": { "type": "string" },
            "repo": { "type": "string" },
            "path": { "type": "string" },
            "ref": { "type": "string" },
            "branch": { "type": "string" }
        }
    });
    let mcp_policy_entry = json!({
        "description": "One MCP server rule. It matches by URL, by command, by argv or by name.",
        "type": "object",
        "properties": {
            "serverUrl": { "type": "string" },
            "server_url": { "type": "string" },
            "command": { "type": "string" },
            "serverCommand": strings(),
            "server_command": strings(),
            "serverName": { "type": "string" },
            "server_name": { "type": "string" }
        }
    });
    let positive = json!({ "type": "integer", "minimum": 1 });
    let mut keys = Map::new();
    keys.insert(
        "dashboard".into(),
        json!({
            "description": "The agent dashboard.",
            "type": "object",
            "properties": {
                "enabled": { "type": "boolean" },
                "grouping": { "type": "string", "enum": ["state", "directory"] },
                "pinned": strings(),
                "reorder": strings()
            }
        }),
    );
    keys.insert(
        "mcp".into(),
        json!({
            "type": "object",
            "properties": {
                "max_output_bytes": positive,
                "startup_timeout_sec": positive
            }
        }),
    );
    keys.insert(
        "claude_compat".into(),
        json!({
            "type": "object",
            "properties": { "imported": { "type": "boolean" } }
        }),
    );
    keys.insert(
        "slash_command_tags".into(),
        json!({
            "description": "A tag for each slash command, by command name.",
            "type": "object",
            "additionalProperties": { "type": "string" }
        }),
    );
    keys.insert(
		"allowed_mcp_servers".into(),
		json!({ "description": "Admin policy: the MCP servers that may run.", "type": "array", "items": mcp_policy_entry }),
	);
    keys.insert(
		"denied_mcp_servers".into(),
		json!({ "description": "Admin policy: the MCP servers that may not run.", "type": "array", "items": mcp_policy_entry }),
	);
    keys.insert(
        "allow_managed_mcp_servers_only".into(),
        bool_schema("Admin policy: run only the MCP servers that managed config defines."),
    );
    keys.insert(
        "allow_managed_hooks_only".into(),
        bool_schema("Admin policy: run only the hooks that managed config defines."),
    );
    keys.insert(
        "enable_all_project_mcp_servers".into(),
        bool_schema("Admin policy: enable every MCP server that a project config defines."),
    );
    keys.insert(
        "plugin_auto_update".into(),
        bool_schema("Admin policy: update plugins automatically."),
    );
    keys.insert(
		"strict_known_marketplaces".into(),
		json!({ "description": "Admin policy: the only plugin marketplaces allowed.", "type": "array", "items": marketplace_source }),
	);
    keys.insert(
        "extra_known_marketplaces".into(),
        json!({
            "description": "Admin policy: plugin marketplaces to register, by name.",
            "type": "object",
            "additionalProperties": {
                "type": "object",
                "properties": {
                    "source": marketplace_source,
                    "autoUpdate": { "type": "boolean" },
                    "auto_update": { "type": "boolean" }
                }
            }
        }),
    );
    keys.insert(
		"fail_closed".into(),
		bool_schema("Requirements files only: refuse to start when the managed requirements cannot be read."),
	);
    keys
}

/// Keys inside a section that a raw-layer resolver reads. The struct of that section has no field for them.
fn add_raw_section_keys(root: &mut Map<String, Value>, defs: &mut Map<String, Value>) {
    let grove = json!({
        "description": "`true`, `false`, or a mode: `grove`, `grove-fuse`, `grove-nfs`, `nfs`, `all`, `copy`, `on`, `off`, `1`, `0`.",
        "type": ["boolean", "string"]
    });
    let raw: [(&[&str], &str, Value); 6] = [
        (
            &["grok_com_config"],
            "login_device_flow",
            json!({ "description": "Log in with the device flow.", "type": "boolean" }),
        ),
        (&["cli"], "grove", grove.clone()),
        (&["cli"], "grove_worktree", grove.clone()),
        (
            &["cli"],
            "nfs_worktree",
            json!({ "description": "Another spelling of `grove_worktree`.", "type": ["boolean", "string"] }),
        ),
        (&["toolset", "web_search"], "allowed_domains", strings()),
        (&["toolset", "web_search"], "excluded_domains", strings()),
    ];
    for (path, key, schema) in raw {
        let section = section_props(root, defs, path)
            .unwrap_or_else(|| panic!("no section {path:?} for the raw key `{key}`"));
        section.insert(key.to_owned(), schema);
    }
}

/// The `properties` of the object schema at `path`, with each `$ref` on the way copied in place.
fn section_props<'a>(
    root: &'a mut Map<String, Value>,
    defs: &Map<String, Value>,
    path: &[&str],
) -> Option<&'a mut Map<String, Value>> {
    let mut cur = root;
    for seg in path {
        let props = cur.get_mut("properties")?.as_object_mut()?;
        let node = props.get_mut(*seg)?;
        inline_ref(node, defs);
        cur = node.as_object_mut()?;
    }
    Some(object_at(cur, "properties"))
}

/// Replace a `$ref` node, or a nullable `anyOf` of one `$ref`, with a copy of the definition.
fn inline_ref(node: &mut Value, defs: &Map<String, Value>) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    let Some(name) = obj
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
        .map(str::to_owned)
    else {
        return;
    };
    let Some(Value::Object(def)) = defs.get(&name) else {
        panic!("`$ref` names a missing definition `{name}`");
    };
    obj.remove("$ref");
    for (k, v) in def {
        obj.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

fn object_at<'a>(obj: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let slot = obj
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    match slot {
        Value::Object(map) => map,
        _ => unreachable!("the slot was just made an object"),
    }
}

/// Remove the property at `path` and return it. A parent section that the removal empties goes too.
fn take_property(
    root: &mut Map<String, Value>,
    defs: &mut Map<String, Value>,
    path: &[&str],
) -> Option<Value> {
    let (last, parents) = path.split_last()?;
    let props = if parents.is_empty() {
        object_at(root, "properties")
    } else {
        section_props(root, defs, parents)?
    };
    let value = props.remove(*last)?;
    if let [first, _, ..] = path {
        let root_props = object_at(root, "properties");
        let empty = root_props
            .get(*first)
            .and_then(|s| s.get("properties"))
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty);
        if empty {
            root_props.remove(*first);
        }
    }
    Some(value)
}

/// Put `value` at `path`, making the sections on the way.
fn put_property(
    root: &mut Map<String, Value>,
    defs: &mut Map<String, Value>,
    path: &[&str],
    value: Value,
) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = root;
    for seg in parents {
        let props = object_at(cur, "properties");
        let node = props
            .entry(seg.to_string())
            .or_insert_with(|| json!({ "type": "object" }));
        inline_ref(node, defs);
        cur = node
            .as_object_mut()
            .unwrap_or_else(|| panic!("the section `{seg}` is not an object schema"));
    }
    let props = object_at(cur, "properties");
    let Some(existing) = props.get_mut(*last) else {
        props.insert(last.to_string(), value);
        return;
    };
    let mut value = value;
    inline_ref(existing, defs);
    inline_ref(&mut value, defs);
    let (Value::Object(existing), Value::Object(mut value)) = (existing, value) else {
        panic!("the JSON layout puts two keys that are not objects at {path:?}");
    };
    let moved_props = match value.remove("properties") {
        Some(Value::Object(moved)) => moved,
        _ => Map::new(),
    };
    let existing_props = object_at(existing, "properties");
    for (key, schema) in moved_props {
        if existing_props.insert(key.clone(), schema).is_some() {
            panic!("the JSON layout puts two keys at {path:?}.{key}");
        }
    }
    for (key, schema) in value {
        existing.entry(key).or_insert(schema);
    }
}

/// A config file is one layer of a deep merge over the compiled defaults, so no key of a section is required.
/// A member of `anyOf` or `oneOf` keeps its `required`: that is how the member is told apart.
fn strip_section_required(root: &mut Map<String, Value>, defs: &mut Map<String, Value>) {
    let mut variant_defs = std::collections::BTreeSet::new();
    collect_variant_refs(&Value::Object(root.clone()), &mut variant_defs);
    for def in defs.values() {
        collect_variant_refs(def, &mut variant_defs);
    }
    strip_required(root);
    for (name, def) in defs.iter_mut() {
        if !variant_defs.contains(name)
            && let Value::Object(def) = def
        {
            strip_required(def);
        }
    }
}

fn collect_variant_refs(value: &Value, out: &mut std::collections::BTreeSet<String>) {
    match value {
        Value::Object(obj) => {
            for (key, child) in obj {
                if (key == "anyOf" || key == "oneOf")
                    && let Value::Array(members) = child
                    && members.iter().filter(|m| !is_null_schema(m)).count() > 1
                {
                    for member in members {
                        if let Some(name) = member
                            .get("$ref")
                            .and_then(Value::as_str)
                            .and_then(|r| r.strip_prefix("#/$defs/"))
                        {
                            out.insert(name.to_owned());
                        }
                    }
                }
                collect_variant_refs(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_variant_refs(v, out)),
        _ => {}
    }
}

fn is_null_schema(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("null")
}

fn strip_required(obj: &mut Map<String, Value>) {
    obj.remove("required");
    for (key, child) in obj.iter_mut() {
        match (key.as_str(), child) {
            ("anyOf" | "oneOf", Value::Array(_)) => {}
            (_, Value::Object(child)) => strip_required(child),
            (_, Value::Array(items)) => {
                for item in items {
                    if let Value::Object(item) = item {
                        strip_required(item);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `version_overrides` and `campaigns` hold patches: each element may set any root key except another patch array.
fn add_patch_arrays(props: &mut Map<String, Value>) {
    let mut patch = Map::new();
    for key in props.keys() {
        if PATCH_ARRAY_KEYS.contains(&key.as_str()) {
            continue;
        }
        patch.insert(
            key.clone(),
            json!({ "$ref": format!("#/properties/{}", pointer_escape(key)) }),
        );
    }
    let element = |meta: Map<String, Value>| {
        let mut properties = meta;
        properties.extend(patch.clone());
        json!({ "type": "object", "properties": properties, "additionalProperties": false })
    };
    let mut version_meta = Map::new();
    for key in VERSION_OVERRIDE_META {
        version_meta.insert(
            (*key).into(),
            json!({ "description": "A semver version.", "type": "string" }),
        );
    }
    let mut campaign_meta = Map::new();
    campaign_meta.insert("id".into(), json!({ "type": "string" }));
    campaign_meta.insert(
        "campaign_id".into(),
        json!({ "description": "Another spelling of `id`.", "type": "string" }),
    );
    props.insert(
		"version_overrides".into(),
		json!({
			"description": "Patches that apply only to the grok versions from `minimum_version` up to `maximum_version`.",
			"type": "array",
			"items": element(version_meta)
		}),
	);
    props.insert(
        "campaigns".into(),
        json!({
            "description": "Named campaign patches. The deployment publishes these.",
            "type": "array",
            "items": element(campaign_meta)
        }),
    );
}

fn pointer_escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Drop every definition that nothing reachable from `doc` names.
fn prune_defs(doc: &Value, defs: &mut Map<String, Value>) {
    let mut used = std::collections::BTreeSet::new();
    let mut queue = Vec::new();
    collect_refs(doc, &mut queue);
    while let Some(name) = queue.pop() {
        if !used.insert(name.clone()) {
            continue;
        }
        if let Some(def) = defs.get(&name) {
            collect_refs(def, &mut queue);
        }
    }
    defs.retain(|name, _| used.contains(name));
}

fn collect_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(obj) => {
            if let Some(name) = obj
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
            {
                out.push(name.to_owned());
            }
            obj.values().for_each(|v| collect_refs(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

fn sort_map(map: &mut Map<String, Value>) {
    let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    map.extend(entries);
}

#[cfg(test)]
#[path = "config_schema_tests.rs"]
mod tests;
