//! `--sandbox=pathbox` / `--ro`/`--rw`/`--rn`: replace this process with itself
//! inside an OS jail ("pathbox").
//!
//! Linux uses `bwrap`, macOS uses `sandbox-exec`. The re-exec happens before
//! any other startup work, so everything the session does — the agent, its
//! tools, every child process — runs inside the jail.
//!
//! The mount list is ordered. A later `--ro`/`--rw` overrides an earlier one
//! for the same path or for a path that contains it. `$GROK_HOME` (`~/.grok`)
//! is bound read-write after the user mounts, so nothing can take it away. A
//! `--rn` path is actively hidden (Deny) and, being applied last, hides even
//! when it lives under a visible mount.
//!
//! The working directory is bound read-write by default, so the jail can write
//! in the cwd without an explicit `--rw .`. An explicit `--ro .` or `--rw .`
//! (or any user mount containing the cwd) overrides that default.
//!
//! The jail is selected by `--sandbox=pathbox` (the reserved name of the
//! path-mount jail) or by any `--ro`/`--rw`/`--rn` on the line; a bare
//! `--sandbox` with no value is invalid, and `--sandbox <other-profile>` is the
//! built-in profile sandbox, never the jail.
//!
//! Both backends confine reads AND writes to the bound set: bwrap mounts only
//! the base, the user mounts and `$GROK_HOME`, and the Seatbelt profile denies
//! every file read and write and re-allows exactly that same set. A path no
//! mount covers (a sibling repo, `$HOME` outside `~/.grok`) is unreadable and
//! unwritable on macOS just as it is on Linux.
//!
//! The per-path defaults this jail applies — the working directory, `$GROK_HOME`,
//! `/tmp` and the platform system base — are read from the `[jail]` table of
//! `$GROK_HOME/config.toml` (see [`JailDefaults`]). This is a *default layer*:
//! a session with no config, or no `[jail]` section, gets exactly the four
//! release defaults below, and a command-line `--ro`/`--rw` always beats the
//! config for the path it names. This jail-layer config is deliberately
//! separate from the built-in **grok-build sandbox** (`~/.grok/sandbox.toml`,
//! `[profiles.*]`, `SandboxProfile`, the nono/Seatbelt/Landlock deny manager in
//! `profiles.rs`) — that file is never read here, and these defaults never
//! touch its `deny`/`read_write` profile model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Set on the jailed process. Its presence stops a second re-exec.
pub const JAIL_ENV_VAR: &str = "__GROK_SANDBOX_JAIL";
/// The marker `is_inside_bwrap` reads. Set too, so the profile bwrap path
/// does not wrap an already-jailed process a second time.
const BWRAP_ENV_VAR: &str = "__GROK_INSIDE_BWRAP";
/// Read-only system paths the jail binds so the process can execute at all.
/// Bound first, so a user mount can override any of them.
/// `/run` and `/var` are here because `/etc/resolv.conf` is a symlink into one
/// of them on a systemd host, and a jail without it resolves no name at all.
const SYSTEM_RO_BASE: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/run", "/var",
];

/// Extra read-only system paths macOS needs that the Linux base omits.
///
/// bwrap binds the whole tree at the VFS layer, so Linux gets everything it
/// needs from `SYSTEM_RO_BASE`. Seatbelt matches *canonical* paths, so macOS
/// must name what the process genuinely reads that is not under the Linux base:
/// - `/System` and `/Library` hold the shared dylibs the dynamic loader pulls
///   in — a jail that does not allow reading them cannot load the binary at all.
/// - `/private` is the real home of `/etc`, `/var` and `/tmp` (they are
///   symlinks into it on macOS), and Seatbelt matches the real path, so it has
///   to be allowed too.
#[cfg(target_os = "macos")]
const SYSTEM_RO_BASE_MACOS: &[&str] = &["/System", "/Library", "/private"];

/// The read-only system base, platform-appropriate. Used by the Seatbelt
/// profile so macOS confines reads over exactly the set of paths it needs to
/// run, matching the tree bwrap mounts on Linux.
#[cfg(target_os = "macos")]
fn system_ro_base() -> impl Iterator<Item = &'static str> {
    SYSTEM_RO_BASE
        .iter()
        .copied()
        .chain(SYSTEM_RO_BASE_MACOS.iter().copied())
}
/// Where the dedicated tmpfs is mounted on Linux.
const JAIL_TMP: &str = "/tmp";

/// How a path is bound inside the jail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Readable, not writable.
    Ro,
    /// Readable and writable.
    Rw,
    /// Actively denied (hidden): not readable and not writable, even when an
    /// ancestor is mounted in via `--ro`/`--rw` or exposed by a default.
    Deny,
}

/// One mount the jail will make. Carries a `--ro`/`--rw`/`--rn` request, in
/// command line order, OR the plan-injected working-directory mount
/// `build_plan` puts at the front of [`JailPlan::mounts`] (bound read-write by
/// default). A [`Access::Deny`] mount shadows any visible ancestor and is
/// applied last by both backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub access: Access,
    pub path: PathBuf,
}

/// How `/tmp` is exposed in the jail. Unlike the three `Access` toggles this is
/// a tri-state, because the release default mounts a dedicated writable tmpfs
/// there rather than binding the host tree (see [`system_ro_base`]/[`JAIL_TMP`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpHandling {
    /// A fresh, writable tmpfs over `/tmp`. On Linux this is a `--tmpfs` mount;
    /// on macOS Seatbelt exposes a dedicated sandbox temp dir, so nothing binds
    /// the host `/tmp`. The release default.
    Tmpfs,
    /// Bind the host `/tmp` in read-write. The knob the objective names for the
    /// future "grant the git ssh control socket under `/tmp`" follow-up. The
    /// existing [`Mount`]/`Access` grammar intentionally does not model this, so
    /// we advertise a value an SSH control socket over host `/tmp` needs.
    Rw,
    /// Bind the host `/tmp` read-only.
    Ro,
}

/// The per-path defaults the `--sandbox` re-exec jail applies where the
/// command line is silent. This is the [`config.toml` `[jail]`] layer only —
/// distinct from, and never read by, the built-in grok-build sandbox profiles
/// (`sandbox.toml`). Read via [`JailDefaults::load`]; the release defaults match
/// the historical jail byte for byte, so a session with no `[jail]` section is
/// unchanged:
///
/// - [`cwd`](JailDefaults::cwd) — the working directory, `Access::Rw`
/// - [`grok_home`](JailDefaults::grok_home) — `$GROK_HOME`, `Access::Rw`
/// - [`tmp`](JailDefaults::tmp) — `/tmp`, [`TmpHandling::Tmpfs`]
/// - [`system`](JailDefaults::system) — the platform system base, `Access::Ro`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JailDefaults {
    /// Working-directory access when no user `--ro`/`--rw` covers it.
    pub cwd: Access,
    /// `$GROK_HOME` access (kept, like today, bound after the user mounts — a
    /// config-granted ro is honoured, a CLI flag cannot take it below that).
    pub grok_home: Access,
    /// `/tmp` handling.
    pub tmp: TmpHandling,
    /// The read-only system base (`/usr`, `/lib`, … and the macOS system dirs).
    pub system: Access,
}

impl Default for JailDefaults {
    fn default() -> Self {
        Self {
            cwd: Access::Rw,
            grok_home: Access::Rw,
            tmp: TmpHandling::Tmpfs,
            system: Access::Ro,
        }
    }
}

/// One recognized `[jail]` config key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultKey {
    Cwd,
    GrokHome,
    Tmp,
    System,
}

/// Map a `[jail]` config key name onto its enum so we resolve each key the same
/// way (`cwd`/`grok_home`/`system` take `ro`/`rw`, `tmp` takes the tri-state).
fn default_key(name: &str) -> Option<DefaultKey> {
    match name {
        "cwd" => Some(DefaultKey::Cwd),
        "grok_home" => Some(DefaultKey::GrokHome),
        "tmp" => Some(DefaultKey::Tmp),
        "system" => Some(DefaultKey::System),
        _ => None,
    }
}

