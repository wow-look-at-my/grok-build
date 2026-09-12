//! Cache redirection for package-runner child processes under a write-confining
//! sandbox profile.
//!
//! An MCP server configured as `uvx kagimcp` or `npx -y some-server` is a
//! *package runner*: the command it names is not installed, so the runner
//! resolves it from a package index into a cache of its own. Every one of those
//! caches defaults under `$HOME` (`~/.cache/uv`, `~/.npm`, `~/.bun/install`, …)
//! or under an XDG data dir, and **no** write-confining profile grants `$HOME`:
//! `workspace`, `read-only` and `strict` write only to the workspace,
//! `$GROK_HOME` and the temp dirs (see [`crate::profiles`]).
//!
//! The failure is total and misleading. The runner cannot create its cache
//! directory, exits during startup with `EPERM`, and the MCP client sees only a
//! closed pipe:
//!
//! ```text
//! error: Failed to initialize cache at `/Users/me/.cache/uv`
//!   Caused by: failed to open file `/Users/me/.cache/uv/sdists-v9/.git`:
//!              Operation not permitted (os error 1)
//! ```
//!
//! ```text
//! MCP server 'kagi' handshake failed: ... Broken pipe (os error 32),
//! when send initialize request
//! ```
//!
//! Nothing in that chain names the sandbox, so the server simply "shows but
//! doesn't work".
//!
//! The fix maps the runner's cache onto the **session's tmpfs**: the writable
//! temp directory every confining profile already grants (Linux: `/tmp`,
//! mounted as a fresh tmpfs by the [`crate::jail`] backend; macOS: the Seatbelt
//! profile's writable temp dirs). Caches are scratch state — a fetched wheel or
//! npm tarball — so they belong on scratch storage, not in `$HOME` and not in
//! `$GROK_HOME`, which the session owns for real state. Nothing has to be
//! granted to the sandbox for this to work, so the profile's write set is
//! unchanged and a package that would have been refused a home-directory write
//! stays refused.
//!
//! Redirection happens in a *child* environment, never in the session's own
//! process: only the MCP child is affected, so a runner the model invokes
//! through bash keeps its normal caches.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Environment variables that relocate a package runner's cache/state onto the
/// session's writable temp storage.
///
/// Each entry is the variable and the subdirectory of the cache root that holds
/// its state. Only variables that are **absent** from the child env are set, so
/// an explicit value from the MCP server's own config always wins.
///
/// - `UV_CACHE_DIR`, `UV_TOOL_DIR`, `UV_TOOL_BIN_DIR`, `UV_PYTHON_INSTALL_DIR`:
///   `uvx`/`uv tool run` keeps four separate trees. `UV_CACHE_DIR` alone still
///   dies creating `~/.local/share/uv/tools`, and the tool dir alone still dies
///   on the cache and on managed Python downloads.
/// - `npm_config_cache`: `npx`/`npm exec` (`~/.npm/_cacache`).
/// - `npm_config_prefix`: `npx` installs a fetched package under the prefix.
/// - `PNPM_STORE_DIR` and `npm_config_store_dir`: `pnpm dlx` (both spellings;
///   pnpm reads the `npm_config_*` form for every `npm`-compatible setting).
/// - `BUN_INSTALL_CACHE_DIR` and `BUN_INSTALL`: `bunx` (`~/.bun/install/cache`).
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

/// The `(name, value)` pairs to add to a package runner's child environment so
/// its caches land on the session's writable temp storage.
///
/// `root` is the writable scratch root (the caller passes the session temp dir;
/// tests pass a fixture so the shipped function is driven against something
/// disposable rather than the host's real temp tree). Every directory is created
/// up front: a runner that cannot create its own cache root fails exactly the
/// way this function exists to prevent. A directory that cannot be created is
/// skipped, so the runner gets the variables it can use rather than none.
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

/// Whether `program` names a package runner — a command that fetches the server
/// it runs from a package index and therefore needs a writable cache.
///
/// The check is on the **file name** of the command with a launcher suffix
/// stripped, so a path-qualified invocation (`/opt/homebrew/bin/uvx`) is
/// recognized exactly like the bare name, and a Windows launcher (`npx.cmd`)
/// matches its `npx` stem on every platform. Matching is by whole name rather
/// than substring: `my-uvx-wrapper` is a different program.
pub fn is_package_runner(program: &str) -> bool {
    // Take the final path component under BOTH separators: `Path` on Unix does
    // not split a Windows path, and the `.cmd` suffix has to be stripped the
    // same way on either host so the classification cannot differ by platform.
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

/// The scratch root a package runner's caches are mapped onto.
///
/// This is the session's writable temp storage — the same directory family
/// [`crate::paths::temp_writable_paths`] hands a confining profile, so it is
/// writable by construction and needs no new grant. It is deliberately **not**
/// `$GROK_HOME`: caches are disposable, and `$GROK_HOME` is where the session
/// keeps state that must survive.
///
/// `TMPDIR` wins when it is set, because the macOS Seatbelt backend points it at
/// the jail's dedicated writable scratch dir (`JailPlan::temp_dir`). Otherwise
/// `/tmp` is used, which is the writable tmpfs on both backends.
///
/// Returns `None` when no writable scratch root is available, so a caller sets
/// nothing rather than pointing a runner at a directory it cannot use.
pub fn scratch_root() -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::iter::once(PathBuf::from("/tmp")))
        .collect();
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// The scratch subdirectory a runner's state lives under, for diagnostics.
///
/// Kept separate from [`cache_env`] so a caller can say *where* the redirect
/// pointed without re-deriving it.
pub fn cache_root(scratch: &Path) -> PathBuf {
    scratch.join("grok-mcp-cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped mapping must cover every variable the four runner families
    /// read. Regression guard: dropping `UV_TOOL_DIR` (or any single entry)
    /// leaves `uvx` dying on `~/.local/share/uv/tools` even with the others set.
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
        let names: Vec<&str> = vars
            .iter()
            .filter_map(|(n, _)| n.to_str())
            .collect();
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
    /// disposable, and mapping them into either would either widen the profile's
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
            if let Some(home) = dirs::home_dir() {
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

    /// `scratch_root` must return a directory that actually exists, and must
    /// prefer `TMPDIR` (which the macOS jail sets to its dedicated writable
    /// scratch dir) over the `/tmp` fallback.
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
