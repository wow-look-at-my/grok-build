//! Load the JSON grok config as the internal `toml::Value` tree for edits.

use std::path::Path;

/// Read the config file at `path` as the internal tree. An absent or blank file is an empty table.
#[must_use]
pub(crate) fn read_config_tree_for_edit(path: &Path) -> Option<toml::Value> {
    match xai_grok_config::read_config_tree(path) {
        Ok(tree) => Some(tree),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "config is not valid; refusing to overwrite"
            );
            None
        }
    }
}

/// Set `hints.<key>` to `value` in the user config, preserving every other key. No-ops when the
/// existing file is non-blank but unparseable, so a malformed config is never clobbered.
pub(crate) fn set_hint(key: &str, value: impl Into<toml::Value>) -> std::io::Result<()> {
    let home = xai_grok_tools::util::grok_home::grok_home();
    let path = xai_grok_config::user_config_file(&home)?;
    set_hint_at(&path, key, value)
}

/// Core of [`set_hint`]; takes the path so tests can point it at a temp dir.
fn set_hint_at(path: &Path, key: &str, value: impl Into<toml::Value>) -> std::io::Result<()> {
    let value = value.into();
    let edited = xai_grok_config::edit_config_tree(
        path,
        xai_grok_config::format::schema_ref_for(path),
        |tree| {
            let Some(root) = tree.as_table_mut() else {
                return;
            };
            let hints = root
                .entry("hints")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if !hints.is_table() {
                *hints = toml::Value::Table(toml::map::Map::new());
            }
            if let Some(table) = hints.as_table_mut() {
                table.insert(key.to_string(), value);
            }
        },
    );
    match edited {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            tracing::warn!(path = %path.display(), error = %e, "config is not valid; hint not saved");
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn config_path(dir: &Path) -> std::path::PathBuf {
        dir.join(xai_grok_config::USER_CONFIG_FILENAME)
    }

    #[test]
    fn merge_round_trip_preserves_sibling_tables() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        fs::write(
            &path,
            r#"{"ui": {"compact_mode": false}, "mcp": {"servers": {"x": {"command": "y"}}}}"#,
        )
        .unwrap();

        set_hint_at(&path, "memory_modal_fullscreen", true).unwrap();

        let tree = read_config_tree_for_edit(&path).expect("parse");
        assert_eq!(
            tree.get("mcp_servers")
                .and_then(|m| m.get("x"))
                .and_then(|x| x.get("command"))
                .and_then(|v| v.as_str()),
            Some("y")
        );
        assert_eq!(
            tree.get("ui")
                .and_then(|u| u.get("compact_mode"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn nonempty_unparseable_returns_none_and_leaves_file() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        let bad = "this is {not valid json\n";
        fs::write(&path, bad).unwrap();

        assert!(read_config_tree_for_edit(&path).is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }

    #[test]
    fn missing_file_is_editable_empty_tree() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("absent.json");
        let tree = read_config_tree_for_edit(&path).expect("editable");
        assert!(tree.get("ui").is_none());
    }

    #[test]
    fn blank_file_is_editable_empty_tree() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        for blank in ["", "   \n", "\n\t  \n"] {
            fs::write(&path, blank).unwrap();
            let tree = read_config_tree_for_edit(&path)
                .unwrap_or_else(|| panic!("blank {blank:?} must be an empty tree"));
            assert!(
                tree.get("ui").is_none(),
                "blank {blank:?} must not be treated as unparseable"
            );
        }
    }

    #[test]
    fn set_hint_at_round_trips_and_preserves_siblings() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        fs::write(&path, r#"{"ui": {"compact_mode": false}}"#).unwrap();

        set_hint_at(&path, "memory_modal_fullscreen", true).unwrap();

        let tree = read_config_tree_for_edit(&path).expect("reparse");
        assert_eq!(
            tree.get("hints")
                .and_then(|h| h.get("memory_modal_fullscreen"))
                .and_then(|v| v.as_bool()),
            Some(true),
        );
        let body = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            json["ui"]["compact_mode"],
            serde_json::Value::Bool(false),
            "sibling ui key should be preserved"
        );
        assert_eq!(
            json["ui"]["hints"]["memory_modal_fullscreen"],
            serde_json::Value::Bool(true),
            "hint lands under ui.hints in the JSON layout"
        );
    }

    #[test]
    fn set_hint_at_creates_missing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        set_hint_at(&path, "memory_modal_fullscreen", true).unwrap();
        assert!(
            path.exists(),
            "missing file and parent dir should be created"
        );
    }

    #[test]
    fn set_hint_writes_string_values() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        fs::write(&path, r#"{"ui": {"theme": "dark"}}"#).unwrap();

        set_hint_at(&path, "fork_worktree_mode", "always").unwrap();

        let tree = read_config_tree_for_edit(&path).expect("reparse");
        assert_eq!(
            tree.get("hints")
                .and_then(|h| h.get("fork_worktree_mode"))
                .and_then(|v| v.as_str()),
            Some("always"),
        );
    }

    #[test]
    fn set_hint_at_leaves_unparseable_file_untouched() {
        let dir = tempdir().unwrap();
        let path = config_path(dir.path());
        let bad = "this is {not valid json\n";
        fs::write(&path, bad).unwrap();

        set_hint_at(&path, "memory_modal_fullscreen", true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }
}