impl JailDefaults {
    /// The four defaults from one `[jail]` config value. A key that is absent,
    /// not a string, or holds a string we do not recognize keeps the release
    /// default: a bad token is a no-op rather than a jail that silently grants
    /// more (or refuses) than the user wrote meaningful words for. Unknown
    /// section keys are ignored the same way — they may be reserved by `--sandbox`
    /// profile config or written by a newer build.
    pub fn from_config(config: &toml::Value) -> JailDefaults {
        let mut defaults = JailDefaults::default();
        let Some(table) = config.get("jail").and_then(toml::Value::as_table) else {
            return defaults;
        };
        for (key, value) in table {
            let Some(kind) = default_key(key) else { continue };
            let Some(token) = value.as_str() else { continue };
            match (kind, token) {
                (DefaultKey::Cwd, "ro") => defaults.cwd = Access::Ro,
                (DefaultKey::Cwd, "rw") => defaults.cwd = Access::Rw,
                (DefaultKey::GrokHome, "ro") => defaults.grok_home = Access::Ro,
                (DefaultKey::GrokHome, "rw") => defaults.grok_home = Access::Rw,
                (DefaultKey::System, "ro") => defaults.system = Access::Ro,
                (DefaultKey::System, "rw") => defaults.system = Access::Rw,
                (DefaultKey::Tmp, "tmpfs") => defaults.tmp = TmpHandling::Tmpfs,
                (DefaultKey::Tmp, "rw") => defaults.tmp = TmpHandling::Rw,
                (DefaultKey::Tmp, "ro") => defaults.tmp = TmpHandling::Ro,
                _ => {}
            }
        }
        defaults
    }

    /// Read the `[jail]` defaults from `<home>/config.toml`. A missing or
    /// unparsable file yields the release defaults (fail open to the historical
    /// behavior, never to a `[jail]`-free accident). The built-in grok-build
    /// sandbox file `sandbox.toml` is deliberately never consulted here. The
    /// caller may pass a blank `home` to force defaults (used by tests that drive
    /// `build_plan` without a config fixture).
    pub fn load(home: &Path) -> JailDefaults {
        let path = home.join("config.toml");
        let contents = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return JailDefaults::default(),
        };
        match toml::from_str::<toml::Value>(&contents) {
            Ok(value) => JailDefaults::from_config(&value),
            Err(_) => JailDefaults::default(),
        }
    }
}

/// What the command line asked the jail for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JailRequest {
    /// Whether a path-box jail was asked for: `--sandbox=pathbox` or any
    /// `--ro`/`--rw`/`--rn` path flag.
    pub enabled: bool,
    /// User mounts, in command-line order. Later entries win (a `--rn` deny is
    /// applied after every visible bind regardless).
    pub mounts: Vec<Mount>,
}

/// Why a jail could not start. Every one of these is fatal: a partial jail is
/// worse than none, because the caller believes the process is confined.
#[derive(Debug, thiserror::Error)]
pub enum JailError {
    #[error("--{flag} needs a PATH argument")]
    MissingPath { flag: String },
    #[error("--{flag} path '{path}': {source}")]
    BadPath {
        flag: String,
        path: String,
        source: std::io::Error,
    },
    #[error(
        "a bare `--sandbox` (no profile value) is no longer valid. \
         The path-mount jail is named `--sandbox={PATHBOX_PROFILE}` (or implied \
         by `--ro`/`--rw`/`--rn`); use `--sandbox <profile>` for a built-in \
         sandbox profile."
    )]
    BareSandboxInvalid,
    #[error(
        "cannot combine sandbox profile '{profile}' with --ro/--rw/--rn path \
         flags; use --sandbox={PATHBOX_PROFILE} for path mounts."
    )]
    ProfileWithPathFlags { profile: String },
    #[error("the path '{path}' is denied by `--rn`, but it is the working \
         directory; the jail cannot start a process in a hidden cwd")]
    CwdDenied { path: String },
    #[error("could not resolve the current executable: {0}")]
    NoSelfExe(std::io::Error),
    #[error("could not read the current directory: {0}")]
    NoCwd(std::io::Error),
    #[error(
        "the working directory '{cwd}' is not bound inside the sandbox. \
         Pass --rw {cwd} (or --ro {cwd}) to bind it."
    )]
    CwdNotBound { cwd: String },
    #[error("could not create the sandbox temp directory '{path}': {source}")]
    NoTempDir { path: String, source: std::io::Error },
    #[error("`{0}` is not installed, so --sandbox cannot confine this process")]
    MissingBackend(&'static str),
    #[error("--sandbox is not supported on this platform")]
    UnsupportedPlatform,
}

/// Whether this process is already inside a jail this module built.
pub fn is_jailed() -> bool {
    std::env::var_os(JAIL_ENV_VAR).is_some()
}

/// The reserved name of the path-mount jail profile. Like the built-in sandbox
/// profiles (`workspace`, `strict`, …) this is magic and un-overridable: a
/// project/custom `sandbox.toml` profile cannot redefine it, and it is the only
/// value of `--sandbox` that selects the re-exec jail (see [`parse_jail_args`]).
pub const PATHBOX_PROFILE: &str = "pathbox";

/// Read `--sandbox`, `--ro`, `--rw` and `--rn` off the raw command line.
///
/// The raw line is what carries the order of the visible mounts; a parsed
/// struct groups the flags into separate lists and loses it.
///
/// The path-mount jail is enabled only when `--sandbox=pathbox` is given, OR
/// any of `--ro`/`--rw`/`--rn` appears (the jail is then implied). A bare
/// `--sandbox` with no value is always invalid, and a `--sandbox <profile>`
/// naming anything other than `pathbox` is the built-in profile sandbox (not a
/// jail) and cannot be combined with path flags. Scanning stops at a bare `--`,
/// so a prompt is never read as a flag.
pub fn parse_jail_args<I>(args: I) -> Result<JailRequest, JailError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut request = JailRequest::default();
    let mut args = args.into_iter().peekable();
    let mut has_path_flag = false;
    let mut has_pathbox = false;
    let mut other_profile: Option<String> = None;
    let mut has_bare_sandbox = false;

    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else { continue };
        if text == "--" {
            break;
        }
        if let Some(path) = flag_value("ro", text, &mut args)? {
            has_path_flag = true;
            request.enabled = true;
            request.mounts.push(Mount {
                access: Access::Ro,
                path,
            });
            continue;
        }
        if let Some(path) = flag_value("rw", text, &mut args)? {
            has_path_flag = true;
            request.enabled = true;
            request.mounts.push(Mount {
                access: Access::Rw,
                path,
            });
            continue;
        }
        if let Some(path) = flag_value("rn", text, &mut args)? {
            has_path_flag = true;
            request.enabled = true;
            request.mounts.push(Mount {
                access: Access::Deny,
                path,
            });
            continue;
        }
        // `--sandbox` takes an optional value (clap `num_args = 0..=1`); mirror
        // clap's "next non-flag token is the value" so the two agree.
        if text == "--sandbox" {
            let has_value = args
                .peek()
                .and_then(|next| next.to_str())
                .is_some_and(|next| !next.starts_with('-'));
            if has_value {
                let value = args
                    .next()
                    .map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if value == PATHBOX_PROFILE {
                    has_pathbox = true;
                    request.enabled = true;
                } else {
                    other_profile.get_or_insert(value);
                }
            } else {
                has_bare_sandbox = true;
            }
            continue;
        }
        if let Some(rest) = text.strip_prefix("--sandbox=") {
            if rest == PATHBOX_PROFILE {
                has_pathbox = true;
                request.enabled = true;
            } else if rest.is_empty() {
                has_bare_sandbox = true;
            } else {
                other_profile.get_or_insert(rest.to_owned());
            }
            continue;
        }
    }

    // Pre-clap mirror of the CLI contract, so we never run a jail the command
    // line did not actually authorize.
    if has_bare_sandbox {
        return Err(JailError::BareSandboxInvalid);
    }
    if let Some(profile) = other_profile
        && (has_path_flag || has_pathbox)
    {
        return Err(JailError::ProfileWithPathFlags { profile });
    }
    Ok(request)
}

/// Match `--<flag> PATH` or `--<flag>=PATH` and return the path.
fn flag_value<I>(
    flag: &str,
    text: &str,
    args: &mut std::iter::Peekable<I>,
) -> Result<Option<PathBuf>, JailError>
where
    I: Iterator<Item = OsString>,
{
    let long = format!("--{flag}");
    if text == long {
        let value = args.next().ok_or_else(|| JailError::MissingPath {
            flag: flag.to_string(),
        })?;
        return Ok(Some(PathBuf::from(value)));
    }
    let eq = format!("--{flag}=");
    if let Some(rest) = text.strip_prefix(&eq) {
        if rest.is_empty() {
            return Err(JailError::MissingPath {
                flag: flag.to_string(),
            });
        }
        return Ok(Some(PathBuf::from(rest)));
    }
    Ok(None)
}

