pub mod config;
// Extracted to the `xai-grok-login` crate.
pub use xai_grok_login::grok_auth_credentials;
pub mod hooks;
pub mod limits;
pub(crate) mod text_sanitize;
pub(crate) mod user_identity;

// The foundation utilities live in `xai-grok-shell-base` (upstream of this crate so they build in parallel) Re-exported at the paths.
pub use xai_grok_shell_base::util::*;

/// Parse an env var as a JSON object. Returns `None` if unset or not a valid JSON object.
pub(crate) fn parse_json_object_env(var: &str) -> Option<serde_json::Value> {
    let val = std::env::var(var).ok()?;
    match serde_json::from_str::<serde_json::Value>(&val) {
        Ok(v) if v.is_object() => Some(v),
        Ok(_) => {
            tracing::warn!("{var} is not a JSON object, ignoring");
            None
        }
        Err(e) => {
            tracing::warn!("{var} is invalid JSON: {e}");
            None
        }
    }
}

pub(crate) fn is_user_instruction_path(
    path: &std::path::Path,
    grok_home: &std::path::Path,
    vendor_homes: &[(std::path::PathBuf, bool)],
    workspace_roots: &[&std::path::Path],
) -> bool {
    let parent = path.parent();
    let grok_rules = grok_home.join("rules");
    let is_exact_home_surface = parent
        .is_some_and(|parent| parent == grok_home || parent == grok_rules)
        || vendor_homes.iter().any(|(vendor_home, named_enabled)| {
            parent.is_some_and(|parent| {
                (*named_enabled && parent == vendor_home) || parent == vendor_home.join("rules")
            })
        });
    if is_exact_home_surface {
        return true;
    }
    // Both prefixes are workspace because forks mix display-rewritten and on-disk paths.
    if workspace_roots.iter().any(|root| path.starts_with(root)) {
        return false;
    }
    path.starts_with(grok_home)
        || vendor_homes
            .iter()
            .any(|(vendor_home, _)| path.starts_with(vendor_home))
}

/// Ties a spawned helper task's lifetime to an async scope by aborting it on drop.
pub(crate) struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod is_user_instruction_path_tests {
    use super::is_user_instruction_path;
    use std::path::Path;

    #[test]
    fn grok_home_named_file_nested_in_workspace_is_user_scoped() {
        assert!(is_user_instruction_path(
            Path::new("/repo/config/AGENTS.md"),
            Path::new("/repo/config"),
            &[],
            &[Path::new("/repo")],
        ));
        assert!(!is_user_instruction_path(
            Path::new("/repo/config/src/AGENTS.md"),
            Path::new("/repo/config"),
            &[],
            &[Path::new("/repo")],
        ));
    }

    #[test]
    fn workspace_descendants_under_grok_home_stay_project_scoped() {
        assert!(!is_user_instruction_path(
            Path::new("/custom/grok/worktrees/repo/src/AGENTS.md"),
            Path::new("/custom/grok"),
            &[],
            &[Path::new("/custom/grok/worktrees/repo")],
        ));
    }
}
