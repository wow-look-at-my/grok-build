use super::*;

fn tree(json: serde_json::Value) -> toml::Value {
    crate::format::json_to_tree(&json).unwrap()
}

#[test]
fn json_layout_maps_to_internal_keys() {
    let mut doc = tree(serde_json::json!({
        "models": {"default": "m", "providers": {"p": {"base_url": "https://x"}}, "definitions": {"m": {"name": "M"}}},
        "tools": {"bash": {"login_shell_capture": true}, "respect_gitignore": false},
        "mcp": {"servers": {"s": {"command": "c"}}, "max_output_bytes": 9},
        "worktrees": {"pool": {"enabled": true}, "auto_gc": {"enabled": false}},
        "agents": {"subagents": {"enabled": true}},
        "ui": {"theme": "dark", "dashboard": {"enabled": true}},
    }));
    json_to_internal(&mut doc);
    let get = |p: &[&str]| {
        let mut cur = &doc;
        for s in p {
            cur = cur
                .get(*s)
                .unwrap_or_else(|| panic!("missing {p:?} in {doc:#?}"));
        }
        cur.clone()
    };
    assert_eq!(get(&["models", "default"]).as_str(), Some("m"));
    assert!(get(&["model_providers", "p"]).is_table());
    assert!(get(&["model", "m"]).is_table());
    assert_eq!(
        get(&["toolset", "bash", "login_shell_capture"]).as_bool(),
        Some(true)
    );
    assert_eq!(get(&["tools", "respect_gitignore"]).as_bool(), Some(false));
    assert!(get(&["mcp_servers", "s"]).is_table());
    assert_eq!(get(&["mcp", "max_output_bytes"]).as_integer(), Some(9));
    assert_eq!(get(&["worktree_pool", "enabled"]).as_bool(), Some(true));
    assert!(get(&["worktree", "auto_gc"]).is_table());
    assert!(get(&["subagents"]).is_table());
    assert!(get(&["dashboard"]).is_table());
    assert_eq!(get(&["ui", "theme"]).as_str(), Some("dark"));
    assert!(
        doc.get("agents").is_none(),
        "a section that only held moved subtrees must go"
    );
    assert!(doc.get("worktrees").is_none());
}

#[test]
fn layout_round_trips_both_ways() {
    let json = tree(serde_json::json!({
        "models": {"default": "m", "groups": {"g": {"members": ["a"]}}, "pricing": {"lookup_enabled": false}},
        "login": {"preferred_method": "oauth"},
        "tools": {"web_search": {"allowed_domains": ["a.com"]}, "disable_web_search": true},
        "session": {"compaction": {"x": 1}, "load_envrc": true},
        "memory": {"enabled": true, "v2": {"enabled": false}},
        "extensions": {"skills": {"paths": ["s"]}, "plugins": {"disabled": ["p"]}},
        "privacy": {"privacy_banner_acked": "t", "consent": {"version": 2}},
        "policy": {"allowed_mcp_servers": [{"command": "c"}]},
        "features": {"x": true},
        "hooks": {"Stop": []},
    }));
    let mut internal = json.clone();
    json_to_internal(&mut internal);
    let mut back = internal.clone();
    internal_to_json(&mut back);
    assert_eq!(back, json);
    let mut again = back.clone();
    json_to_internal(&mut again);
    assert_eq!(again, internal);
}

#[test]
fn patch_arrays_are_moved_per_element() {
    let mut doc = tree(serde_json::json!({
        "version_overrides": [{"minimum_version": "1.0.0", "mcp": {"servers": {"s": {"command": "c"}}}}],
        "campaigns": [{"id": "c1", "agents": {"goal": {"enabled": true}}}],
    }));
    json_to_internal(&mut doc);
    let vo = doc.get("version_overrides").and_then(|v| v.get(0)).unwrap();
    assert!(vo.get("mcp_servers").and_then(|s| s.get("s")).is_some());
    assert_eq!(
        vo.get("minimum_version").and_then(toml::Value::as_str),
        Some("1.0.0")
    );
    let c = doc.get("campaigns").and_then(|v| v.get(0)).unwrap();
    assert!(c.get("goal").is_some());
}

#[test]
fn legacy_auth_alias_migrates_to_login() {
    let mut doc = tree(serde_json::json!({"auth": {"preferred_method": "oauth"}}));
    internal_to_json(&mut doc);
    assert_eq!(
        doc.get("login")
            .and_then(|l| l.get("preferred_method"))
            .and_then(toml::Value::as_str),
        Some("oauth")
    );
    assert!(doc.get("auth").is_none());
}

#[test]
fn internal_path_names_the_json_path() {
    assert_eq!(
        internal_path_to_json("mcp_servers.foo.command"),
        "mcp.servers.foo.command"
    );
    assert_eq!(internal_path_to_json("toolset.bash.x"), "tools.bash.x");
    assert_eq!(internal_path_to_json("ui.theme"), "ui.theme");
    assert_eq!(
        internal_path_to_json("worktree_pool.size"),
        "worktrees.pool.size"
    );
}

#[test]
fn json_layout_never_targets_another_rows_source() {
    // A JSON path must not sit under an internal path that another row moves, or a round trip could move it twice
    for a in LAYOUT {
        for b in LAYOUT {
            if std::ptr::eq(a, b) {
                continue;
            }
            assert!(
                !(a.json.starts_with(b.internal) && a.json.len() > b.internal.len()),
                "{:?} sits under the internal path {:?}",
                a.json,
                b.internal
            );
        }
    }
}