/// Everything the backend command needs, with each path already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JailPlan {
    /// User mounts, in order, canonicalized, with the working directory
    /// mounted at the FRONT. The front cwd mount is bound read-write by
    /// default (`Access::Rw`) so a bare `--sandbox` is usable without an
    /// explicit `--rw .`; a user `--ro .`/`--rw .` (or any user mount that
    /// contains the cwd) overrides that default, and being at the front means
    /// a later, more-specific user mount still wins for a path both cover.
    pub mounts: Vec<Mount>,
    /// `$GROK_HOME`. Bound read-write after the user mounts.
    pub grok_home: PathBuf,
    /// The scratch directory the jail dedicates to this process.
    pub temp_dir: PathBuf,
    /// An empty, read-only host directory used as the bwrap sink for `--rn`
    /// denied paths (bound ro over each deny so nothing in them leaks). Never
    /// used by the Seatbelt backend, which hides via profile rules instead.
    pub deny_sink: PathBuf,
    /// The binary to run inside the jail.
    pub self_exe: PathBuf,
    /// The directory the jailed process starts in.
    pub cwd: PathBuf,
    /// Arguments for the jailed binary, without `argv[0]`.
    pub args: Vec<OsString>,
    /// The per-path defaults resolved from `[jail]` config (or the four release
    /// defaults). The bwrap/Seatbelt builders read this for the access level of
    /// `/tmp`, the system base and `$GROK_HOME`; the cwd default was already
    /// applied by [`build_plan`] when it injected the front cwd mount.
    pub defaults: JailDefaults,
}

/// Resolve a request into a plan. Every path must exist: a missing bind is a
/// hole the caller cannot see. `defaults` is the config layer (see
/// [`JailDefaults`]) that supplies the per-path access where the command line
/// is silent; the runtime caller loads it via [`JailDefaults::load`], tests pass
/// an explicit value so they never depend on the host's `config.toml`.
///
/// The working directory is bound read-write by default, so a jail built with
/// the pathbox marker (`--sandbox=pathbox`) or a path flag can write in the
/// cwd without an explicit `--rw .`. The
/// effective cwd access is the LAST user mount that contains the cwd
/// (`defaults.cwd` when none does), and that single cwd mount is placed at the
/// FRONT of `plan.mounts`. Front-placement plus the effective access is what
/// lets `--ro .` win cleanly: the front cwd mount then carries `Access::Ro`,
/// so the Seatbelt profile emits no stale write-allow for the cwd and bwrap
/// binds it read-only. `parse_jail_args` is untouched — the default is a
/// plan-level injection, not a flag. A user `--ro`/`--rw` that names (or
/// contains) the cwd is checked first and always beats `defaults.cwd`.
pub fn build_plan(
    request: &JailRequest,
    defaults: &JailDefaults,
    args: Vec<OsString>,
) -> Result<JailPlan, JailError> {
    let mut mounts = Vec::with_capacity(request.mounts.len());
    for mount in &request.mounts {
        let flag = match mount.access {
            Access::Ro => "ro",
            Access::Rw => "rw",
            Access::Deny => "rn",
        };
        let path = dunce::canonicalize(&mount.path).map_err(|source| JailError::BadPath {
            flag: flag.to_string(),
            path: mount.path.display().to_string(),
            source,
        })?;
        mounts.push(Mount {
            access: mount.access,
            path,
        });
    }
    let grok_home = crate::paths::grok_home();
    std::fs::create_dir_all(&grok_home).map_err(|source| JailError::BadPath {
        flag: "grok-home".to_string(),
        path: grok_home.display().to_string(),
        source,
    })?;
    let grok_home = dunce::canonicalize(&grok_home).unwrap_or(grok_home);
    let self_exe = std::env::current_exe().map_err(JailError::NoSelfExe)?;
    let cwd = std::env::current_dir().map_err(JailError::NoCwd)?;
    let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
    // A `--rn` that contains or equals the cwd hides it, but the jail must
    // chdir into the cwd to run — that combination is impossible, so refuse it
    // with a message instead of a confusing unbound-cwd error.
    if mounts
        .iter()
        .any(|mount| mount.access == Access::Deny && cwd.starts_with(&mount.path))
    {
        return Err(JailError::CwdDenied {
            path: cwd.display().to_string(),
        });
    }
    // The effective cwd access comes from the LAST user mount that contains
    // the cwd (`cwd.starts_with(mount.path)`); later mounts win in the ordered
    // contract, so scanning reversed is what honors `--ro .` over an earlier
    // `--rw /`. Deny mounts never cover the cwd here (they would have been
    // refused above). With no covering mount, fall back to the config default
    // (`Access::Rw` in the release, overridable to `Access::Ro` from `[jail]`).
    let cwd_access = mounts
        .iter()
        .rev()
        .find(|mount| cwd.starts_with(&mount.path))
        .map(|mount| mount.access)
        .unwrap_or(defaults.cwd);
    mounts.insert(
        0,
        Mount {
            access: cwd_access,
            path: cwd.clone(),
        },
    );
    let temp_dir = dedicated_temp_dir()?;
    // A host sink for `--rn` denoted paths: an empty dir bwrap binds read-only
    // over each denied path so the subtree reads as empty and nothing leaks.
    // Unused by the Seatbelt backend (which hides via profile rules).
    let deny_sink = crate::paths::grok_home()
        .join("sandbox-tmp")
        .join(std::process::id().to_string())
        .join("deny-sink");
    std::fs::create_dir_all(&deny_sink).map_err(|source| JailError::NoTempDir {
        path: deny_sink.display().to_string(),
        source,
    })?;
    let plan = JailPlan {
        mounts,
        grok_home,
        temp_dir,
        deny_sink,
        self_exe,
        cwd,
        args,
        defaults: *defaults,
    };
    plan.check_cwd_is_bound()?;
    Ok(plan)
}

impl JailPlan {
    /// Refuse a jail whose working directory nothing binds. bwrap answers that
    /// with a bare chdir error, which reads as a broken sandbox rather than a
    /// missing `--rw`.
    fn check_cwd_is_bound(&self) -> Result<(), JailError> {
        // The temp directory is compared exactly: on Linux it is a fresh
        // tmpfs, so a path under it exists on the host and not in the jail.
        if self.cwd == self.temp_dir {
            return Ok(());
        }
        let bound = self
            .mounts
            .iter()
            .map(|mount| mount.path.as_path())
            .chain(std::iter::once(self.grok_home.as_path()))
            .chain(SYSTEM_RO_BASE.iter().map(Path::new));
        for path in bound {
            if self.cwd.starts_with(path) {
                return Ok(());
            }
        }
        Err(JailError::CwdNotBound {
            cwd: self.cwd.display().to_string(),
        })
    }
}

