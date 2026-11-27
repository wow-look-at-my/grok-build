// claude_import.rs holds is_claude_import_marked.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeImport {
    Imported,
    NotImported,
}

impl ClaudeImport {
    #[must_use]
    pub fn from_marker(is_marked: bool) -> ClaudeImport {
        if is_marked {
            ClaudeImport::Imported
        } else {
            ClaudeImport::NotImported
        }
    }
}

/// Reads `claude_compat.imported` from the user config in `grok_home`: `config.json`, or a legacy `config.toml` when only that exists.
/// A missing file, a missing key, or a file that does not parse counts as not imported.
pub fn is_claude_import_marked(grok_home: &Path) -> bool {
    let path = crate::format::resolve_config_file(grok_home, crate::CONFIG_STEM);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(config) = crate::format::parse_config_source(&path, &contents) else {
        return false;
    };
    config
        .get("claude_compat")
        .and_then(|section| section.get("imported"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "claude_import_tests.rs"]
mod tests;
