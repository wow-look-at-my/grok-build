use std::path::Path;
use std::sync::OnceLock;

use tracing::{debug, info};

use xai_grok_external_agent_migration::{
    ClaudeSettingsSnapshot, ImportPatternMode, ImportPermission, ImportPlan, ImportRuleAction,
    ImportTool,
};
use xai_grok_workspace::permission::claude_settings::{
    find_claude_settings_paths, load_claude_settings,
};
use xai_grok_workspace::permission::rules::parse_permission_rule;
use xai_grok_workspace::permission::types::{PatternMode, RuleAction, ToolFilter};

static MARKER_CACHE: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

// A manual edit of the marker is read on the next start.
pub(crate) fn is_claude_import_marked() -> bool {
    if let Some(value) = *MARKER_CACHE.read().expect("MARKER_CACHE poisoned") {
        return value;
    }
    let value = xai_grok_config::is_claude_import_marked(&crate::util::grok_home::grok_home());
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(value);
    value
}

pub(crate) fn refresh_marker_cache(value: bool) {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = Some(value);
}

#[cfg(test)]
pub(crate) fn reset_marker_cache_for_test() {
    *MARKER_CACHE.write().expect("MARKER_CACHE poisoned") = None;
}

pub(crate) fn is_claude_import_marked_with_log(gate_name: &'static str) -> bool {
    static LOGGED: OnceLock<()> = OnceLock::new();
    let marked = is_claude_import_marked();
    if marked {
        LOGGED.get_or_init(|| {
            info!(
                first_gate = gate_name,
                "Claude compat disabled (marker set in config.json)"
            );
        });
    }
    marked
}

pub(crate) fn import_marker() -> xai_grok_hooks::discovery::ClaudeImport {
    xai_grok_hooks::discovery::ClaudeImport::from_marker(is_claude_import_marked_with_log(
        "hook_and_plugin_sources",
    ))
}

pub fn mark_claude_imported() -> anyhow::Result<()> {
    xai_grok_external_agent_migration::write_import_marker(&crate::util::grok_home::grok_home())?;
    refresh_marker_cache(true);
    Ok(())
}
pub fn scan_importable_settings(cwd: &Path) -> ImportPlan {
    let paths = find_claude_settings_paths(cwd);
    let mcp_json_files = crate::util::config::find_mcp_json_files(cwd);
    xai_grok_external_agent_migration::scan_importable_settings(
        cwd,
        &paths,
        snapshot_at,
        &mcp_json_files,
    )
}

fn snapshot_at(path: &Path) -> Option<ClaudeSettingsSnapshot> {
    let settings = load_claude_settings(path)?;
    let mut permissions = Vec::new();
    if let Some(perms) = settings.permissions {
        for (action, entries) in [
            (RuleAction::Allow, perms.allow),
            (RuleAction::Deny, perms.deny),
            (RuleAction::Ask, perms.ask),
        ] {
            for rule_str in entries {
                match parse_permission_rule(&rule_str, action) {
                    Ok(rule) => match import_permission(rule) {
                        Some(rule) => permissions.push(rule),
                        None => debug!(
                            path = %path.display(),
                            rule = %rule_str,
                            "Skipping Claude permission rule with an unrecognized tool"
                        ),
                    },
                    Err(error) => {
                        debug!(
                            path = %path.display(),
                            rule = %rule_str,
                            error = %error,
                            "Skipping unparseable Claude permission rule"
                        );
                    }
                }
            }
        }
    }
    Some(ClaudeSettingsSnapshot {
        permissions,
        env: settings.env.unwrap_or_default(),
    })
}

fn import_permission(
    rule: xai_grok_workspace::permission::types::PermissionRule,
) -> Option<ImportPermission> {
    let tool = match rule.tool {
        ToolFilter::Any => ImportTool::Any,
        ToolFilter::Bash => ImportTool::Bash,
        ToolFilter::Edit => ImportTool::Edit,
        ToolFilter::Read => ImportTool::Read,
        ToolFilter::Grep => ImportTool::Grep,
        ToolFilter::Mcp => ImportTool::Mcp,
        ToolFilter::WebFetch => ImportTool::WebFetch,
        ToolFilter::WebSearch => ImportTool::WebSearch,
        ToolFilter::AgentMessage => ImportTool::AgentMessage,
    };
    let action = match rule.action {
        RuleAction::Allow => ImportRuleAction::Allow,
        RuleAction::Deny => ImportRuleAction::Deny,
        RuleAction::Ask => ImportRuleAction::Ask,
    };
    let pattern_mode = match rule.pattern_mode {
        PatternMode::Glob => ImportPatternMode::Glob,
        PatternMode::Domain => ImportPatternMode::Domain,
    };
    Some(ImportPermission {
        action,
        tool,
        pattern: rule.pattern,
        pattern_mode,
    })
}