/// A fresh scratch directory for this process.
///
/// On Linux the jail mounts a tmpfs over `/tmp` and this directory lives
/// inside it, so it never touches the host. On macOS Seatbelt mounts nothing,
/// so the directory is a real one under `$GROK_HOME` and `TMPDIR` points at it.
fn dedicated_temp_dir() -> Result<PathBuf, JailError> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from(JAIL_TMP));
    }
    let path = crate::paths::grok_home()
        .join("sandbox-tmp")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&path).map_err(|source| JailError::NoTempDir {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// Build the `bwrap` command that runs the plan.
///
/// Order is the whole contract: the read-only system base first, then the
/// user mounts as given, then `$GROK_HOME`. bwrap applies binds in order and
/// a later one covers an earlier one, so this is what makes a later `--ro`
/// beat an earlier `--rw`.
///
/// The access each synthesized bind carries comes from [`JailPlan::defaults`]
/// where a user mount does not name it: the system base from `system`, `/tmp`
/// from `tmp`, and `$GROK_HOME` from `grok_home`. All three default to the
/// release behavior (ro base, tmpfs `/tmp`, rw `$GROK_HOME`), so a plan built
/// with [`JailDefaults::default`] produces byte-identical argv to the jail
/// before config existed.
#[cfg(target_os = "linux")]
pub fn bwrap_command(plan: &JailPlan) -> std::process::Command {
    // No --die-with-parent: it kills the jail when bwrap's parent dies, and a
    // session started from a script that exits right after is a live session.
    let mut cmd = std::process::Command::new("bwrap");
    cmd.arg("--cap-drop").arg("ALL");
    // The platform read-only base. `--ro-bind-try` swallows an absent path;
    // a config `system = "rw"` binds it read-write instead (the user's explicit
    // choice to weaken the jail).
    let system_flag = match plan.defaults.system {
        Access::Ro => "--ro-bind-try",
        Access::Rw => "--bind",
    };
    for path in SYSTEM_RO_BASE {
        cmd.arg(system_flag).arg(path).arg(path);
    }
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--dev").arg("/dev");
    // `--dev` builds a fresh /dev without /dev/shm, and a program that wants
    // shared memory fails on the missing directory rather than on a denial.
    cmd.arg("--tmpfs").arg("/dev/shm");
    match plan.defaults.tmp {
        // Release default: a fresh, writable tmpfs over /tmp.
        TmpHandling::Tmpfs => {
            cmd.arg("--tmpfs").arg(JAIL_TMP);
        }
        // Overrides bind the host /tmp in/out so a granted path (e.g. an ssh
        // control socket) reaches the jailed process. bwrap creates the
        // destination mountpoint, so a bare bind joins the fresh namespace.
        TmpHandling::Rw => {
            cmd.arg("--bind").arg(JAIL_TMP).arg(JAIL_TMP);
        }
        TmpHandling::Ro => {
            cmd.arg("--ro-bind").arg(JAIL_TMP).arg(JAIL_TMP);
        }
    }
    for mount in &plan.mounts {
        match mount.access {
            Access::Ro => cmd
                .arg("--ro-bind")
                .arg(&mount.path)
                .arg(&mount.path),
            Access::Rw => cmd
                .arg("--bind")
                .arg(&mount.path)
                .arg(&mount.path),
            // Deny mounts are not bound here; they are applied last, after
            // every visible bind, as an empty read-only sink over the path.
            Access::Deny => {}
        }
    }
    // `$GROK_HOME`, bound after the user mounts. A config `grok_home = "ro"`
    // binds it read-only; the release default (`rw`) is unchanged. Because it
    // is bound last it survives a user `--ro` aimed at it either way.
    match plan.defaults.grok_home {
        Access::Rw => cmd
            .arg("--bind")
            .arg(&plan.grok_home)
            .arg(&plan.grok_home),
        Access::Ro => cmd
            .arg("--ro-bind")
            .arg(&plan.grok_home)
            .arg(&plan.grok_home),
    }
    cmd.arg("--ro-bind-try")
        .arg(&plan.self_exe)
        .arg(&plan.self_exe);
    // `--rn` denies hide their subtree *after* every visible bind so they
    // shadow even a containing `--rw`/`--ro` ancestor: bind the empty read-only
    // sink over each denied path. bwrap applies binds in order, so these last
    // binds win for the paths they name.
    for mount in &plan.mounts {
        if mount.access == Access::Deny {
            cmd.arg("--ro-bind")
                .arg(&plan.deny_sink)
                .arg(&mount.path);
        }
    }
    cmd.arg("--chdir").arg(&plan.cwd);
    cmd.arg("--setenv").arg(JAIL_ENV_VAR).arg("1");
    cmd.arg("--setenv").arg(BWRAP_ENV_VAR).arg("1");
    cmd.arg("--").arg(&plan.self_exe).args(&plan.args);
    cmd
}

/// Build the Seatbelt profile for the plan.
///
/// Seatbelt confines READS and WRITES here, mirroring bwrap. `(allow default)`
/// keeps the process's non-file capabilities (network, process, sysctl) open,
/// then `(deny file-read*)` and `(deny file-write*)` take every file access
/// away, and the rules that follow give each kind of access back only for the
/// paths the jail is supposed to expose:
///
/// - `/dev` (a terminal, a PTY, `/dev/null`) — read and write.
/// - a `--rw` mount — read and write.
/// - a `--ro` mount — read only.
/// - the read-only system base (including the macOS `/System`, `/Library` and
///   `/private` additions) — read only, or read and write when the config sets
///   `system = "rw"` (the user's explicit choice to weaken the jail).
/// - `$GROK_HOME` and the sandbox temp dir — read and write, unless
///   `grok_home = "ro"` leaves `$GROK_HOME` readable only.
///
/// Anything not in that set — e.g. a sibling repo under `$HOME` — is denied
/// for both reads and writes, exactly as bwrap confines it on Linux. SBPL
/// gives the last matching rule, so emitting the allows after the deny is what
/// makes them win, and emitting the mounts in order is what makes a later flag
/// beat an earlier one. Where a default from [`JailPlan::defaults`] is at play
/// (system base, `$GROK_HOME`) the emitted rule still derives from the real
/// shipped profile, so a config override changes exactly the rule the builder
/// materializes. The `/tmp` [`TmpHandling`] is a bwrap mount concept; Seatbelt
/// mounts nothing, so it has no rule here (the writable sandbox temp is the
/// separate `temp_dir` allowed below).
#[cfg(target_os = "macos")]
pub fn seatbelt_profile(plan: &JailPlan) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str("(deny file-read*)\n(deny file-write*)\n");
    // A terminal, a PTY and /dev/null are reads and writes every tool makes.
    profile.push_str("(allow file-read* (subpath \"/dev\"))\n");
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    // The read-only system base the process needs to run (dylibs, binaries,
    // config, and on macOS the real /private home of /etc, /var and /tmp).
    // Emitted before the user mounts, matching bwrap's bind order (base first,
    // then user mounts, then $GROK_HOME) so a user mount over a base path wins.
    // A config `system = "rw"` additionally re-allows writes over the base.
    for path in system_ro_base() {
        profile.push_str(&format!(
            "(allow file-read* (subpath \"{}\"))\n",
            sbpl_escape(Path::new(path))
        ));
        if plan.defaults.system == Access::Rw {
            profile.push_str(&format!(
                "(allow file-write* (subpath \"{}\"))\n",
                sbpl_escape(Path::new(path))
            ));
        }
    }
    // A --rw mount is readable and writable; a --ro mount is readable only. A
    // --rn (Deny) mount gets no allow here — it is hidden at the end.
    for mount in &plan.mounts {
        if mount.access == Access::Deny {
            continue;
        }
        profile.push_str(&format!(
            "(allow file-read* (subpath \"{}\"))\n",
            sbpl_escape(&mount.path)
        ));
        if mount.access == Access::Rw {
            profile.push_str(&format!(
                "(allow file-write* (subpath \"{}\"))\n",
                sbpl_escape(&mount.path)
            ));
        }
    }
    profile.push_str(&format!(
        "(allow file-read* (subpath \"{}\"))\n",
        sbpl_escape(&plan.grok_home)
    ));
    // `$GROK_HOME` is writable by release default; `grok_home = "ro"` leaves it
    // readable only, matching the bwrap `--ro-bind`. Rule emitted after the
    // mounts so nothing below it licks a rw grant back in for the home.
    if plan.defaults.grok_home == Access::Rw {
        profile.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            sbpl_escape(&plan.grok_home)
        ));
    }
    profile.push_str(&format!(
        "(allow file-read* (subpath \"{}\"))\n",
        sbpl_escape(&plan.temp_dir)
    ));
    profile.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        sbpl_escape(&plan.temp_dir)
    ));
    // `--rn` denies are emitted LAST so SBPL's last-matching-rule hides each
    // path even when a visible ancestor (including $GROK_HOME or /dev) was
    // allowed above it.
    for mount in &plan.mounts {
        if mount.access != Access::Deny {
            continue;
        }
        profile.push_str(&format!(
            "(deny file-read* (subpath \"{}\"))\n",
            sbpl_escape(&mount.path)
        ));
        profile.push_str(&format!(
            "(deny file-write* (subpath \"{}\"))\n",
            sbpl_escape(&mount.path)
        ));
    }
    profile
}

/// Quote a path for an SBPL string literal.
#[cfg(any(target_os = "macos", test))]
fn sbpl_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// Build the `sandbox-exec` command that runs the plan.
#[cfg(target_os = "macos")]
pub fn seatbelt_command(plan: &JailPlan) -> std::process::Command {
    let mut cmd = std::process::Command::new("sandbox-exec");
    cmd.arg("-p").arg(seatbelt_profile(plan));
    cmd.arg(&plan.self_exe).args(&plan.args);
    cmd.env(JAIL_ENV_VAR, "1");
    cmd.env("TMPDIR", &plan.temp_dir);
    cmd.current_dir(&plan.cwd);
    cmd
}

