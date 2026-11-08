//! The JSON Schema of `pager.json`, built from the Raw appearance types.

use serde_json::{Map, Value, json};

use super::config::RawAppearanceConfig;

const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The schema of `pager.json`.
pub fn pager_json_schema() -> Value {
    let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let Value::Object(mut root) = generator
        .into_root_schema_for::<RawAppearanceConfig>()
        .to_value()
    else {
        panic!("the RawAppearanceConfig schema is not an object");
    };
    let mut props = match root.remove("properties") {
        Some(Value::Object(props)) => props,
        _ => Map::new(),
    };
    props.insert(
        xai_grok_config::format::SCHEMA_KEY.into(),
        json!({ "description": "The schema of this file.", "type": "string" }),
    );
    let mut defs = match root.remove("$defs") {
        Some(Value::Object(defs)) => defs,
        _ => Map::new(),
    };
    for def in defs.values_mut() {
        close_objects(def);
    }

    let mut out = Map::new();
    out.insert("$schema".into(), json!(DIALECT));
    out.insert("title".into(), json!("grok pager appearance"));
    if let Some(description) = root.remove("description") {
        out.insert("description".into(), description);
    }
    out.insert("type".into(), json!("object"));
    out.insert("properties".into(), Value::Object(props));
    out.insert("additionalProperties".into(), json!(false));
    if !defs.is_empty() {
        out.insert("$defs".into(), Value::Object(defs));
    }
    Value::Object(out)
}

/// [`pager_json_schema`] as the text of `schemas/pager.schema.json`.
pub fn pager_json_schema_text() -> String {
    let Value::Object(root) = pager_json_schema() else {
        unreachable!("pager_json_schema builds an object");
    };
    // The writer drops a root `$schema` member and writes the one it is given first
    xai_grok_config::format::to_config_json_string(&root, Some(DIALECT))
}

/// An object schema with properties refuses other keys, so an editor flags a misspelt key.
fn close_objects(schema: &mut Value) {
    if let Value::Object(obj) = schema
        && obj.contains_key("properties")
    {
        obj.entry("additionalProperties").or_insert(json!(false));
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
