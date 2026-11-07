use super::*;

#[test]
fn json_source_parses_to_internal_tree() {
    let src = r#"{"$schema": "./config.schema.json", "ui": {"theme": "dark", "scroll_lines": 3}, "mcp": {"servers": {"s": {"command": "c", "args": ["a"]}}}, "models": {"temperature": 0.5, "default": null}}"#;
    let v = parse_config_source(Path::new("/x/config.json"), src).unwrap();
    assert!(v.get(SCHEMA_KEY).is_none());
    assert_eq!(
        v.get("ui")
            .and_then(|u| u.get("theme"))
            .and_then(toml::Value::as_str),
        Some("dark")
    );
    assert_eq!(
        v.get("ui")
            .and_then(|u| u.get("scroll_lines"))
            .and_then(toml::Value::as_integer),
        Some(3)
    );
    assert!(v.get("mcp_servers").and_then(|s| s.get("s")).is_some());
    assert_eq!(
        v.get("models")
            .and_then(|m| m.get("temperature"))
            .and_then(toml::Value::as_float),
        Some(0.5)
    );
    assert!(
        v.get("models").and_then(|m| m.get("default")).is_none(),
        "null means unset"
    );
}

#[test]
fn toml_extension_still_parses_legacy_toml() {
    let v =
        parse_config_source(Path::new("/x/requirements.toml"), "[ui]\ntheme = \"x\"\n").unwrap();
    assert_eq!(
        v.get("ui")
            .and_then(|u| u.get("theme"))
            .and_then(toml::Value::as_str),
        Some("x")
    );
}

#[test]
fn json_errors_name_the_location_and_never_the_source() {
    let src = "{\n\"endpoints\": {\"management_api_key\": \"xai-secretmustnotleak\",}\n}";
    let msg = parse_config_source(Path::new("/x/config.json"), src)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("line 2"), "want location: {msg}");
    assert!(
        !msg.contains("xai-secretmustnotleak"),
        "leaked the value: {msg}"
    );
}

#[test]
fn json_root_must_be_an_object_and_nulls_in_arrays_fail() {
    assert!(parse_config_source(Path::new("/x/c.json"), "[1]").is_err());
    let msg = parse_config_source(
        Path::new("/x/c.json"),
        r#"{"skills": {"paths": ["a", null]}}"#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        msg.contains("extensions.skills.paths.1") || msg.contains("skills.paths.1"),
        "{msg}"
    );
}

#[test]
fn blank_json_is_an_empty_table() {
    let v = parse_config_source(Path::new("/x/c.json"), "  \n").unwrap();
    assert_eq!(v.as_table().map(|t| t.is_empty()), Some(true));
}

#[test]
fn written_json_uses_tabs_and_names_the_schema_first() {
    let tree =
        parse_config_source(Path::new("/x/c.toml"), "[mcp_servers.s]\ncommand = \"c\"\n").unwrap();
    let text = tree_to_config_json_string(&tree, Some(USER_SCHEMA_REF));
    assert!(
        text.starts_with("{\n\t\"$schema\": \"./config.schema.json\",\n\t\"mcp\": {"),
        "{text}"
    );
    assert!(text.ends_with("}\n"));
    let back = parse_config_source(Path::new("/x/c.json"), &text).unwrap();
    assert_eq!(back, tree);
}

#[test]
fn datetimes_become_strings() {
    let tree = parse_config_source(
        Path::new("/x/c.toml"),
        "[privacy]\nacked = 1979-05-27T07:32:00Z\n",
    )
    .unwrap();
    let json = tree_to_json(&tree);
    assert_eq!(json["privacy"]["acked"], "1979-05-27T07:32:00Z");
}

#[test]
fn migration_converts_renames_and_keeps_the_original() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[ui]\ntheme = \"dark\"\n[model_providers.p]\nbase_url = \"https://x\"\n",
    )
    .unwrap();
    let path = migrate_legacy_config(dir.path(), "config", Some(USER_SCHEMA_REF)).unwrap();
    assert_eq!(path, dir.path().join("config.json"));
    assert!(!dir.path().join("config.toml").exists());
    assert!(dir.path().join("config.toml.migrated").exists());
    let text = std::fs::read_to_string(&path).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["$schema"], USER_SCHEMA_REF);
    assert_eq!(doc["models"]["providers"]["p"]["base_url"], "https://x");
    assert_eq!(doc["ui"]["theme"], "dark");
    // A second call is a no-op
    assert_eq!(
        migrate_legacy_config(dir.path(), "config", None).unwrap(),
        path
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn migration_refuses_a_broken_toml_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "this = = broken").unwrap();
    assert!(migrate_legacy_config(dir.path(), "config", None).is_err());
    assert!(
        dir.path().join("config.toml").exists(),
        "a broken file stays where the user can fix it"
    );
    assert!(!dir.path().join("config.json").exists());
}

#[test]
fn json_wins_over_a_legacy_sibling() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[ui]\ntheme = \"old\"\n").unwrap();
    std::fs::write(dir.path().join("config.json"), "{}").unwrap();
    assert_eq!(
        resolve_config_file(dir.path(), "config"),
        dir.path().join("config.json")
    );
}

#[test]
fn edit_config_tree_keeps_the_json_layout_and_refuses_broken_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"$schema": "custom.json", "mcp": {"servers": {"a": {"command": "x"}}}}"#,
    )
    .unwrap();
    edit_config_tree(&path, None, |tree| {
        let servers = tree
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
            .unwrap();
        servers.insert("b".into(), toml::Value::Table(toml::map::Map::new()));
    })
    .unwrap();
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["$schema"], "custom.json");
    assert!(doc["mcp"]["servers"]["b"].is_object());
    assert!(doc.get("mcp_servers").is_none());

    std::fs::write(&path, "{ broken").unwrap();
    assert!(edit_config_tree(&path, None, |_| ()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
}
