//! Cache redirection for package-runner child processes under a write-confining sandbox profile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Environment variables that relocate a package runner's cache/state onto
/// the session's writable temp storage. Each entry is the variable and the
/// subdirectory of the cache root that holds its state.
pub const CACHE_ENV_VARS: &[(&str, &str)] = &[
    ("UV_CACHE_DIR", "uv"),
    ("UV_TOOL_DIR", "uv-tools"),
    ("UV_TOOL_BIN_DIR", "uv-tools/bin"),
    ("UV_PYTHON_INSTALL_DIR", "uv-python"),
    ("npm_config_cache", "npm"),
    ("npm_config_prefix", "npm-prefix"),
    ("PNPM_STORE_DIR", "pnpm-store"),
    ("npm_config_store_dir", "pnpm-store"),
    ("BUN_INSTALL_CACHE_DIR", "bun"),
    ("BUN_INSTALL", "bun-install"),
];

/// The `(name, value)` pairs to add to a package runner's child environment
/// so its caches land on the session's writable temp storage.
pub fn cache_env(root: &Path) -> Vec<(OsString, OsString)> {
    let mut vars = Vec::with_capacity(CACHE_ENV_VARS.len());
    for (name, rel) in CACHE_ENV_VARS {
        let dir = root.join(rel);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        vars.push((OsString::from(name), dir.into_os_string()));
    }
    vars
}

/// Whether `program` names a package runner — a command that fetches the
/// server it runs from a package index and therefore needs a writable cache.
pub fn is_package_runner(program: &str) -> bool {
    // Take the final path component under BOTH separators: `Path` on Unix does not split a Windows path.
    let file = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let stem = file
        .strip_suffix(".cmd")
        .or_else(|| file.strip_suffix(".exe"))
        .or_else(|| file.strip_suffix(".CMD"))
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file);
    RUNNER_STEMS
        .iter()
        .any(|runner| stem.eq_ignore_ascii_case(runner))
}

/// Command names that resolve a package from an index at run time.
const RUNNER_STEMS: &[&str] = &[
    "uvx", "uv", "npx", "npm", "pnpm", "pnpx", "yarn", "yarnpkg", "bunx", "bun", "pipx",
];

/// The scratch root a package runner's caches are mapped onto. This is the session's writable temp storage — the same directory family [`crate::paths::temp_writable_paths`] hands a confining profile. It is writable by
/// construction and needs no new grant.
pub fn scratch_root() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::iter::once(PathBuf::from("/tmp")))
        .collect();
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// The scratch subdirectory a runner's state lives under, for diagnostics.
pub fn cache_root(scratch: &Path) -> PathBuf {
    scratch.join("grok-mcp-cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped mapping must cover every variable those runner families read.
    /// Regression guard: dropping `UV_TOOL_DIR` (or any single entry) leaves
    /// `uvx` dying on `~/.local/share/uv/tools` even with the others set.
    #[test]
    fn cache_env_covers_every_runner_state_dir() {
        let scratch = std::env::temp_dir().join(format!(
            "grok-mcp-cache-env-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        let root = cache_root(&scratch);

        let vars = cache_env(&root);
        let names: Vec<&str> = vars.iter().filter_map(|(n, _)| n.to_str()).collect();
        for expected in [
            "UV_CACHE_DIR",
            "UV_TOOL_DIR",
            "UV_TOOL_BIN_DIR",
            "UV_PYTHON_INSTALL_DIR",
            "npm_config_cache",
            "npm_config_prefix",
            "PNPM_STORE_DIR",
            "npm_config_store_dir",
            "BUN_INSTALL_CACHE_DIR",
            "BUN_INSTALL",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from cache_env: {names:?}"
            );
        }

        // Every value must be a real, existing directory under the injected
        // scratch root, so a runner can write there immediately.
        for (name, value) in &vars {
            let path = Path::new(value);
            assert!(
                path.is_dir(),
                "{} did not materialize a directory: {}",
                name.to_string_lossy(),
                path.display()
            );
            assert!(
                path.starts_with(&root),
                "{} must stay under the injected scratch root: {}",
                name.to_string_lossy(),
                path.display()
            );
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// The redirect must never land in `$HOME` or `$GROK_HOME`: caches are
    /// disposable. Mapping them into either would either widen the profile's
    /// write set or fill the session's own state directory with garbage. This is
    /// the regression guard for "map the caches onto tmpfs".
    #[test]
    fn cache_env_never_points_at_home_or_grok_home() {
        let scratch = std::env::temp_dir().join(format!(
            "grok-mcp-cache-home-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = cache_root(&scratch);
        let vars = cache_env(&root);
        assert!(!vars.is_empty(), "cache_env must produce a mapping");

        for (name, value) in &vars {
            let path = Path::new(value);
            if let Some(home) = xai_dirs::home_dir() {
                assert!(
                    !path.starts_with(&home),
                    "{} must not point into $HOME ({}): {}",
                    name.to_string_lossy(),
                    home.display(),
                    path.display()
                );
            }
            let grok_home = crate::paths::grok_home();
            assert!(
                !path.starts_with(&grok_home),
                "{} must not point into $GROK_HOME ({}): {}",
                name.to_string_lossy(),
                grok_home.display(),
                path.display()
            );
        }

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// `scratch_root` must return a directory that exists, and must prefer
    /// `TMPDIR` (which the macOS jail sets to its dedicated writable scratch
    /// dir) over the `/tmp` fallback.
    #[test]
    fn scratch_root_prefers_tmpdir_and_exists() {
        let scratch = std::env::temp_dir().join(format!(
            "grok-mcp-scratch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&scratch).unwrap();

        let found = scratch_root().expect("a scratch root must resolve");
        assert!(found.is_dir(), "resolved scratch root must exist");
        // Whatever it resolves to, it has to be writable by this process.
        let probe = found.join(format!("grok-scratch-probe-{}", std::process::id()));
        assert!(
            std::fs::create_dir_all(&probe).is_ok(),
            "resolved scratch root must be writable: {}",
            found.display()
        );
        let _ = std::fs::remove_dir(&probe);

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// A runner named by absolute path, by bare name, and by a Windows launcher
    /// suffix must all be recognized; a lookalike must not be.
    #[test]
    fn is_package_runner_matches_stems_not_substrings() {
        for yes in [
            "uvx",
            "uv",
            "/opt/homebrew/bin/uvx",
            "npx",
            "npx.cmd",
            "pnpm",
            "pnpx",
            "bunx",
            "pipx",
            r"C:\Program Files\nodejs\npx.cmd",
        ] {
            assert!(is_package_runner(yes), "{yes} must be a package runner");
        }
        for no in [
            "python3",
            "/usr/bin/node",
            "my-uvx-wrapper",
            "kagimcp",
            "server",
            "",
        ] {
            assert!(!is_package_runner(no), "{no} must not be a package runner");
        }
    }
}