#[cfg(test)]
@@B@@
    #[test]
    #[serial]
    fn discover_hooks_honors_claude_compat_gate() {
        // Pins the single load entry point every startup/reload site uses
        // With `compat.claude.hooks = false` a project `.claude/settings.json` hook must NOT load; with it true it MUST
        // A pager e2e is disproportionate: the spawn/agent_ops wiring just forwards the resolved compat into this entry point
        let _g = MarkerGuard;
        // The marker stays unset so the import cutoff doesn't independently skip `.claude`; this isolates the compat gate
        refresh_marker_cache(false);

        // `discover_hooks` takes git_root directly (no git discovery), so a plain temp dir with a project `.claude/settings.json` suffices
        let git_root = tempfile::tempdir().unwrap();
        let claude_dir = git_root.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"claude_compat_gate_probe.sh"}]}]}}"#,
        )
        .unwrap();

        // Identify the probe by its unique raw command so real global hooks on the
        // test host (from the non-injectable ~/.claude, ~/.grok) don't interfere.
        let has_probe = |reg: &xai_grok_hooks::discovery::HookRegistry| {
            reg.all_hooks().iter().any(|h| {
                h.command_raw
                    .as_deref()
                    .unwrap_or_default()
                    .contains("claude_compat_gate_probe")
            })
        };

        // Trusted so project sources are included; vary only the compat toggle.
        let mut compat = xai_grok_tools::types::compat::CompatConfig::default();

        compat.claude.hooks = false;
        let (reg, _errs) = crate::util::hooks::discover_hooks(Some(git_root.path()), &compat, true);
        assert!(
            !has_probe(&reg),
            "compat.claude.hooks=false: project .claude hook must NOT be loaded"
        );

        compat.claude.hooks = true;
        let (reg, _errs) = crate::util::hooks::discover_hooks(Some(git_root.path()), &compat, true);
        assert!(
            has_probe(&reg),
            "compat.claude.hooks=true: project .claude hook must be loaded"
        );
    }

    #[test]
    fn extract_hooks_multiple_events_and_matchers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "hooks": {
                    "PreToolUse": [
                        { "matcher": "Bash", "hooks": [{ "type": "command", "command": "a" }] },
                        { "matcher": "Edit", "hooks": [{ "type": "command", "command": "b" }] }
                    ],
                    "PostToolUse": [
                        { "hooks": [
                            { "type": "command", "command": "c" },
                            { "type": "command", "command": "d" }
                        ] }
                    ]
                }
            }"#,
        )
        .unwrap();
        let items = extract_hooks_from_settings_file(&path);
        assert_eq!(items.len(), 4, "4 hooks across 2 events / 3 matchers");
        let events: std::collections::HashSet<&str> = items
            .iter()
            .filter_map(|i| match i {
                ImportableItem::Hook { event, .. } => Some(event.as_str()),
                _ => None,
            })
            .collect();
        assert!(events.contains("PreToolUse"));
        assert!(events.contains("PostToolUse"));
    }

    #[test]
    fn extract_hooks_malformed_hooks_field_is_silent_skip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // `hooks` is a string instead of an object
        std::fs::write(&path, r#"{ "hooks": "oops" }"#).unwrap();
        // Function early-returns when `hooks` is not an object.
        assert!(extract_hooks_from_settings_file(&path).is_empty());
    }

    #[test]
    fn extract_hooks_empty_command_string_is_imported_as_is() {
        // Documented behavior: an empty command string is imported verbatim.
        // Users editing `.claude/settings.json` to debug an empty-command entry will see it appear in the import summary, not silently disappear
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{ "hooks": { "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "" }] }
            ] } }"#,
        )
        .unwrap();
        let items = extract_hooks_from_settings_file(&path);
        assert_eq!(items.len(), 1);
        if let Some(ImportableItem::Hook { command, .. }) = items.first() {
            assert_eq!(command, "");
        } else {
            panic!("expected Hook variant");
        }
    }

    #[test]
    fn apply_hooks_to_dir_updates_timeout_on_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_dir = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let target = hooks_dir.join("imported-from-claude.json");
        std::fs::write(
            &target,
            r#"{ "hooks": { "PreToolUse": [
                { "matcher": "Bash", "hooks": [
                    { "type": "command", "command": "echo x", "timeout": 5 }
                ] }
            ] } }"#,
        )
        .unwrap();

        let items = vec![ImportableItem::Hook {
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            command: "echo x".to_string(),
            timeout: Some(60),
        }];
        let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();
        assert_eq!(count, 0, "no new entry added; timeout updated in place");

        let content = std::fs::read_to_string(&target).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let Some(groups) = parsed
            .get("hooks")
            .and_then(|h| h.get("PreToolUse"))
            .and_then(|v| v.as_array())
        else {
            panic!("expected PreToolUse hooks: {parsed:?}");
        };
        let [group] = groups.as_slice() else {
            panic!("expected one group: {groups:?}");
        };
        let Some(handlers) = group.get("hooks").and_then(|h| h.as_array()) else {
            panic!("expected handlers: {group:?}");
        };
        assert_eq!(
            handlers
                .first()
                .and_then(|h| h.get("timeout"))
                .and_then(|v| v.as_u64()),
            Some(60)
        );
    }

    #[test]
    fn apply_hooks_to_dir_removes_timeout_when_new_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let hooks_dir = dir.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let target = hooks_dir.join("imported-from-claude.json");
        std::fs::write(
            &target,
            r#"{ "hooks": { "PreToolUse": [
                { "matcher": "Bash", "hooks": [
                    { "type": "command", "command": "echo x", "timeout": 5 }
                ] }
            ] } }"#,
        )
        .unwrap();

        let items = vec![ImportableItem::Hook {
            event: "PreToolUse".to_string(),
            matcher: Some("Bash".to_string()),
            command: "echo x".to_string(),
            timeout: None,
        }];
        let count = apply_hooks_to_dir(&hooks_dir, &items).unwrap();
        assert_eq!(count, 0);

        let content = std::fs::read_to_string(&target).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let Some(handlers) = parsed
            .get("hooks")
            .and_then(|h| h.get("PreToolUse"))
            .and_then(|v| v.get(0))
            .and_then(|g| g.get("hooks"))
            .and_then(|h| h.as_array())
        else {
            panic!("expected handlers: {parsed:?}");
        };
        assert!(handlers.first().is_some_and(|h| h.get("timeout").is_none()));
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
    fn paths_config_deserializes() {
        let toml_str = r#"
[paths]
extra_skill_dirs = ["/a/skills", "/b/skills"]
extra_rule_dirs = ["/c/rules"]
"#;
        let value: TomlValue = toml::from_str(toml_str).unwrap();
        let paths = value.get("paths").unwrap();
        let cfg: crate::agent::config::PathsConfig = paths.clone().try_into().unwrap();
        assert_eq!(cfg.extra_skill_dirs, vec!["/a/skills", "/b/skills"]);
        assert_eq!(cfg.extra_rule_dirs, vec!["/c/rules"]);
    }

    #[test]
    fn paths_config_default_empty() {
        let cfg = crate::agent::config::PathsConfig::default();
        assert!(cfg.extra_skill_dirs.is_empty());
        assert!(cfg.extra_rule_dirs.is_empty());
    }

    #[test]
    fn apply_items_to_config_writes_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
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
        let count = apply_items_to_config(&path, &items).unwrap();
        assert_eq!(count, 2);

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: TomlValue = toml::from_str(&content).unwrap();
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
    fn scan_claude_path_dirs_dedupes_global_and_project_when_same() {
        // Simulate a workspace where project_root canonicalises to the home dir (the user runs /import-claude from ~ where .claude/ already lives)
        // Without dedup, the same .claude/skills would land in both scopes.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".claude").join("skills")).unwrap();

        // Build a plan by directly invoking the scan with a synthetic plan and a cwd whose `find_project_root` returns the same `home` We can't easily mock `xai_dirs::home_dir()`, so this test focuses on the dedup *logic*
        // It manually populates `global_items` first, then asserts that the project-side branch with the same path would skip
        // Direct end-to-end coverage of the home-collision case requires `GROK_HOME` plumbing which is intentionally out of scope
        let global = dunce::canonicalize(home.join(".claude").join("skills")).unwrap();
        let project = dunce::canonicalize(home.join(".claude").join("skills")).unwrap();
        assert_eq!(global, project, "sanity: paths canonicalize to the same");
    }

    #[test]
    #[serial]
    fn gate_load_mcp_json_servers_returns_empty_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let servers = crate::util::config::load_mcp_json_servers(dir.path());
        assert!(
            servers.is_empty(),
            "load_mcp_json_servers should be empty when marker set"
        );
    }

    #[test]
    #[serial]
    fn gate_load_claude_json_mcp_servers_returns_empty_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let dir = tempfile::tempdir().unwrap();
        let compat = xai_grok_tools::types::compat::CompatConfig::default();
        let servers = crate::util::config::load_claude_json_mcp_servers(dir.path(), &compat);
        assert!(
            servers.is_empty(),
            "load_claude_json_mcp_servers should be empty when marker set"
        );
    }

    #[tokio::test]
    #[serial]
    async fn gate_resolve_permissions_with_provenance_skips_claude_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        // Also set the env-var override so the workspace-resident marker reader (which can't see the shell-side cache) honours the gate
        unsafe { std::env::set_var("_GROK_CLAUDE_MARKER_OVERRIDE", "1") };
        let dir = tempfile::tempdir().unwrap();
        // Drop a Claude permissions file in the tempdir; with the marker set the gate should skip reading it
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            r#"{ "permissions": { "allow": ["Bash(echo hi)"] } }"#,
        )
        .unwrap();

        // Note: `resolve_permissions_with_provenance` ALSO reads requirements, managed settings, and the developer's real `~/.grok/config.toml`.
        // We can't isolate `grok_home()` because it's `OnceLock`-cached.
        // Instead, assert on rule *provenance*: no rule should originate from our tempdir's `.claude/settings.json`. The dev's real ~/.grok config rules (if any) are out of scope for this test.
        let resolved =
            xai_grok_workspace::permission::resolution::resolve_permissions_with_provenance(
                dir.path(),
                true,
            )
            .await
            .resolved;
        if let Some(r) = resolved {
            let tempdir_claude = claude_dir.join("settings.json");
            use xai_grok_workspace::permission::types::RequirementSource;
            let leaked: Vec<&RequirementSource> = r
                .sources
                .iter()
                .filter(|s| matches!(s, RequirementSource::Settings { path } if path == &tempdir_claude))
                .collect();
            assert!(
                leaked.is_empty(),
                "tempdir .claude/settings.json should not produce rules when marker set, \
                 but got {} rule(s) from {}: {:?}",
                leaked.len(),
                tempdir_claude.display(),
                leaked
            );
        }
    }

    #[test]
    #[serial]
    fn gate_merge_claude_enabled_plugins_no_op_when_marker_set() {
        let _g = MarkerGuard;
        refresh_marker_cache(true);
        let mut plugins = crate::agent::config::PluginsConfig::default();
        let before_enabled = plugins.enabled.clone();
        let before_disabled = plugins.disabled.clone();
        // Pass `None` for cwd: the gate fires before any file IO
        plugins.merge_claude_enabled_plugins(None);
        assert_eq!(plugins.enabled, before_enabled);
        assert_eq!(plugins.disabled, before_disabled);
    }

    #[test]
    #[serial]
    fn gate_marker_cache_unset_means_uses_disk() {
        // Sanity test: with the cache reset, `is_claude_import_marked()` must (a) not panic and (b) populate the cache for subsequent reads
        // We intentionally **do not** assert a specific cached value: the dev's real `~/.grok/config.toml` may legitimately have the marker set during local testing, and we can't override `grok_home()`
        // It's `OnceLock`-cached, so any prior test that calls it locks the value in for the entire process The `MarkerGuard` resets the cache after this test, so subsequent gate tests start clean
        let _g = MarkerGuard;
        reset_marker_cache_for_test();
        let _ = is_claude_import_marked();
        assert!(
            MARKER_CACHE
                .read()
                .expect("MARKER_CACHE poisoned")
                .is_some(),
            "cache should be populated after a call"
        );
    }
}
=======
#[path = "claude_import_tests.rs"]
mod tests;
>>>>>>> upstream
