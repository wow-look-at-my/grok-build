use super::{
    apply_import, apply_items_to_config, merge_env_vars, merge_paths, merge_permissions,
    write_import_marker,
};
use crate::{
    ImportPatternMode, ImportPermission, ImportPlan, ImportRuleAction, ImportScope, ImportTool,
    ImportableItem, PathKind,
};
use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use xai_grok_config::{USER_CONFIG_FILENAME, is_claude_import_marked};

#[test]
fn merge_permissions_dedup() {
    let mut table = TomlMap::new();
    let mut perm = TomlMap::new();
    perm.insert(
        "allow".to_string(),
        TomlValue::Array(vec![TomlValue::String("Bash(npm test)".to_string())]),
    );
    table.insert("permission".to_string(), TomlValue::Table(perm));

    let rule_existing = ImportPermission {
        action: ImportRuleAction::Allow,
        tool: ImportTool::Bash,
        pattern: Some("npm test".to_string()),
        pattern_mode: ImportPatternMode::Glob,
    };
    let rule_new = ImportPermission {
        action: ImportRuleAction::Allow,
        tool: ImportTool::Bash,
        pattern: Some("npm run build".to_string()),
        pattern_mode: ImportPatternMode::Glob,
    };

    let count = merge_permissions(&mut table, &[&rule_existing, &rule_new]).unwrap();
    assert_eq!(count, 1);

    let Some(arr) = table
        .get("permission")
        .and_then(|p| p.get("allow"))
        .and_then(|a| a.as_array())
    else {
        panic!("expected permission.allow: {table:?}");
    };
    let [a0, a1] = arr.as_slice() else {
        panic!("expected two allow rules: {arr:?}");
    };
    assert_eq!(a0.as_str(), Some("Bash(npm test)"));
    assert_eq!(a1.as_str(), Some("Bash(npm run build)"));
}

#[test]
fn merge_env_vars_no_overwrite() {
    let mut table = TomlMap::new();
    let mut env = TomlMap::new();
    env.insert(
        "EXISTING".to_string(),
        TomlValue::String("old_value".to_string()),
    );
    table.insert("env".to_string(), TomlValue::Table(env));

    let count = merge_env_vars(
        &mut table,
        &[("EXISTING", "new_value"), ("NEW_VAR", "value")],
    );
    assert_eq!(count, 1);

    let Some(env_table) = table.get("env").and_then(|v| v.as_table()) else {
        panic!("expected env table: {table:?}");
    };
    assert_eq!(
        env_table.get("EXISTING").and_then(|v| v.as_str()),
        Some("old_value"),
        "existing key should NOT be overwritten"
    );
    assert_eq!(
        env_table.get("NEW_VAR").and_then(|v| v.as_str()),
        Some("value")
    );
}

#[test]
fn merge_env_vars_creates_section() {
    let mut table = TomlMap::new();
    let count = merge_env_vars(&mut table, &[("FOO", "bar")]);
    assert_eq!(count, 1);
    assert_eq!(
        table
            .get("env")
            .and_then(|e| e.get("FOO"))
            .and_then(|v| v.as_str()),
        Some("bar")
    );
}

#[test]
fn write_import_marker_creates_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let grok_home = dir.path().join("sub");
    write_import_marker(&grok_home).unwrap();
    assert!(is_claude_import_marked(&grok_home));
}

#[test]
fn write_import_marker_preserves_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(USER_CONFIG_FILENAME);
    std::fs::write(
        &path,
        r#"{"other": {"key": "value"}, "mcp": {"servers": {"foo": {"command": "x"}}}}"#,
    )
    .unwrap();

    write_import_marker(dir.path()).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed
            .get("other")
            .and_then(|o| o.get("key"))
            .and_then(|v| v.as_str()),
        Some("value")
    );
    assert_eq!(
        parsed
            .get("mcp")
            .and_then(|m| m.get("servers"))
            .and_then(|m| m.get("foo"))
            .and_then(|f| f.get("command"))
            .and_then(|v| v.as_str()),
        Some("x")
    );
    assert_eq!(
        parsed
            .get("claude_compat")
            .and_then(|c| c.get("imported"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn write_import_marker_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    write_import_marker(dir.path()).unwrap();
    write_import_marker(dir.path()).unwrap();
    assert!(is_claude_import_marked(dir.path()));
}

#[test]
fn merge_paths_creates_section() {
    let mut table = TomlMap::new();
    let count = merge_paths(&mut table, "extra_skill_dirs", &["/a", "/b"]).unwrap();
    assert_eq!(count, 2);

    let Some(arr) = table
        .get("paths")
        .and_then(|p| p.get("extra_skill_dirs"))
        .and_then(|a| a.as_array())
    else {
        panic!("expected extra_skill_dirs: {table:?}");
    };
    let [a0, a1] = arr.as_slice() else {
        panic!("expected two dirs: {arr:?}");
    };
    assert_eq!(a0.as_str(), Some("/a"));
    assert_eq!(a1.as_str(), Some("/b"));
}

#[test]
fn merge_paths_dedup_existing() {
    let mut table = TomlMap::new();
    let mut paths = TomlMap::new();
    paths.insert(
        "extra_skill_dirs".into(),
        TomlValue::Array(vec![TomlValue::String("/existing".into())]),
    );
    table.insert("paths".into(), TomlValue::Table(paths));

    let count = merge_paths(&mut table, "extra_skill_dirs", &["/existing", "/new"]).unwrap();
    assert_eq!(count, 1, "existing entry should be deduped");

    let Some(arr) = table
        .get("paths")
        .and_then(|p| p.get("extra_skill_dirs"))
        .and_then(|a| a.as_array())
    else {
        panic!("expected extra_skill_dirs: {table:?}");
    };
    assert_eq!(arr.len(), 2);
}

#[test]
fn apply_items_to_config_writes_path_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(USER_CONFIG_FILENAME);
    let items = vec![
        ImportableItem::PathEntry {
            kind: PathKind::Skill,
            path: "/foo/skills".into(),
        },
        ImportableItem::PathEntry {
            kind: PathKind::Rule,
            path: "/bar/rules".into(),
        },
    ];

    let count = apply_items_to_config(&path, &items, ImportScope::Global).unwrap();
    assert_eq!(count, 2);

    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let parsed = parsed.get("extensions").unwrap();
    assert_eq!(
        parsed
            .get("paths")
            .and_then(|p| p.get("extra_skill_dirs"))
            .and_then(|a| a.get(0))
            .and_then(|v| v.as_str()),
        Some("/foo/skills")
    );
    assert_eq!(
        parsed
            .get("paths")
            .and_then(|p| p.get("extra_rule_dirs"))
            .and_then(|a| a.get(0))
            .and_then(|v| v.as_str()),
        Some("/bar/rules")
    );
}

#[test]
fn apply_items_to_config_refuses_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(USER_CONFIG_FILENAME);
    let original = "{\"ui\": {\"theme\": \"dark\"},}";
    std::fs::write(&path, original).unwrap();
    let items = vec![ImportableItem::EnvVar {
        key: "FOO".into(),
        value: "bar".into(),
    }];

    let error = apply_items_to_config(&path, &items, ImportScope::Global).unwrap_err();
    assert!(
        matches!(error, crate::MigrationError::InvalidConfig { .. }),
        "{error}"
    );
    assert_eq!(original, std::fs::read_to_string(&path).unwrap());
}

