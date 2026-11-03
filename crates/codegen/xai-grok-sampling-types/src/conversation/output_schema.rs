//! The structured-output schema in the form the Anthropic Messages API accepts.

use serde_json::{Map, Value};

/// Keywords the Messages API refuses in a structured-output schema.
const UNSUPPORTED: [&str; 8] = [
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "maxItems",
];

/// Keys whose value is one subschema.
const SUBSCHEMA_KEYS: [&str; 3] = ["items", "not", "additionalProperties"];

/// Keys whose value is an array of subschemas.
const SUBSCHEMA_LIST_KEYS: [&str; 4] = ["anyOf", "oneOf", "allOf", "prefixItems"];

/// Keys whose value is a map of name to subschema.
const SUBSCHEMA_MAP_KEYS: [&str; 3] = ["properties", "$defs", "definitions"];

/// `schema` with every constraint the Messages API refuses moved into its description.
pub fn for_messages_api(schema: &Value) -> Value {
    let mut schema = schema.clone();
    sanitize(&mut schema);
    schema
}

fn sanitize(schema: &mut Value) {
    let Value::Object(obj) = schema else {
        return;
    };
    let mut notes = Vec::new();
    for key in UNSUPPORTED {
        if let Some(value) = obj.remove(key) {
            notes.push(format!("{key}: {value}"));
        }
    }
    if obj
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|n| n > 1)
        && let Some(value) = obj.remove("minItems")
    {
        notes.push(format!("minItems: {value}"));
    }
    if !notes.is_empty() {
        append_description(obj, &notes.join(", "));
    }
    if is_object_schema(obj) && !obj.contains_key("additionalProperties") {
        obj.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    for key in SUBSCHEMA_KEYS {
        if let Some(child) = obj.get_mut(key) {
            sanitize(child);
        }
    }
    for key in SUBSCHEMA_LIST_KEYS {
        if let Some(Value::Array(children)) = obj.get_mut(key) {
            children.iter_mut().for_each(sanitize);
        }
    }
    for key in SUBSCHEMA_MAP_KEYS {
        if let Some(Value::Object(children)) = obj.get_mut(key) {
            children.values_mut().for_each(sanitize);
        }
    }
}

fn is_object_schema(obj: &Map<String, Value>) -> bool {
    match obj.get("type") {
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "object"),
        _ => obj.contains_key("properties"),
    }
}

fn append_description(obj: &mut Map<String, Value>, note: &str) {
    let text = match obj.get("description").and_then(Value::as_str) {
        Some(existing) if !existing.is_empty() => format!("{existing} ({note})"),
        _ => format!("Constraint: {note}"),
    };
    obj.insert("description".to_owned(), Value::String(text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_and_number_constraints_move_into_the_description() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "statement": { "type": "string", "maxLength": 512 },
                "score": { "type": "number", "minimum": 0, "maximum": 1, "description": "Confidence" },
            }
        });
        let out = for_messages_api(&schema);
        assert_eq!(
            out["properties"]["statement"],
            json!({ "type": "string", "description": "Constraint: maxLength: 512" })
        );
        assert_eq!(
            out["properties"]["score"],
            json!({ "type": "number", "description": "Confidence (minimum: 0, maximum: 1)" })
        );
    }

    #[test]
    fn nested_objects_gain_additional_properties_false() {
        let schema = json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "a": { "type": "string" } } }
                }
            }
        });
        let out = for_messages_api(&schema);
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(
            out["properties"]["items"]["items"]["additionalProperties"],
            json!(false)
        );
    }

    #[test]
    fn an_explicit_additional_properties_is_kept() {
        let schema = json!({ "type": "object", "additionalProperties": { "type": "string" } });
        assert_eq!(for_messages_api(&schema), schema);
    }

    #[test]
    fn min_items_of_zero_or_one_is_supported_and_kept() {
        let schema = json!({ "type": "array", "minItems": 1, "items": { "type": "string" } });
        assert_eq!(for_messages_api(&schema), schema);

        let out = for_messages_api(&json!({ "type": "array", "minItems": 3, "maxItems": 5 }));
        assert_eq!(
            out,
            json!({ "type": "array", "description": "Constraint: maxItems: 5, minItems: 3" })
        );
    }

    #[test]
    fn combinator_branches_and_definitions_are_walked() {
        let schema = json!({
            "anyOf": [ { "type": "string", "minLength": 2 }, { "$ref": "#/$defs/n" } ],
            "$defs": { "n": { "type": "integer", "multipleOf": 2 } }
        });
        let out = for_messages_api(&schema);
        assert_eq!(
            out["anyOf"][0],
            json!({ "type": "string", "description": "Constraint: minLength: 2" })
        );
        assert_eq!(
            out["$defs"]["n"],
            json!({ "type": "integer", "description": "Constraint: multipleOf: 2" })
        );
    }

    #[test]
    fn a_supported_schema_is_unchanged() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["outcome"],
            "properties": { "outcome": { "type": "string", "enum": ["noop", "observations"] } }
        });
        assert_eq!(for_messages_api(&schema), schema);
    }
}
