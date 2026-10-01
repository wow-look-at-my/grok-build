//! The form a tool's JSON Schema takes on the wire.

use std::borrow::Cow;

use serde_json::{Map, Value};

use super::{ConversationRequest, ToolSpec};

const COMBINATORS: [&str; 3] = ["allOf", "anyOf", "oneOf"];

/// A `$ref` chain longer than this stops the merge. A self-referencing definition otherwise recurses forever.
const MAX_REF_DEPTH: usize = 8;

/// Which form each tool schema takes on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolSchemaForm {
    /// The schema exactly as the tool published it.
    #[default]
    Native,
    /// A top-level `oneOf`/`anyOf`/`allOf` merges into one `type: object` schema.
    NoTopLevelCombinators,
}

impl ToolSchemaForm {
    /// The schema to send for `schema` in this form.
    pub fn apply(self, schema: &Value) -> Cow<'_, Value> {
        match self {
            Self::Native => Cow::Borrowed(schema),
            Self::NoTopLevelCombinators if has_top_level_combinator(schema) => {
                Cow::Owned(without_top_level_combinators(schema))
            }
            Self::NoTopLevelCombinators => Cow::Borrowed(schema),
        }
    }
}

/// Whether a provider's rejection names a top-level schema combinator.
/// Anthropic: "input_schema does not support oneOf, allOf, or anyOf at the top level".
pub fn names_top_level_schema_combinator(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    (message.contains("top level") || message.contains("top-level"))
        && ["oneof", "anyof", "allof"]
            .iter()
            .any(|k| message.contains(k))
}

pub fn has_top_level_combinator(schema: &Value) -> bool {
    schema
        .as_object()
        .is_some_and(|root| COMBINATORS.iter().any(|k| root.contains_key(*k)))
}

impl ConversationRequest {
    /// The parameters schema to send for `tool`, in this request's form.
    pub fn tool_parameters<'a>(&self, tool: &'a ToolSpec) -> Cow<'a, Value> {
        self.tool_schema_form.apply(&tool.parameters)
    }

    /// The tools whose schema the fallback form changes.
    pub fn tools_with_top_level_combinators(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|t| has_top_level_combinator(&t.parameters))
            .map(|t| t.name.as_str())
            .collect()
    }

    /// Step to the fallback schema form. `false` means the body would not
    /// change, so the caller reports the rejection.
    pub fn degrade_tool_schemas(&mut self) -> bool {
        if self.tool_schema_form == ToolSchemaForm::NoTopLevelCombinators
            || self.tools_with_top_level_combinators().is_empty()
        {
            return false;
        }
        self.tool_schema_form = ToolSchemaForm::NoTopLevelCombinators;
        true
    }
}

/// One object schema that accepts what `schema` accepts. `anyOf`/`oneOf`
/// require only the fields every branch requires.
fn without_top_level_combinators(schema: &Value) -> Value {
    let Some(root) = schema.as_object() else {
        return schema.clone();
    };
    Value::Object(merge(root, root, 0))
}