#[test]
fn project_import_converts_legacy_toml_config() {
    let tmp = tempfile::tempdir().unwrap();
    xai_test_utils::git::init_git_repo(tmp.path());
    let grok_dir = tmp.path().join(".grok");
    std::fs::create_dir_all(&grok_dir).unwrap();
    std::fs::write(
        grok_dir.join("config.toml"),
        "[paths]\nextra_rule_dirs = [\"/keep\"]\n",
    )
    .unwrap();

    let plan = ImportPlan {
        project_items: vec![ImportableItem::PathEntry {
            kind: PathKind::Skill,
            path: "/foo/skills".into(),
        }],
        ..ImportPlan::default()
    };
    let result = apply_import(&plan, tmp.path()).unwrap();
    assert_eq!(result.project_count, 1);

    let content = std::fs::read_to_string(grok_dir.join("config.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let paths = parsed
        .get("extensions")
        .and_then(|e| e.get("paths"))
        .unwrap();
    assert_eq!(
        paths
            .get("extra_rule_dirs")
            .and_then(|a| a.get(0))
            .and_then(|v| v.as_str()),
        Some("/keep")
    );
    assert_eq!(
        paths
            .get("extra_skill_dirs")
            .and_then(|a| a.get(0))
            .and_then(|v| v.as_str()),
        Some("/foo/skills")
    );
}

#[cfg(unix)]
#[test]
fn project_import_replaces_config_symlink_not_referent() {
    let tmp = tempfile::tempdir().unwrap();
    xai_test_utils::git::init_git_repo(tmp.path());
    let outside = tmp.path().join("outside.json");
    let outside_body = r#"{"extensions": {"paths": {"extra_rule_dirs": ["/keep"]}}}"#;
    std::fs::write(&outside, outside_body).unwrap();
    let project_cfg = tmp.path().join(".grok").join("config.json");
    std::fs::create_dir_all(project_cfg.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &project_cfg).unwrap();

    let plan = ImportPlan {
        project_items: vec![ImportableItem::PathEntry {
            kind: PathKind::Skill,
            path: "/foo/skills".into(),
        }],
        ..ImportPlan::default()
    };

    let result = apply_import(&plan, tmp.path()).unwrap();
    assert_eq!(result.project_count, 1);

    assert!(
        !std::fs::symlink_metadata(&project_cfg)
            .unwrap()
            .file_type()
            .is_symlink(),
        "project slot must become a regular file"
    );
    let body = std::fs::read_to_string(&project_cfg).unwrap();
    assert!(body.contains("/foo/skills"), "{body}");
    assert!(body.contains("/keep"), "{body}");
    assert_eq!(
        outside_body,
        std::fs::read_to_string(&outside).unwrap(),
        "external referent must stay untouched"
    );
}

#[cfg(unix)]
#[test]
fn global_import_writes_through_user_config_symlink() {
    let home = tempfile::tempdir().unwrap();
    let outside = home.path().join("dotfiles").join(USER_CONFIG_FILENAME);
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, "").unwrap();
    let slot = home.path().join(USER_CONFIG_FILENAME);
    std::os::unix::fs::symlink(&outside, &slot).unwrap();

    let items = vec![ImportableItem::PathEntry {
        kind: PathKind::Skill,
        path: "/foo/skills".into(),
    }];

    let count = apply_items_to_config(&slot, &items, ImportScope::Global).unwrap();
    assert_eq!(count, 1);

    assert!(
        std::fs::symlink_metadata(&slot)
            .unwrap()
            .file_type()
            .is_symlink(),
        "user slot must stay a symlink"
    );
    let body = std::fs::read_to_string(&outside).unwrap();
    assert!(body.contains("/foo/skills"), "{body}");
}