/// Build the backend command for this platform.
#[allow(unused_variables)]
fn backend_command(plan: &JailPlan) -> Result<std::process::Command, JailError> {
    #[cfg(target_os = "linux")]
    {
        require_backend("bwrap")?;
        Ok(bwrap_command(plan))
    }
    #[cfg(target_os = "macos")]
    {
        require_backend("sandbox-exec")?;
        Ok(seatbelt_command(plan))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(JailError::UnsupportedPlatform)
    }
}

/// Fail before the exec when the backend is absent, so the message names the
/// missing program instead of reporting a generic exec failure.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn require_backend(program: &'static str) -> Result<(), JailError> {
    let Some(path) = std::env::var_os("PATH") else {
        return Err(JailError::MissingBackend(program));
    };
    for dir in std::env::split_paths(&path) {
        if dir.join(program).exists() {
            return Ok(());
        }
    }
    Err(JailError::MissingBackend(program))
}

/// Replace this process with itself inside the jail, when the command line
/// asked for one.
///
/// Returns on the paths that need no jail: no `--sandbox`, or a process that
/// is inside one already. Every other outcome ends the process — either the
/// exec succeeds and this function never returns, or the jail could not be
/// built and starting unconfined would be a lie.
pub fn maybe_reexec_into_jail() {
    if is_jailed() {
        return;
    }
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let request = match parse_jail_args(argv.clone()) {
        Ok(request) => request,
        Err(e) => fail(&e.to_string()),
    };
    // Honor `GROK_SANDBOX=pathbox` (clap reads the same env into `--sandbox`,
    // but the raw job must re-exec before clap runs, so it reads the env too).
    let env_pathbox = std::env::var("GROK_SANDBOX").ok().as_deref() == Some(PATHBOX_PROFILE);
    if !request.enabled && !env_pathbox {
        return;
    }
    // Read the `[jail]` defaults from this process's `$GROK_HOME`/`config.toml`
    // (release defaults when there is no `[jail]` section), then drive the real
    // plan/service builders so the config layer shapes the emitted jail.
    let defaults = JailDefaults::load(&crate::paths::grok_home());
    let plan = match build_plan(&request, &defaults, argv) {
        Ok(plan) => plan,
        Err(e) => fail(&e.to_string()),
    };
    // Start the unsandboxed `gh` CI-status worker *before* the exec so its
    // stream fd survives into the jail. The jailed pager reads `gh` results
    // from it instead of reaching the host from inside the jail. `None` when
    // the worker cannot start — the jailed dot then degrades to "off", which
    // is the same graceful state as a missing `gh`.
    let ci_host_fd = crate::ci_host::spawn_ci_host(&plan.cwd);
    let mut cmd = match backend_command(&plan) {
        Ok(cmd) => cmd,
        Err(e) => fail(&e.to_string()),
    };
    // Thread the host-worker fd through the jail boundary. bwrap rebuilds the
    // env from its own argument list; Seatbelt inherits and we set it anyway.
    if let Some(fd) = ci_host_fd {
        let value = fd.to_string();
        #[cfg(target_os = "linux")]
        {
            cmd.arg("--setenv")
                .arg(crate::ci_host::CI_HOST_FD_ENV)
                .arg(&value);
        }
        #[cfg(target_os = "macos")]
        {
            cmd.env(crate::ci_host::CI_HOST_FD_ENV, &value);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let e = cmd.exec();
        fail(&format!("could not start the sandbox: {e}"));
    }
    #[cfg(not(unix))]
    {
        let _ = &mut cmd;
        fail("--sandbox is not supported on this platform");
    }
}

/// Report why the jail did not start, then exit. Never returns.
fn fail(message: &str) -> ! {
    eprintln!("error: --sandbox: {message}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn parse(args: &[&str]) -> JailRequest {
        parse_jail_args(argv(args)).expect("parse")
    }

    #[test]
    fn pathbox_value_and_each_path_flag_select_the_jail() {
        // The reserved pathbox name enables the jail with no mounts.
        assert!(parse(&["--sandbox=pathbox"]).enabled);
        assert!(parse(&["--sandbox", "pathbox"]).enabled);
        // Any of the three path flags alone implies the pathbox jail.
        assert!(parse(&["--rn", "/a"]).enabled);
        assert!(parse(&["--ro", "/a"]).enabled);
        assert!(parse(&["--rw", "/a"]).enabled);
        // A path flag beside an explicit pathbox marker is fine (redundant).
        assert!(parse(&["--sandbox=pathbox", "--rn", "/a/secrets"]).enabled);
    }

    #[test]
    fn bare_sandbox_is_invalid_everywhere() {
        // A bare `--sandbox` (no value) no longer means "the jail" — it is an
        // error even when path flags are present.
        assert!(parse_jail_args(argv(&["--sandbox"])).is_err());
        assert!(parse_jail_args(argv(&["--sandbox="])).is_err());
        assert!(parse_jail_args(argv(&["--sandbox", "--ro", "."])).is_err());
        assert!(parse_jail_args(argv(&["--rn", "/s", "--sandbox"])).is_err());
        // `--sandbox` followed by a prompt/path token is the profile value, so
        // with path flags it is the invalid profile+path mix, not a bare flag.
        assert!(parse_jail_args(argv(&["--sandbox", "strict", "--ro", "."])).is_err());
    }

    #[test]
    fn a_profile_value_is_not_the_jail_but_cannot_combine_with_path_flags() {
        let request = parse(&["--sandbox", "strict"]);
        assert!(!request.enabled, "a profile name must not build a jail");
        assert!(!parse(&["--sandbox=strict"]).enabled);
        // Mixing a (non-pathbox) profile with path flags is rejected outright.
        assert!(parse_jail_args(argv(&["--sandbox", "strict", "--rn", "/a"])).is_err());
        assert!(parse_jail_args(argv(&["--rn", "/a", "--sandbox", "strict"])).is_err());
    }

    #[test]
    fn ro_and_rw_keep_their_command_line_order() {
        let request = parse(&["--rw", "/a", "--ro", "/b", "--rw=/c"]);
        assert!(request.enabled, "a path flag asks for the jail by itself");
        assert_eq!(
            request.mounts,
            vec![
                Mount {
                    access: Access::Rw,
                    path: PathBuf::from("/a")
                },
                Mount {
                    access: Access::Ro,
                    path: PathBuf::from("/b")
                },
                Mount {
                    access: Access::Rw,
                    path: PathBuf::from("/c")
                },
            ]
        );
    }

    #[test]
    fn a_path_flag_without_a_path_is_an_error() {
        assert!(parse_jail_args(argv(&["--ro"])).is_err());
        assert!(parse_jail_args(argv(&["--rw="])).is_err());
    }

    #[test]
    fn scanning_stops_at_a_bare_double_dash() {
        let request = parse(&["--", "--rw", "/etc"]);
        assert!(!request.enabled);
        assert!(request.mounts.is_empty());
    }

    #[test]
    fn a_prompt_that_mentions_a_flag_is_not_a_flag() {
        let request = parse(&["fix --rw /etc please"]);
        assert!(!request.enabled);
    }

    fn plan_fixture(mounts: Vec<Mount>) -> JailPlan {
        JailPlan {
            mounts,
            grok_home: PathBuf::from("/home/u/.grok"),
            temp_dir: PathBuf::from("/tmp"),
            deny_sink: PathBuf::from("/home/u/.grok/sandbox-tmp/deny-sink"),
            self_exe: PathBuf::from("/opt/grok/bin/grok"),
            cwd: PathBuf::from("/work"),
            args: vec![OsString::from("--sandbox=pathbox")],
            defaults: JailDefaults::default(),
        }
    }

    #[test]
    fn cwd_must_be_bound() {
        let unbound = plan_fixture(vec![Mount {
            access: Access::Rw,
            path: PathBuf::from("/elsewhere"),
        }]);
        assert!(matches!(
            unbound.check_cwd_is_bound(),
            Err(JailError::CwdNotBound { .. })
        ));
        let bound = plan_fixture(vec![Mount {
            access: Access::Ro,
            path: PathBuf::from("/work"),
        }]);
        assert!(bound.check_cwd_is_bound().is_ok());
    }

    #[test]
    fn a_cwd_under_the_read_only_system_base_needs_no_mount() {
        let mut plan = plan_fixture(Vec::new());
        plan.cwd = PathBuf::from("/usr/share/example");
        assert!(plan.check_cwd_is_bound().is_ok());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn bwrap_binds_grok_home_after_the_user_mounts() {
        let plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/work"),
            },
            Mount {
                access: Access::Ro,
                path: PathBuf::from("/work/secrets"),
            },
        ]);
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let rw = args
            .windows(3)
            .position(|w| w == ["--bind", "/work", "/work"])
            .expect("rw bind");
        let ro = args
            .windows(3)
            .position(|w| w == ["--ro-bind", "/work/secrets", "/work/secrets"])
            .expect("ro bind");
        let home = args
            .windows(3)
            .position(|w| w == ["--bind", "/home/u/.grok", "/home/u/.grok"])
            .expect("grok home bind");
        assert!(rw < ro, "user mounts keep their order: {args:?}");
        assert!(ro < home, "grok home must win over a user mount: {args:?}");
        assert!(
            args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]),
            "a dedicated tmpfs must be mounted: {args:?}"
        );
        let base = args
            .windows(3)
            .position(|w| w[0] == "--ro-bind-try" && w[1] == "/usr")
            .expect("system base");
        assert!(base < rw, "a user mount must override the base: {args:?}");
        assert!(
            args.windows(2).any(|w| w == ["--chdir", "/work"]),
            "the jail must keep the working directory: {args:?}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_confines_reads_and_writes_ending_with_grok_home() {
        let plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/work"),
            },
            Mount {
                access: Access::Ro,
                path: PathBuf::from("/work/secrets"),
            },
        ]);
        let profile = seatbelt_profile(&plan);
        // Both reads and writes are denied up front.
        assert!(profile.contains("(deny file-read*)\n"));
        assert!(profile.contains("(deny file-write*)\n"));
        // The rw mount is readable and writable.
        let rw_read = profile
            .find("(allow file-read* (subpath \"/work\"))")
            .expect("rw read rule");
        let rw_write = profile
            .find("(allow file-write* (subpath \"/work\"))")
            .expect("rw write rule");
        // The ro mount is readable only — it must NOT be writable.
        let ro_read = profile
            .find("(allow file-read* (subpath \"/work/secrets\"))")
            .expect("ro read rule");
        assert!(
            !profile.contains("(allow file-write* (subpath \"/work/secrets\"))"),
            "a --ro mount must not be writable: {profile}"
        );
        // The system base is readable so the process can load and run.
        let base_read = profile
            .find("(allow file-read* (subpath \"/usr\"))")
            .expect("system base read rule");
        let home = profile
            .find("(allow file-write* (subpath \"/home/u/.grok\"))")
            .expect("grok home rule");
        // Last matching rule wins: the rw read comes before its own write, the
        // ro read comes after the rw read, the base before both, and grok_home
        // after everything.
        assert!(base_read < rw_read, "base must precede user mounts");
        assert!(rw_read < rw_write, "read then write for an rw mount");
        assert!(rw_read < ro_read, "user mounts keep their order");
        assert!(ro_read < home, "grok home must win over a user mount");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_reads_cover_the_macos_system_base() {
        // macOS needs /System, /Library and the real /private tree readable or
        // the binary cannot dyld-load; without them the read-deny bricks the
        // sandboxed process outright.
        let plan = plan_fixture(Vec::new());
        let profile = seatbelt_profile(&plan);
        for path in ["/System", "/Library", "/private", "/usr", "/etc", "/opt"] {
            assert!(
                profile.contains(&format!("(allow file-read* (subpath \"{path}\"))\n")),
                "missing read allow for {path}: {profile}"
            );
        }
        assert!(
            !profile.contains("(allow file-write* (subpath \"/System\"))"),
            "the system base must never be writable: {profile}"
        );
    }

    #[test]
    fn sbpl_escapes_a_quote_in_a_path() {
        assert_eq!(sbpl_escape(Path::new("/a\"b")), "/a\\\"b");
    }

    // ── cwd rw default ─────────────────────────────────────────────────────
    //
    // These drive the shipped functions end to end (`parse_jail_args` →
    // `build_plan` → `bwrap_command` / `seatbelt_profile`) from the real
    // working directory, so the assertions are about the actual cwd the jail
    // would mount, not a hand-built `JailPlan`.

    /// Run the shipped parse + plan against the real cwd, with the release
    /// (no-config) defaults so the assertions are independent of any host
    /// `config.toml`.
    #[allow(clippy::needless_pass_by_value)]
    fn plan_for(args: &[&str]) -> JailPlan {
        let request = parse(args);
        assert!(request.enabled, "args must ask for a jail");
        build_plan(&request, &JailDefaults::default(), argv(args)).expect("build_plan")
    }

    /// The front of `plan.mounts` is the injected cwd mount.
    fn front_cwd_mount(plan: &JailPlan) -> &Mount {
        &plan.mounts[0]
    }

    #[test]
    fn cwd_defaults_to_rw_when_no_cwd_flag_is_given() {
        let plan = plan_for(&["--sandbox=pathbox"]);
        let front = front_cwd_mount(&plan);
        assert_eq!(front.access, Access::Rw, "pathbox (no mounts) must mount the cwd rw");
        assert_eq!(
            front.path, plan.cwd,
            "the injected mount must be exactly the working directory"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cwd_default_bwrap_binds_the_cwd_read_write() {
        let plan = plan_for(&["--sandbox=pathbox"]);
        let cwd = plan.cwd.display().to_string();
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3).any(|w| w == ["--bind", &cwd, &cwd]),
            "the cwd must be bound read-write by default: {args:?}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn cwd_default_seatbelt_allows_write_on_the_cwd() {
        let plan = plan_for(&["--sandbox=pathbox"]);
        let profile = seatbelt_profile(&plan);
        let cwd = sbpl_escape(&plan.cwd);
        assert!(
            profile.contains(&format!("(allow file-write* (subpath \"{cwd}\"))\n")),
            "the cwd must be writable by default: {profile}"
        );
    }

    #[test]
    fn ro_cwd_overrides_the_rw_default() {
        let plan = plan_for(&["--sandbox=pathbox", "--ro", "."]);
        assert_eq!(
            front_cwd_mount(&plan).access,
            Access::Ro,
            "--ro . must mount the cwd read-only"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn ro_cwd_bwrap_binds_read_only_and_never_read_write() {
        let plan = plan_for(&["--sandbox=pathbox", "--ro", "."]);
        let cwd = plan.cwd.display().to_string();
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3).any(|w| w == ["--ro-bind", &cwd, &cwd]),
            "--ro . must bind the cwd read-only: {args:?}"
        );
        assert!(
            !args.windows(3).any(|w| w == ["--bind", &cwd, &cwd]),
            "--ro . must NOT leave a read-write bind for the cwd: {args:?}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn ro_cwd_seatbelt_grants_read_but_not_write() {
        let plan = plan_for(&["--sandbox=pathbox", "--ro", "."]);
        let profile = seatbelt_profile(&plan);
        let cwd = sbpl_escape(&plan.cwd);
        assert!(
            profile.contains(&format!("(allow file-read* (subpath \"{cwd}\"))\n")),
            "--ro . must keep the cwd readable: {profile}"
        );
        assert!(
            !profile.contains(&format!("(allow file-write* (subpath \"{cwd}\"))\n")),
            "--ro . must not leave a stale write-allow for the cwd: {profile}"
        );
    }

    #[test]
    fn rw_cwd_remains_accepted_and_stays_rw() {
        let plan = plan_for(&["--sandbox=pathbox", "--rw", "."]);
        assert_eq!(
            front_cwd_mount(&plan).access,
            Access::Rw,
            "--rw . must keep the cwd read-write"
        );
    }

    #[test]
    fn precedence_keeps_the_parse_contract_and_lets_the_user_win() {
        // The default is a plan-level injection, not a flag: the parsed
        // request must carry exactly the user's mounts, with no injected cwd.
        let request = parse(&["--sandbox=pathbox", "--ro", "."]);
        assert_eq!(
            request.mounts,
            vec![Mount {
                access: Access::Ro,
                path: PathBuf::from("."),
            }],
            "parse_jail_args must not inject a cwd mount"
        );
        // And the plan must honor the user's ro (no stale write-allow).
        let plan = build_plan(&request, &JailDefaults::default(), argv(&["--sandbox=pathbox", "--ro", "."]))
            .expect("build_plan");
        assert_eq!(
            front_cwd_mount(&plan).access,
            Access::Ro,
            "a user --ro . must beat the rw default"
        );
        // A user ro mount that merely *contains* the cwd also flips it to ro.
        let plan = plan_for(&["--sandbox=pathbox", "--ro", "/"]);
        assert_eq!(
            front_cwd_mount(&plan).access,
            Access::Ro,
            "a user ro mount containing the cwd must override the rw default"
        );
    }

    #[test]
    fn capture_cwd_default_cases() {
        // Prints the plan + platform backend for the three cases so the
        // rw-vs-ro difference is visible in captured output, not just asserted.
        let cases: &[(&str, &[&str])] = &[
            ("pathbox (no mounts)", &["--sandbox=pathbox"]),
            ("pathbox --ro .", &["--sandbox=pathbox", "--ro", "."]),
            ("pathbox --rw .", &["--sandbox=pathbox", "--rw", "."]),
        ];
        for (label, args) in cases {
            let plan = plan_for(args);
            let mounts: Vec<String> = plan
                .mounts
                .iter()
                .map(|m| {
                    format!(
                        "{:?} {}",
                        m.access,
                        m.path.display()
                    )
                })
                .collect();
            println!("== {label} ==");
            println!("cwd: {}", plan.cwd.display());
            println!("mounts: {mounts:?}");
            #[cfg(target_os = "linux")]
            {
                let args: Vec<String> = bwrap_command(&plan)
                    .get_args()
                    .map(|a| a.to_string_lossy().to_string())
                    .collect();
                println!("bwrap args: {args:?}");
            }
            #[cfg(target_os = "macos")]
            {
                println!("seatbelt profile:\n{}", seatbelt_profile(&plan));
            }
        }
    }

    // ── `[jail]` config defaults ──────────────────────────────────────────
    //
    // These drive the REAL shipped config→plan→profile path: `JailDefaults::load`
    // reads the `[jail]` table off a fixture `config.toml` under an isolated
    // `$GROK_HOME`, and the resulting `JailDefaults` is what the shipped
    // `build_plan`/`seatbelt_profile`/`bwrap_command` consume. None of them re-
    // implement the mapping under test.

    /// Resolve the `[jail]` table of an in-memory config string.
    fn config_defaults(toml: &str) -> JailDefaults {
        let value: toml::Value = toml::from_str(toml).expect("fixture toml parses");
        JailDefaults::from_config(&value)
    }

    const RELEASE: JailDefaults = JailDefaults {
        cwd: Access::Rw,
        grok_home: Access::Rw,
        tmp: TmpHandling::Tmpfs,
        system: Access::Ro,
    };

    #[test]
    fn no_config_yields_the_four_release_defaults() {
        assert_eq!(JailDefaults::default(), RELEASE);
        // No `[jail]` section, or an empty file, still resolves to the release
        // struct that every builder consumes.
        assert_eq!(config_defaults(""), RELEASE);
        assert_eq!(config_defaults("[permission]\nmode = \"accept\""), RELEASE);
        // An unknown `[jail]` key is not a recognized axis and is ignored.
        assert_eq!(config_defaults("[jail]\ndenied = [\".env\"]"), RELEASE);
    }

    #[test]
    fn each_single_override_moves_only_its_axis() {
        // cwd ro.
        let d = config_defaults("[jail]\ncwd = \"ro\"");
        assert_eq!(
            d,
            JailDefaults {
                cwd: Access::Ro,
                ..RELEASE
            }
        );
        // grok home ro (the read-only home), the rest unchanged.
        let d = config_defaults("[jail]\ngrok_home = \"ro\"");
        assert_eq!(
            d,
            JailDefaults {
                grok_home: Access::Ro,
                ..RELEASE
            }
        );
        // /tmp rw bind.
        let d = config_defaults("[jail]\ntmp = \"rw\"");
        assert_eq!(
            d,
            JailDefaults {
                tmp: TmpHandling::Rw,
                ..RELEASE
            }
        );
        // /tmp ro bind.
        let d = config_defaults("[jail]\ntmp = \"ro\"");
        assert_eq!(
            d,
            JailDefaults {
                tmp: TmpHandling::Ro,
                ..RELEASE
            }
        );
        // system rw (weakens the jail — the user's explicit choice).
        let d = config_defaults("[jail]\nsystem = \"rw\"");
        assert_eq!(
            d,
            JailDefaults {
                system: Access::Rw,
                ..RELEASE
            }
        );
    }

    #[test]
    fn all_four_together_and_explicit_rewrite_back_to_release() {
        let d = config_defaults(
            "[jail]\ncwd = \"ro\"\ngrok_home = \"ro\"\ntmp = \"rw\"\nsystem = \"rw\"",
        );
        assert_eq!(
            d,
            JailDefaults {
                cwd: Access::Ro,
                grok_home: Access::Ro,
                tmp: TmpHandling::Rw,
                system: Access::Rw,
            }
        );
        // Explicitly spelling the release values round-trips to release.
        assert_eq!(
            config_defaults("[jail]\ncwd = \"rw\"\ngrok_home = \"rw\"\ntmp = \"tmpfs\"\nsystem = \"ro\""),
            RELEASE
        );
    }

    #[test]
    fn bogus_values_and_unknown_section_keys_fall_back_not_guess() {
        // A misspelled value is a no-op (keeps the release access), never a jail
        // the user did not literally ask for.
        assert_eq!(config_defaults("[jail]\ncwd = \"reads-only\""), RELEASE);
        assert_eq!(config_defaults("[jail]\ntmp = \"bind-mode\"\n"), RELEASE);
        let d = config_defaults("[jail]\ncwd = \"ro\"\nsystem = \"wr\""); // one bad token
        assert_eq!(
            d,
            JailDefaults {
                cwd: Access::Ro,
                ..RELEASE
            }
        );
        // Sanity: grok-build sandbox profile keys (`deny`, `read_write`, profile
        // tables) are not `[jail]` axes and never leak in.
        assert_eq!(config_defaults("[profiles.strict]\ndeny = [\".env\"]"), RELEASE);
    }

    /// A throwaway directory for a fixture `config.toml`, under this process's
    /// temp dir (never a shared fixed path), removed on drop.
    struct FixtureHome(std::path::PathBuf);
    impl FixtureHome {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "grok-jail-config-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).expect("create fixture home");
            FixtureHome(dir)
        }
    }
    impl Drop for FixtureHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_reads_the_config_toml_under_an_isolated_home() {
        let home = FixtureHome::new("reads");
        std::fs::write(
            home.0.join("config.toml"),
            "[sandbox]\ntmp = \"tmpfs\"\n[jail]\ncwd = \"ro\"\ntmp = \"rw\"\nsystem = \"rw\"\n",
        )
        .expect("write config.toml");
        // The `[jail]` table is read; the unrelated `[sandbox]` in the same file
        // (the profile-era vocabulary) is not a `[jail]` axis.
        assert_eq!(
            JailDefaults::load(&home.0),
            JailDefaults {
                cwd: Access::Ro,
                grok_home: Access::Rw,
                tmp: TmpHandling::Rw,
                system: Access::Rw,
            }
        );
    }

    #[test]
    fn load_with_no_file_or_bad_file_keeps_release_defaults() {
        let empty = FixtureHome::new("empty");
        assert_eq!(JailDefaults::load(&empty.0), RELEASE);
        let broken = FixtureHome::new("broken");
        std::fs::write(broken.0.join("config.toml"), "this is = = not toml")
            .expect("write config.toml");
        assert_eq!(JailDefaults::load(&broken.0), RELEASE);
    }

    #[test]
    fn config_cwd_ro_default_mounts_the_cwd_read_only() {
        // cwd default ro, no user mount names the cwd: the injected front mount
        // is read-only. (These run against the real cwd via the shipped builder.)
        let plan = build_plan(
            &parse(&["--sandbox=pathbox"]),
            &JailDefaults {
                cwd: Access::Ro,
                ..RELEASE
            },
            argv(&["--sandbox=pathbox"]),
        )
        .expect("build_plan");
        assert_eq!(
            plan.mounts[0].access,
            Access::Ro,
            "a config cwd=ro must default the injected cwd mount to read-only"
        );
    }

    #[test]
    fn a_cli_flag_beats_the_config_cwd_default() {
        // Config says ro, but --rw . on the line must win.
        let ro_default = JailDefaults {
            cwd: Access::Ro,
            ..RELEASE
        };
        let plan = build_plan(
            &parse(&["--sandbox=pathbox", "--rw", "."]),
            &ro_default,
            argv(&["--sandbox=pathbox", "--rw", "."]),
        )
        .expect("build_plan");
        assert_eq!(
            plan.mounts[0].access,
            Access::Rw,
            "--rw . must beat a config cwd=ro default"
        );
        // Config says rw, but --ro . on the line must win.
        let plan = build_plan(
            &parse(&["--sandbox=pathbox", "--ro", "."]),
            &RELEASE,
            argv(&["--sandbox=pathbox", "--ro", "."]),
        )
        .expect("build_plan");
        assert_eq!(
            plan.mounts[0].access,
            Access::Ro,
            "--ro . must beat a config cwd=rw default"
        );
    }

    #[test]
    fn plan_carries_exactly_the_resolved_defaults_it_was_built_with() {
        let defaults = JailDefaults {
            cwd: Access::Ro,
            grok_home: Access::Ro,
            tmp: TmpHandling::Rw,
            system: Access::Rw,
        };
        let plan = build_plan(
            &parse(&["--sandbox=pathbox"]),
            &defaults,
            argv(&["--sandbox=pathbox"]),
        )
        .expect("build_plan");
        assert_eq!(plan.defaults, defaults);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_grok_home_ro_keeps_read_and_drops_write() {
        // The shipped profile with grok home ro must keep the home readable and
        // emit NO write-allow for it (matching bwrap's --ro-bind).
        let mut plan = plan_fixture(Vec::new());
        plan.defaults = JailDefaults {
            grok_home: Access::Ro,
            ..RELEASE
        };
        let profile = seatbelt_profile(&plan);
        assert!(
            profile.contains("(allow file-read* (subpath \"/home/u/.grok\"))"),
            "an ro grok home stays readable: {profile}"
        );
        assert!(
            !profile.contains("(allow file-write* (subpath \"/home/u/.grok\"))"),
            "an ro grok home must not be writable: {profile}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_release_grok_home_is_writable_and_system_is_never_written() {
        // Release defaults: grok home is writable, the system base is read-only
        // (no write-allow over /usr, /System, ..., by default).
        let plan = plan_fixture(Vec::new());
        let profile = seatbelt_profile(&plan);
        assert!(
            profile.contains("(allow file-write* (subpath \"/home/u/.grok\"))"),
            "release grok home must be writable: {profile}"
        );
        assert!(
            !profile.contains("(allow file-write* (subpath \"/usr\"))"),
            "release system base must never be writable: {profile}"
        );
        assert!(
            !profile.contains("(allow file-write* (subpath \"/System\"))"),
            "release macOS base must never be writable: {profile}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_system_rw_makes_every_base_path_writable_too() {
        // system = "rw" emits a write-allow over the whole platform base (the
        // user's explicit weakening), while keeping the read side present.
        let plan = plan_fixture(Vec::new());
        let ro_profile = seatbelt_profile(&plan);

        let mut rw_plan = plan_fixture(Vec::new());
        rw_plan.defaults.system = Access::Rw;
        let rw_profile = seatbelt_profile(&rw_plan);

        for path in ["/usr", "/opt", "/System", "/Library", "/private"] {
            assert!(
                ro_profile.contains(&format!("(allow file-read* (subpath \"{path}\"))\n")),
                "base {path} read allow missing in release: {ro_profile}"
            );
            assert!(
                !ro_profile.contains(&format!("(allow file-write* (subpath \"{path}\"))\n")),
                "base {path} must not be writable at release: {ro_profile}"
            );
            assert!(
                rw_profile.contains(&format!("(allow file-write* (subpath \"{path}\"))\n")),
                "system=rw must grant writes over {path}: {rw_profile}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn rn_denies_a_subpath_even_under_a_visible_ancestor() {
        // --rn /work/secrets hides the subtree even though /work is an rw mount
        // and the cwd. Seatbelt hides by emitting last-match deny rules.
        let plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/work"),
            },
            Mount {
                access: Access::Deny,
                path: PathBuf::from("/work/secrets"),
            },
        ]);
        let profile = seatbelt_profile(&plan);
        assert!(
            profile.contains("(deny file-read* (subpath \"/work/secrets\"))\n"),
            "a --rn path must be read-denied even under an rw ancestor: {profile}"
        );
        assert!(
            profile.contains("(deny file-write* (subpath \"/work/secrets\"))\n"),
            "a --rn path must be write-denied: {profile}"
        );
        assert!(
            !profile.contains("(allow file-read* (subpath \"/work/secrets\"))\n"),
            "a --rn path must not get a read-allow from the ancestor grant: {profile}"
        );
        // The deny rule must come after the ancestor's allow so last-match wins.
        let allow_work = profile
            .find("(allow file-read* (subpath \"/work\"))\n")
            .expect("ancestor allow");
        let deny_secrets = profile
            .find("(deny file-read* (subpath \"/work/secrets\"))\n")
            .expect("deny rule");
        assert!(allow_work < deny_secrets, "deny must come after the allow: {profile}");
    }

    #[test]
    fn rn_on_the_working_directory_is_refused() {
        // --rn . hides the cwd, but the jail must chdir into it, so refuse.
        let request = parse(&["--rn", "."]);
        let plan = build_plan(&request, &JailDefaults::default(), argv(&["--rn", "."]));
        assert!(
            matches!(plan, Err(JailError::CwdDenied { .. })),
            "denying the cwd must be refused: {plan:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rn_binds_an_empty_read_only_sink_over_the_denied_path() {
        // bwrap has no "unmount": a --rn hides by binding an empty read-only
        // sink over the exact path, grounded on the shared deny-sink dir. The
        // deny bind must come after the ancestor grant so it shadows it.
        let plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/work"),
            },
            Mount {
                access: Access::Deny,
                path: PathBuf::from("/work/secrets"),
            },
        ]);
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let sink = plan.deny_sink.to_string_lossy().to_string();
        let rw = args
            .windows(3)
            .position(|w| w == ["--bind", "/work", "/work"])
            .expect("rw bind");
        let deny = args
            .windows(3)
            .position(|w| w == ["--ro-bind", &sink, "/work/secrets"])
            .expect("deny sink bind");
        assert!(
            rw < deny,
            "deny bind must come after the ancestor grant: {args:?}"
        );
        assert!(
            !args.windows(3).any(|w| w == ["--bind", "/work/secrets", "/work/secrets"]),
            "a --rn path must not be bound rw: {args:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn bwrap_defaults_stay_ro_base_tmpfs_tmp_and_rw_grok_home() {
        // The linux builder keeps the release contract.
        let plan = plan_fixture(Vec::new());
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3).any(|w| w[0] == "--ro-bind-try" && w[1] == "/usr"),
            "release system base must be ro-bind-try: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]),
            "release /tmp must be a tmpfs: {args:?}"
        );
        assert!(
            args.windows(3).any(|w| w == ["--bind", "/home/u/.grok", "/home/u/.grok"]),
            "release grok home must be bound rw: {args:?}"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn bwrap_overrides_change_only_their_bind() {
        let mut plan = plan_fixture(Vec::new());
        plan.defaults = JailDefaults {
            system: Access::Rw,
            tmp: TmpHandling::Rw,
            grok_home: Access::Ro,
            ..RELEASE
        };
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            args.windows(3).any(|w| w == ["--bind", "/usr", "/usr"]),
            "system=rw must bind the base rw: {args:?}"
        );
        assert!(
            args.windows(3).any(|w| w == ["--bind", "/tmp", "/tmp"]),
            "tmp=rw must bind /tmp: {args:?}"
        );
        assert!(
            args.windows(3).any(|w| w == ["--ro-bind", "/home/u/.grok", "/home/u/.grok"]),
            "grok_home=ro must --ro-bind the home: {args:?}"
        );
        assert!(
            !args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]),
            "tmp=rw must not leave the tmpfs: {args:?}"
        );
    }
}