fn merge(obj: &Map<String, Value>, root: &Map<String, Value>, depth: usize) -> Map<String, Value> {
    let mut out = obj.clone();
    let mut properties = match out.remove("properties") {
        Some(Value::Object(p)) => p,
        _ => Map::new(),
    };
    let mut required = required_of(&out);
    let mut variants: Vec<Vec<String>> = Vec::new();

    for key in COMBINATORS {
        let Some(branches) = out.remove(key) else {
            continue;
        };
        let Value::Array(branches) = branches else {
            continue;
        };
        let branches: Vec<Map<String, Value>> = branches
            .iter()
            .filter_map(|b| resolve(b, root))
            .map(|b| {
                if depth < MAX_REF_DEPTH {
                    merge(&b, root, depth + 1)
                } else {
                    b
                }
            })
            .collect();
        let branch_required: Vec<Vec<String>> = branches.iter().map(required_of).collect();
        for branch in &branches {
            if let Some(Value::Object(p)) = branch.get("properties") {
                for (name, prop) in p {
                    add_property(&mut properties, name, prop);
                }
            }
        }
        if key == "allOf" {
            required.extend(branch_required.into_iter().flatten());
        } else {
            if let Some((first, rest)) = branch_required.split_first() {
                required.extend(
                    first
                        .iter()
                        .filter(|f| rest.iter().all(|r| r.contains(f)))
                        .cloned(),
                );
            }
            if branch_required.len() > 1 {
                variants.extend(branch_required);
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    required.retain(|f| seen.insert(f.clone()));

    out.entry("type").or_insert_with(|| Value::from("object"));
    out.insert("properties".into(), Value::Object(properties));
    if required.is_empty() {
        out.remove("required");
    } else {
        out.insert(
            "required".into(),
            Value::Array(required.into_iter().map(Value::from).collect()),
        );
    }
    if !variants.is_empty() {
        let note = variants_note(&variants);
        let description = match out.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => format!("{d}\n\n{note}"),
            _ => note,
        };
        out.insert("description".into(), Value::from(description));
    }
    out
}

/// A branch as an object schema, following a local `$ref` into `$defs` or
/// `definitions`. A branch that is not an object contributes nothing.
fn resolve(branch: &Value, root: &Map<String, Value>) -> Option<Map<String, Value>> {
    let obj = branch.as_object()?;
    let Some(reference) = obj.get("$ref").and_then(Value::as_str) else {
        return Some(obj.clone());
    };
    let (table, name) = reference
        .strip_prefix("#/$defs/")
        .map(|n| ("$defs", n))
        .or_else(|| {
            reference
                .strip_prefix("#/definitions/")
                .map(|n| ("definitions", n))
        })?;
    root.get(table)?.get(name)?.as_object().cloned()
}

fn required_of(obj: &Map<String, Value>) -> Vec<String> {
    obj.get("required")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Branches that type one field differently leave it accepting either.
fn add_property(properties: &mut Map<String, Value>, name: &str, prop: &Value) {
    let Some(existing) = properties.get_mut(name) else {
        properties.insert(name.to_owned(), prop.clone());
        return;
    };
    if existing == prop {
        return;
    }
    let is_union = existing
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("anyOf"));
    if is_union && let Some(Value::Array(alternatives)) = existing.get_mut("anyOf") {
        if !alternatives.contains(prop) {
            alternatives.push(prop.clone());
        }
        return;
    }
    *existing = serde_json::json!({ "anyOf": [existing.clone(), prop.clone()] });
}

fn variants_note(variants: &[Vec<String>]) -> String {
    let forms: Vec<String> = variants
        .iter()
        .enumerate()
        .map(|(i, fields)| {
            let fields = if fields.is_empty() {
                "no required fields".to_owned()
            } else {
                fields
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("({}) {fields}", i + 1)
        })
        .collect();
    format!(
        "The arguments take one of these forms, by required fields: {}.",
        forms.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_sends_the_schema_untouched() {
        let schema = json!({ "oneOf": [{ "type": "object" }] });
        assert_eq!(*ToolSchemaForm::Native.apply(&schema), schema);
    }

    #[test]
    fn a_schema_without_combinators_is_unchanged_in_the_fallback() {
        let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
        assert!(matches!(
            ToolSchemaForm::NoTopLevelCombinators.apply(&schema),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn one_of_keeps_shared_required_fields_and_lists_the_variants() {
        let schema = json!({
            "description": "Look something up.",
            "oneOf": [
                { "type": "object", "properties": { "kind": { "const": "id" }, "id": { "type": "integer" } }, "required": ["kind", "id"] },
                { "type": "object", "properties": { "kind": { "const": "name" }, "name": { "type": "string" } }, "required": ["kind", "name"] }
            ]
        });
        let flat = ToolSchemaForm::NoTopLevelCombinators
            .apply(&schema)
            .into_owned();
        assert!(!has_top_level_combinator(&flat), "{flat}");
        assert_eq!(flat["type"], "object");
        assert_eq!(flat["required"], json!(["kind"]));
        assert_eq!(flat["properties"]["id"], json!({ "type": "integer" }));
        assert_eq!(flat["properties"]["name"], json!({ "type": "string" }));
        assert_eq!(
            flat["properties"]["kind"],
            json!({ "anyOf": [{ "const": "id" }, { "const": "name" }] })
        );
        let description = flat["description"].as_str().unwrap();
        assert!(
            description.starts_with("Look something up."),
            "{description}"
        );
        assert!(
            description.contains("(1) `kind`, `id`; (2) `kind`, `name`"),
            "{description}"
        );
    }

    #[test]
    fn all_of_keeps_every_required_field() {
        let schema = json!({
            "allOf": [
                { "properties": { "a": { "type": "string" } }, "required": ["a"] },
                { "properties": { "b": { "type": "string" } }, "required": ["b"] }
            ]
        });
        let flat = ToolSchemaForm::NoTopLevelCombinators
            .apply(&schema)
            .into_owned();
        assert_eq!(flat["required"], json!(["a", "b"]));
        assert!(flat.get("description").is_none());
    }

    #[test]
    fn required_only_branches_keep_the_root_properties() {
        let schema = json!({
            "type": "object",
            "properties": { "path": { "type": "string" }, "url": { "type": "string" } },
            "anyOf": [{ "required": ["path"] }, { "required": ["url"] }]
        });
        let flat = ToolSchemaForm::NoTopLevelCombinators
            .apply(&schema)
            .into_owned();
        assert!(!has_top_level_combinator(&flat));
        assert_eq!(flat["properties"]["path"], json!({ "type": "string" }));
        assert!(flat.get("required").is_none());
    }

    #[test]
    fn a_ref_branch_resolves_through_defs() {
        let schema = json!({
            "$defs": { "ById": { "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] } },
            "anyOf": [{ "$ref": "#/$defs/ById" }]
        });
        let flat = ToolSchemaForm::NoTopLevelCombinators
            .apply(&schema)
            .into_owned();
        assert_eq!(flat["properties"]["id"], json!({ "type": "integer" }));
        assert_eq!(flat["required"], json!(["id"]));
    }

    #[test]
    fn a_self_referencing_definition_terminates() {
        let schema = json!({
            "$defs": { "Loop": { "anyOf": [{ "$ref": "#/$defs/Loop" }] } },
            "anyOf": [{ "$ref": "#/$defs/Loop" }]
        });
        let flat = ToolSchemaForm::NoTopLevelCombinators
            .apply(&schema)
            .into_owned();
        assert!(!has_top_level_combinator(&flat));
    }

    #[test]
    fn the_anthropic_rejection_is_recognised() {
        assert!(names_top_level_schema_combinator(
            "invalid_request_error: tools.16.custom.input_schema: input_schema does not support oneOf, allOf, or anyOf at the top level"
        ));
        assert!(!names_top_level_schema_combinator(
            "invalid_request_error: messages.1.content.0.text: field required"
        ));
    }

    #[test]
    fn degrade_steps_once_and_only_when_a_schema_changes() {
        let plain = ToolSpec {
            name: "plain".into(),
            description: None,
            parameters: json!({ "type": "object" }),
        };
        let mut req = ConversationRequest {
            tools: vec![plain.clone()],
            ..Default::default()
        };
        assert!(!req.degrade_tool_schemas(), "nothing would change");

        req.tools.push(ToolSpec {
            name: "mcp__srv__union".into(),
            description: None,
            parameters: json!({ "anyOf": [{ "type": "object" }] }),
        });
        assert_eq!(req.tools_with_top_level_combinators(), ["mcp__srv__union"]);
        assert!(req.degrade_tool_schemas());
        assert!(!req.degrade_tool_schemas(), "the ladder ends");
        assert!(!has_top_level_combinator(
            &req.tool_parameters(&req.tools[1])
        ));
    }
}
