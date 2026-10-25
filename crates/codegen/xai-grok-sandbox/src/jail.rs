//! `--sandbox=pathbox` / `--ro`/`--rw`/`--rn`: replace this process with itself inside an OS jail ("pathbox").

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Set on the jailed process. Its presence stops a second re-exec.
pub const JAIL_ENV_VAR: &str = "__GROK_SANDBOX_JAIL";
/// The marker `is_inside_bwrap` reads.
const BWRAP_ENV_VAR: &str = "__GROK_INSIDE_BWRAP";
/// Read-only system paths the jail binds so the process can execute at all.
/// Bound first, so a user mount can override any of them.
const SYSTEM_RO_BASE: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/run", "/var",
];

/// Extra read-only system paths macOS needs that the Linux base.
#[cfg(target_os = "macos")]
const SYSTEM_RO_BASE_MACOS: &[&str] = &["/System", "/Library", "/private"];

/// The read-only system base, platform-appropriate.
#[cfg(target_os = "macos")]
fn system_ro_base() -> impl Iterator<Item = &'static str> {
    SYSTEM_RO_BASE
        .iter()
        .copied()
        .chain(SYSTEM_RO_BASE_MACOS.iter().copied())
}
/// Where the jail's `/tmp` overlay or tmpfs is mounted on Linux.
const JAIL_TMP: &str = "/tmp";
/// The real directory behind `/tmp`. Seatbelt matches resolved paths.
#[cfg(target_os = "macos")]
const HOST_TMP: &str = "/private/tmp";

/// How a path is bound inside the jail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Readable, not writable.
    Ro,
    /// Readable and writable.
    Rw,
    /// Actively denied (hidden): not readable and not writable.
    Deny,
}

/// One mount the jail will make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub access: Access,
    pub path: PathBuf,
}

/// How a scratch directory (`/tmp`, `~/.cache`) is exposed in the jail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpHandling {
    /// The host directory as a lower layer under a writable tmpfs. The jail reads the host files.
    Overlay,
    /// A fresh, empty, writable tmpfs.
    Tmpfs,
    /// Bind the host `/tmp` in read-write.
    Rw,
    /// Bind the host `/tmp` read-only.
    Ro,
}

/// The per-path defaults the `--sandbox` re-exec jail applies where the
/// command line is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JailDefaults {
    /// Working-directory access when no user `--ro`/`--rw` covers it.
    pub cwd: Access,
    /// `$GROK_HOME` access (kept, like today, bound after the user mounts — a config-granted ro is honoured, a CLI flag cannot take it below that).
    pub grok_home: Access,
    /// `/tmp` handling.
    pub tmp: TmpHandling,
    /// `~/.cache` (and `$XDG_CACHE_HOME`) handling.
    pub cache: TmpHandling,
    /// The read-only system base (`/usr`, `/lib`, … and the macOS system dirs).
    pub system: Access,
}

impl Default for JailDefaults {
    fn default() -> Self {
        Self {
            cwd: Access::Rw,
            grok_home: Access::Rw,
            tmp: TmpHandling::Overlay,
            cache: TmpHandling::Overlay,
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
    Cache,
    System,
}

/// Map a `[jail]` config key name onto its enum so we resolve each key the same
/// way (`cwd`/`grok_home`/`system` take `ro`/`rw`, `tmp`/`cache` take a
/// [`TmpHandling`] token).
fn default_key(name: &str) -> Option<DefaultKey> {
    match name {
        "cwd" => Some(DefaultKey::Cwd),
        "grok_home" => Some(DefaultKey::GrokHome),
        "tmp" => Some(DefaultKey::Tmp),
        "cache" => Some(DefaultKey::Cache),
        "system" => Some(DefaultKey::System),
        _ => None,
    }
}

/// The [`TmpHandling`] a `[jail]` token names.
fn scratch_token(token: &str) -> Option<TmpHandling> {
    match token {
        "overlay" => Some(TmpHandling::Overlay),
        "tmpfs" => Some(TmpHandling::Tmpfs),
        "rw" => Some(TmpHandling::Rw),
        "ro" => Some(TmpHandling::Ro),
        _ => None,
    }
}

impl JailDefaults {
    /// The defaults from one `[jail]` config value. A key that is absent, not
    /// a The defaults from one `[jail]` config value. A key that is absent,
    /// not a string, or holds a string we do not recognize keeps the release
    /// default. A bad token is a no-op rather than a jail that silently
    /// grants more (or refuses) than the user wrote meaningful words for.
    /// Unknown section keys are ignored the same way — they may be reserved
    /// by `--sandbox` profile config or written by a newer build.
    pub fn from_config(config: &toml::Value) -> JailDefaults {
        let mut defaults = JailDefaults::default();
        let Some(table) = config.get("jail").and_then(toml::Value::as_table) else {
            return defaults;
        };
        for (key, value) in table {
            let Some(kind) = default_key(key) else {
                continue;
            };
            let Some(token) = value.as_str() else {
                continue;
            };
            match (kind, token) {
                (DefaultKey::Cwd, "ro") => defaults.cwd = Access::Ro,
                (DefaultKey::Cwd, "rw") => defaults.cwd = Access::Rw,
                (DefaultKey::GrokHome, "ro") => defaults.grok_home = Access::Ro,
                (DefaultKey::GrokHome, "rw") => defaults.grok_home = Access::Rw,
                (DefaultKey::System, "ro") => defaults.system = Access::Ro,
                (DefaultKey::System, "rw") => defaults.system = Access::Rw,
                (DefaultKey::Tmp, token) => {
                    if let Some(handling) = scratch_token(token) {
                        defaults.tmp = handling;
                    }
                }
                (DefaultKey::Cache, token) => {
                    if let Some(handling) = scratch_token(token) {
                        defaults.cache = handling;
                    }
                }
                _ => {}
            }
        }
        defaults
    }

    /// Read the `[jail]` defaults from `<home>/config.toml`. A missing or
    /// unparsable file yields the release defaults (fail open to the
    /// historical behavior, never to a `[jail]`-free accident). The built-in
    /// grok-build sandbox file `sandbox.toml` is deliberately never consulted
    /// here.
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
    /// Whether a path-box jail was asked for: `--sandbox=pathbox` or any `--ro`/`--rw`/`--rn` path flag.
    pub enabled: bool,
    /// User mounts, in command-line order.
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
    #[error(
        "the path '{path}' is denied by `--rn`, but it is the working \
         directory; the jail cannot start a process in a hidden cwd"
    )]
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
    NoTempDir {
        path: String,
        source: std::io::Error,
    },
    #[error("`{0}` is not installed, so --sandbox cannot confine this process")]
    MissingBackend(&'static str),
    #[error("--sandbox is not supported on this platform")]
    UnsupportedPlatform,
}

/// Whether this process is already inside a jail this module built.
pub fn is_jailed() -> bool {
    std::env::var_os(JAIL_ENV_VAR).is_some()
}

/// The reserved name of the path-mount jail profile.
pub const PATHBOX_PROFILE: &str = "pathbox";

/// Read `--sandbox`, `--ro`, `--rw` and `--rn` off the raw command line. The
/// raw line is what carries the order of the visible mounts; a parsed struct
/// groups the flags into separate lists and loses it. The path-mount jail is
/// enabled only when `--sandbox=pathbox` is given, OR any of
/// `--ro`/`--rw`/`--rn` appears (the jail is then implied). A bare `--sandbox`
/// with no value is always invalid, and a `--sandbox <profile>` naming
/// anything other than `pathbox` is the built-in profile sandbox (not a jail).
/// It cannot be combined with path flags. Scanning stops at a bare `--`, so a
/// prompt is never read as a flag.
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
        // clap's "next non-flag token is the value" so both agree.
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
    // line did not authorize.
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
    /// User mounts, in order, canonicalized, with the working directory mounted at the FRONT.
    pub mounts: Vec<Mount>,
    /// `$GROK_HOME`. Bound read-write after the user mounts.
    pub grok_home: PathBuf,
    /// The scratch directory the jail dedicates to this process.
    pub temp_dir: PathBuf,
    /// An empty, read-only host directory used as the bwrap sink for `--rn` denied paths.
    pub deny_sink: PathBuf,
    /// The binary to run inside the jail.
    pub self_exe: PathBuf,
    /// The directory the jailed process starts in.
    pub cwd: PathBuf,
    /// Arguments for the jailed binary, without `argv[0]`.
    pub args: Vec<OsString>,
    /// The per-path defaults resolved from `[jail]` config (or those release defaults).
    pub defaults: JailDefaults,
    /// The inherited fd of the unsandboxed CI host worker, when one was started for this jail.
    pub ci_host_fd: Option<i32>,
    /// The host cache directories (`~/.cache`, `$XDG_CACHE_HOME`), canonical, in that order of preference.
    pub cache_dirs: Vec<PathBuf>,
    /// Whether bwrap can mount an overlay here. When false, an overlay scratch directory is a tmpfs.
    pub overlay: bool,
    /// macOS: the session directory `XDG_CACHE_HOME` points at. Seatbelt cannot mount, so this holds the cache overlay.
    pub cache_redirect: Option<PathBuf>,
}

/// Resolve a request into a plan. Every path must exist: a missing bind is a hole the caller cannot see. `defaults` is the config layer (see [`JailDefaults`]) that supplies the per-path access where the command line is silent. The runtime caller loads it via [`JailDefaults::load`], tests pass an explicit value so they never depend on the host's `config.toml`. The working directory is bound read-write by default, so a jail built with the pathbox marker (`--sandbox=pathbox`). Otherwise, a path flag can write in the cwd without an explicit `--rw .`. The effective cwd access is the LAST user mount that contains the cwd (`defaults.cwd` when none does). The effective cwd access also is that single cwd mount is placed at the FRONT of `plan.mounts`. Front-placement plus the effective access is what lets `--ro .` win cleanly: the front cwd mount then carries `Access::Ro`. The Seatbelt profile emits no stale write-allow for the cwd and bwrap binds it read-only. `parse_jail_args` is untouched — the default is a plan-level injection, not a flag.
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
    // chdir into the cwd to run.
    if mounts
        .iter()
        .any(|mount| mount.access == Access::Deny && cwd.starts_with(&mount.path))
    {
        return Err(JailError::CwdDenied {
            path: cwd.display().to_string(),
        });
    }
    // The effective cwd access comes from the LAST user mount that contains
    // the cwd (`cwd.starts_with(mount.path)`).
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
    let cache_dirs = host_cache_dirs()?;
    // A host sink for `--rn` denoted paths.
    let deny_sink = session_dir().join("deny-sink");
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
        // The CI host worker is spawned by the caller that is about to exec, moments before the jail is built.
        ci_host_fd: None,
        cache_dirs,
        // The runtime caller probes bwrap and sets these. A plan never mounts an overlay it has not proven.
        overlay: false,
        cache_redirect: None,
    };
    plan.check_cwd_is_bound()?;
    Ok(plan)
}

/// This process's directory under `$GROK_HOME/sandbox-tmp`.
fn session_dir() -> PathBuf {
    crate::paths::grok_home()
        .join("sandbox-tmp")
        .join(std::process::id().to_string())
}

/// The host cache directories the jail never shows writable:
/// `$XDG_CACHE_HOME` when set, then `~/.cache`.
fn host_cache_dirs() -> Result<Vec<PathBuf>, JailError> {
    cache_dirs_for(
        xai_dirs::home_dir(),
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
    )
}

/// Creates each directory, because an overlay needs a lower layer. Skips one
/// that contains home: an overlay there exposes all of home.
fn cache_dirs_for(home: Option<PathBuf>, xdg: Option<PathBuf>) -> Result<Vec<PathBuf>, JailError> {
    let xdg = xdg.filter(|path| path.is_absolute());
    let dot_cache = home.as_ref().map(|home| home.join(".cache"));
    let home = home.map(|home| dunce::canonicalize(&home).unwrap_or(home));
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in xdg.into_iter().chain(dot_cache) {
        std::fs::create_dir_all(&dir).map_err(|source| JailError::NoTempDir {
            path: dir.display().to_string(),
            source,
        })?;
        let dir = dunce::canonicalize(&dir).unwrap_or(dir);
        if home.as_ref().is_some_and(|home| home.starts_with(&dir)) || dirs.contains(&dir) {
            continue;
        }
        dirs.push(dir);
    }
    Ok(dirs)
}

impl JailPlan {
    /// Refuse a jail whose working directory nothing binds. bwrap answers that
    /// with a bare chdir error, which reads as a broken sandbox rather than a
    /// missing `--rw`.
    fn check_cwd_is_bound(&self) -> Result<(), JailError> {
        // The temp directory is compared exactly: on Linux it can be a fresh
        // tmpfs. A path under it then exists on the host and not in the jail.
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

    /// The scratch directories (`tmp`, then the caches) with the user-mount
    /// index each goes before. A scratch directory goes right after the last
    /// mount that strictly contains it. So `--rw ~` (or a cwd of `~`) cannot
    /// expose the host `~/.cache`, while a mount on the directory itself or
    /// below it still wins. The order is by slot, then by depth, so a nested
    /// scratch directory goes over its parent.
    fn scratch_slots(&self, tmp: &Path) -> Vec<(usize, PathBuf, TmpHandling)> {
        let mut slots: Vec<(usize, PathBuf, TmpHandling)> =
            std::iter::once((tmp.to_path_buf(), self.defaults.tmp))
                .chain(
                    self.cache_dirs
                        .iter()
                        .map(|dir| (dir.clone(), self.defaults.cache)),
                )
                .map(|(path, handling)| {
                    let slot = self
                        .mounts
                        .iter()
                        .rposition(|mount| {
                            mount.access != Access::Deny
                                && path != mount.path
                                && path.starts_with(&mount.path)
                        })
                        .map_or(0, |index| index + 1);
                    (slot, path, handling)
                })
                .collect();
        slots.sort_by_key(|(slot, path, _)| (*slot, path.components().count()));
        slots
    }
}

/// A fresh scratch directory for this process. On Linux the jail mounts an
/// overlay or a tmpfs over `/tmp` and this directory is that mount, so it
/// never touches the host. On macOS Seatbelt mounts nothing, so the directory
/// is a real, empty one under `$GROK_HOME` and `TMPDIR` points at it.
fn dedicated_temp_dir() -> Result<PathBuf, JailError> {
    if cfg!(target_os = "linux") {
        return Ok(PathBuf::from(JAIL_TMP));
    }
    let path = session_dir().join("tmp");
    std::fs::create_dir_all(&path).map_err(|source| JailError::NoTempDir {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// Build the `bwrap` command that runs the plan. Order is the whole contract:
/// the read-only system base first, then the user mounts as given, then
/// `$GROK_HOME`. bwrap applies binds in order and a later one covers an
/// earlier one, so this is what makes a later `--ro` beat an earlier `--rw`.
/// The access each synthesized bind carries comes from [`JailPlan::defaults`]
/// where a user mount does not name it. This covers the system base from
/// `system`, `/tmp` from `tmp`, the caches from `cache`, and `$GROK_HOME`
/// from `grok_home`. `/tmp` and the caches go among the user mounts, at the
/// slot [`JailPlan::scratch_slots`] picks. Compiled under
/// `cfg(test)` off Linux as well. Because the emitted argv IS the contract
/// (the option order is what makes a later bind win, and what keeps the CI
/// host-worker fd an option rather than a program argument) and an ordering
/// only one host can assert is one that regresses quietly everywhere else.
#[cfg(any(target_os = "linux", test))]
pub fn bwrap_command(plan: &JailPlan) -> std::process::Command {
    // No --die-with-parent: it kills the jail when bwrap's parent dies.
    let mut cmd = std::process::Command::new("bwrap");
    cmd.arg("--cap-drop").arg("ALL");
    // The platform read-only base.
    let system_flag = match plan.defaults.system {
        Access::Ro | Access::Deny => "--ro-bind-try",
        Access::Rw => "--bind",
    };
    for path in SYSTEM_RO_BASE {
        cmd.arg(system_flag).arg(path).arg(path);
    }
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--dev").arg("/dev");
    // `--dev` builds a fresh /dev without /dev/shm.
    cmd.arg("--tmpfs").arg("/dev/shm");
    let mut scratch = plan
        .scratch_slots(Path::new(JAIL_TMP))
        .into_iter()
        .peekable();
    for (index, mount) in plan.mounts.iter().enumerate() {
        while let Some((_, path, handling)) = scratch.next_if(|(slot, ..)| *slot == index) {
            push_bwrap_scratch(&mut cmd, &path, handling, plan.overlay);
        }
        match mount.access {
            Access::Ro => {
                cmd.arg("--ro-bind").arg(&mount.path).arg(&mount.path);
            }
            Access::Rw => {
                cmd.arg("--bind").arg(&mount.path).arg(&mount.path);
            }
            // A deny is not bound here.
            Access::Deny => {}
        }
    }
    for (_, path, handling) in scratch {
        push_bwrap_scratch(&mut cmd, &path, handling, plan.overlay);
    }
    // `$GROK_HOME`, bound after the user mounts. A config `grok_home = "ro"`
    // binds it read-only; the release default (`rw`) is unchanged. Because it
    // is bound last it survives a user `--ro` aimed at it either way.
    match plan.defaults.grok_home {
        Access::Rw => {
            cmd.arg("--bind").arg(&plan.grok_home).arg(&plan.grok_home);
        }
        Access::Ro | Access::Deny => {
            cmd.arg("--ro-bind")
                .arg(&plan.grok_home)
                .arg(&plan.grok_home);
        }
    }
    cmd.arg("--ro-bind-try")
        .arg(&plan.self_exe)
        .arg(&plan.self_exe);
    // `--rn` denies hide their subtree *after* every visible bind so they
    // shadow even a containing `--rw`/`--ro` ancestor.
    for mount in &plan.mounts {
        if mount.access == Access::Deny {
            cmd.arg("--ro-bind").arg(&plan.deny_sink).arg(&mount.path);
        }
    }
    cmd.arg("--chdir").arg(&plan.cwd);
    cmd.arg("--setenv").arg(JAIL_ENV_VAR).arg("1");
    cmd.arg("--setenv").arg(BWRAP_ENV_VAR).arg("1");
    // The host-worker fd, as a bwrap env setting and therefore BEFORE `--`.
    if let Some(fd) = plan.ci_host_fd {
        cmd.arg("--setenv")
            .arg(crate::ci_host::CI_HOST_FD_ENV)
            .arg(fd.to_string());
    }
    cmd.arg("--").arg(&plan.self_exe).args(&plan.args);
    cmd
}

/// Mount one scratch directory. `rw` and `ro` bind the host directory, so a
/// granted path (e.g. an ssh control socket in `/tmp`) reaches the jail.
#[cfg(any(target_os = "linux", test))]
fn push_bwrap_scratch(
    cmd: &mut std::process::Command,
    path: &Path,
    handling: TmpHandling,
    overlay: bool,
) {
    match handling {
        TmpHandling::Overlay if overlay => {
            cmd.arg("--overlay-src")
                .arg(path)
                .arg("--tmp-overlay")
                .arg(path);
        }
        TmpHandling::Overlay | TmpHandling::Tmpfs => {
            cmd.arg("--tmpfs").arg(path);
        }
        TmpHandling::Rw => {
            cmd.arg("--bind").arg(path).arg(path);
        }
        TmpHandling::Ro => {
            cmd.arg("--ro-bind").arg(path).arg(path);
        }
    }
}

/// Build the Seatbelt profile for the plan. Seatbelt confines READS and
/// WRITES here, mirroring bwrap.
#[cfg(target_os = "macos")]
pub fn seatbelt_profile(plan: &JailPlan) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str("(deny file-read*)\n(deny file-write*)\n");
    // A terminal, a PTY and /dev/null are reads and writes every tool makes.
    profile.push_str("(allow file-read* (subpath \"/dev\"))\n");
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    // The root itself.
    profile.push_str("(allow file-read* (literal \"/\"))\n");
    // Path resolution stats each ANCESTOR of every path it is handed, so a
    // grant whose ancestors are denied is unreachable in practice: `getcwd`
    // fails. A tool cannot resolve a path it was given, even inside a mounted
    // directory. Grant metadata on the ancestors — literal rules, one per
    // directory, never a subtree — so the granted trees resolve while a path
    // nobody granted stays. As invisible as bwrap leaves it.
    for path in std::iter::once(&plan.self_exe)
        .chain(
            plan.mounts
                .iter()
                .filter_map(|mount| (mount.access != Access::Deny).then_some(&mount.path)),
        )
        .chain([&plan.grok_home, &plan.temp_dir])
        .chain(&plan.cache_dirs)
        .chain(&plan.cache_redirect)
    {
        for ancestor in path
            .ancestors()
            .skip(1)
            .filter(|dir| *dir != Path::new("/"))
        {
            profile.push_str(&format!(
                "(allow file-read-metadata (literal \"{}\"))\n",
                sbpl_escape(ancestor)
            ));
        }
    }
    // The binary sandbox-exec execs. bwrap binds it read-only explicitly.
    profile.push_str(&format!(
        "(allow file-read* (literal \"{}\"))\n",
        sbpl_escape(&plan.self_exe)
    ));
    // The read-only system base the process needs to run (dylibs, binaries,
    // config, and on macOS the real /private home of /etc, /var and /tmp).
    // Emitted before the user mounts, matching bwrap's bind order (base
    // first, then user mounts, then $GROK_HOME) so a user mount over a base
    // path wins.
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
    let mut scratch = plan
        .scratch_slots(Path::new(HOST_TMP))
        .into_iter()
        .peekable();
    for (index, mount) in plan.mounts.iter().enumerate() {
        while let Some((_, path, handling)) = scratch.next_if(|(slot, ..)| *slot == index) {
            push_seatbelt_scratch(&mut profile, &path, handling);
        }
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
    for (_, path, handling) in scratch {
        push_seatbelt_scratch(&mut profile, &path, handling);
    }
    profile.push_str(&format!(
        "(allow file-read* (subpath \"{}\"))\n",
        sbpl_escape(&plan.grok_home)
    ));
    // `$GROK_HOME` is writable by release default; `grok_home = "ro"` leaves
    // it readable only, matching the bwrap `--ro-bind`.
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
    if let Some(cache) = &plan.cache_redirect {
        profile.push_str(&format!(
            "(allow file-read* (subpath \"{0}\"))\n(allow file-write* (subpath \"{0}\"))\n",
            sbpl_escape(cache)
        ));
    }
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

/// The rules for one scratch directory. Seatbelt cannot mount, so the host
/// directory is never writable in the jail. `overlay` keeps it readable, as
/// the lower layer. `tmpfs` hides it. The writable upper layer is the
/// session directory `TMPDIR` or `XDG_CACHE_HOME` points at.
#[cfg(target_os = "macos")]
fn push_seatbelt_scratch(profile: &mut String, path: &Path, handling: TmpHandling) {
    let path = sbpl_escape(path);
    let rules = match handling {
        TmpHandling::Overlay | TmpHandling::Ro => ["allow file-read*", "deny file-write*"],
        TmpHandling::Tmpfs => ["deny file-read*", "deny file-write*"],
        TmpHandling::Rw => ["allow file-read*", "allow file-write*"],
    };
    for rule in rules {
        profile.push_str(&format!("({rule} (subpath \"{path}\"))\n"));
    }
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
    if let Some(cache) = &plan.cache_redirect {
        cmd.env("XDG_CACHE_HOME", cache);
    }
    // Seatbelt inherits the environment, so the host-worker fd is delivered as
    // a command env setting — never as an argument of the jailed program.
    if let Some(fd) = plan.ci_host_fd {
        cmd.env(crate::ci_host::CI_HOST_FD_ENV, fd.to_string());
    }
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
/// asked for one. Returns on the paths that need no jail: no `--sandbox`, or
/// a process that is inside one already. Every other outcome ends the process
/// — either the exec succeeds and this function never returns. Otherwise,
/// the jail could not be built and starting unconfined would be a lie.
pub fn maybe_reexec_into_jail() {
    if is_jailed() {
        return;
    }
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let request = match parse_jail_args(argv.clone()) {
        Ok(request) => request,
        Err(e) => fail(&e.to_string()),
    };
    // Honor `GROK_SANDBOX=pathbox` (clap reads the same env into `--sandbox`, but the raw job must re-exec before clap runs, so it reads the env too).
    let env_pathbox = std::env::var("GROK_SANDBOX").ok().as_deref() == Some(PATHBOX_PROFILE);
    if !request.enabled && !env_pathbox {
        return;
    }
    // Read the `[jail]` defaults from this process's `$GROK_HOME`/`config.toml` (release defaults when there is no `[jail]` section), then drive the real plan/service builders.
    let defaults = JailDefaults::load(&crate::paths::grok_home());
    sweep_stale_session_dirs(&crate::paths::grok_home());
    let mut plan = match build_plan(&request, &defaults, argv) {
        Ok(plan) => plan,
        Err(e) => fail(&e.to_string()),
    };
    #[cfg(target_os = "linux")]
    {
        plan.overlay = bwrap_can_overlay(&plan);
    }
    #[cfg(target_os = "macos")]
    {
        plan.cache_redirect = match prepare_cache_redirect(&plan) {
            Ok(dir) => dir,
            Err(e) => fail(&e.to_string()),
        };
    }
    // Start the unsandboxed `gh` CI-status worker *before* the exec so its stream fd survives into the jail.
    plan.ci_host_fd = crate::ci_host::spawn_ci_host(&plan.cwd);
    // Last before the exec, so no other helper inherits the pipe and delays the removal.
    #[cfg(unix)]
    if spawn_session_reaper(&session_dir()).is_none() {
        eprintln!(
            "warning: --sandbox: could not start the cleanup process; {} is removed at the next jail start instead",
            session_dir().display()
        );
    }
    let mut cmd = match backend_command(&plan) {
        Ok(cmd) => cmd,
        Err(e) => fail(&e.to_string()),
    };
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

/// Start a detached process that removes `dir` when the jail closes. The
/// process reads a pipe whose write end the jail inherits, together with
/// every process the jail starts. It sees EOF only after all of them exit,
/// a crash and SIGKILL included. Returns the write end, left open across exec.
#[cfg(unix)]
fn spawn_session_reaper(dir: &Path) -> Option<std::os::unix::io::RawFd> {
    use std::os::unix::io::{AsRawFd as _, IntoRawFd as _};
    use std::os::unix::process::CommandExt as _;
    let (reader, writer) = std::io::pipe().ok()?;
    crate::ci_host::inherit_across_exec(writer.as_raw_fd())?;
    let writer_fd = writer.as_raw_fd();
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", "cat >/dev/null; exec rm -rf -- \"$1\"", "sh"])
        .arg(dir)
        .stdin(reader)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: `close` and `setsid` are async-signal-safe. The reaper must
    // hold no write end, or it never sees EOF.
    unsafe {
        cmd.pre_exec(move || {
            libc::close(writer_fd);
            libc::setsid();
            Ok(())
        });
    }
    // The reaper outlives this process by design: the exec replaces it.
    #[allow(clippy::disallowed_methods)]
    cmd.spawn().ok()?;
    Some(writer.into_raw_fd())
}

/// Remove the session directories of jails whose process is gone. On macOS
/// they hold the `TMPDIR` and cache upper layers, which must not outlive the
/// session the way a tmpfs does not.
fn sweep_stale_session_dirs(grok_home: &Path) {
    let Ok(entries) = std::fs::read_dir(grok_home.join("sandbox-tmp")) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() || process_is_alive(pid) {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(entry.path()) {
            eprintln!(
                "warning: --sandbox: could not remove the stale session directory {}: {e}",
                entry.path().display()
            );
        }
    }
}

/// Whether `pid` names a live process.
#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; it only checks that the pid exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

/// Whether bwrap can mount every overlay the plan asks for. When it
/// fails, the overlays become tmpfs mounts and this says so on stderr.
#[cfg(target_os = "linux")]
fn bwrap_can_overlay(plan: &JailPlan) -> bool {
    let overlays: Vec<PathBuf> = plan
        .scratch_slots(Path::new(JAIL_TMP))
        .into_iter()
        .filter(|(_, _, handling)| *handling == TmpHandling::Overlay)
        .map(|(_, path, _)| path)
        .collect();
    if overlays.is_empty() {
        return false;
    }
    let mut probe = std::process::Command::new("bwrap");
    probe.args(["--ro-bind", "/", "/"]);
    for path in &overlays {
        probe
            .arg("--overlay-src")
            .arg(path)
            .arg("--tmp-overlay")
            .arg(path);
    }
    probe
        .args(["--", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let reason = match probe.output() {
        Ok(output) if output.status.success() => return true,
        Ok(output) => String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("bwrap failed with no message")
            .to_string(),
        Err(e) => e.to_string(),
    };
    let names: Vec<String> = overlays.iter().map(|p| p.display().to_string()).collect();
    eprintln!(
        "warning: --sandbox: {} are empty tmpfs mounts, not overlays of the host directories: {reason}",
        names.join(", ")
    );
    false
}

/// Make the directory `XDG_CACHE_HOME` points at in a macOS jail. `overlay`
/// makes it an APFS copy-on-write clone of the host cache. The jail reads the
/// host files, and its writes land only in the clone. `tmpfs` makes it empty.
/// `rw` and `ro` redirect nothing.
#[cfg(target_os = "macos")]
fn prepare_cache_redirect(plan: &JailPlan) -> Result<Option<PathBuf>, JailError> {
    if !matches!(
        plan.defaults.cache,
        TmpHandling::Overlay | TmpHandling::Tmpfs
    ) {
        return Ok(None);
    }
    let target = session_dir().join("cache");
    let no_dir = |source| JailError::NoTempDir {
        path: target.display().to_string(),
        source,
    };
    // A pid reused from a dead session can leave one behind.
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(no_dir)?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(no_dir)?;
    }
    if plan.defaults.cache == TmpHandling::Overlay
        && let Some(source) = plan.cache_dirs.first()
    {
        match clone_tree(source, &target) {
            Ok(()) => return Ok(Some(target)),
            Err(e) => {
                eprintln!(
                    "warning: --sandbox: XDG_CACHE_HOME is an empty directory, not a \
                     copy-on-write clone of {}: {e}",
                    source.display()
                );
                if target.exists() {
                    std::fs::remove_dir_all(&target).map_err(no_dir)?;
                }
            }
        }
    }
    std::fs::create_dir_all(&target).map_err(no_dir)?;
    Ok(Some(target))
}

/// Clone a directory tree with `clonefile(2)`. On APFS each file shares its
/// blocks with the source until one side writes it.
#[cfg(target_os = "macos")]
fn clone_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both pointers are NUL-terminated paths that live across the call.
    if unsafe { libc::clonefile(source.as_ptr(), target.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    Err(std::io::Error::last_os_error())
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
        // Any of those path flags alone implies the pathbox jail.
        assert!(parse(&["--rn", "/a"]).enabled);
        assert!(parse(&["--ro", "/a"]).enabled);
        assert!(parse(&["--rw", "/a"]).enabled);
        // A path flag beside an explicit pathbox marker is fine (redundant).
        assert!(parse(&["--sandbox=pathbox", "--rn", "/a/secrets"]).enabled);
    }

    #[test]
    fn bare_sandbox_is_invalid_everywhere() {
        // A bare `--sandbox` (no value) no longer means "the jail" — it is an error even when path flags are present.
        assert!(parse_jail_args(argv(&["--sandbox"])).is_err());
        assert!(parse_jail_args(argv(&["--sandbox="])).is_err());
        assert!(parse_jail_args(argv(&["--sandbox", "--ro", "."])).is_err());
        assert!(parse_jail_args(argv(&["--rn", "/s", "--sandbox"])).is_err());
        // `--sandbox` followed by a prompt/path token is the profile value.
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
            ci_host_fd: None,
            cache_dirs: vec![PathBuf::from("/home/u/.cache")],
            overlay: false,
            cache_redirect: None,
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

    /// The host-worker fd must reach the jailed process as a bwrap `--setenv`
    /// option, BEFORE the `--` program separator. Emitted after it, bwrap reads
    /// it as argv for the jailed binary instead. The env var then never arrives
    /// (so the dot reports no CI) and the pager starts with stray arguments.
    #[test]
    fn bwrap_hands_the_ci_host_fd_to_the_jail_before_the_program_separator() {
        let mut plan = plan_fixture(vec![]);
        plan.ci_host_fd = Some(7);
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let separator = args
            .iter()
            .position(|a| a == "--")
            .expect("bwrap needs its program separator");
        let override_at = args
            .windows(3)
            .position(|w| w[0] == "--setenv" && w[1] == crate::ci_host::CI_HOST_FD_ENV)
            .expect("the host worker's fd must be handed to the jail");
        assert_eq!(
            args[override_at + 2],
            "7",
            "the value must be the fd number the worker was spawned on"
        );
        assert!(
            override_at < separator,
            "bwrap stops reading its own options at `--`, so the fd override must \
             come before it: {args:?}"
        );
        assert!(
            !args[separator + 1..]
                .iter()
                .any(|a| a == crate::ci_host::CI_HOST_FD_ENV || a == "--setenv"),
            "the fd is a jail setting, never an argument of the jailed program: {args:?}"
        );
    }

    /// With no worker there is nothing to hand over, and the jail must not
    /// invent an fd for the pager to read.
    #[test]
    fn bwrap_emits_no_ci_fd_override_without_a_host_worker() {
        let plan = plan_fixture(vec![]);
        let args: Vec<String> = bwrap_command(&plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            !args.iter().any(|a| a == crate::ci_host::CI_HOST_FD_ENV),
            "no worker means no fd to advertise: {args:?}"
        );
    }

    /// The defect this fixes, on the record. Prints the fixed argv, and beside it
    /// the argv the pre-fix caller produced: the SAME builder for a plan with no
    /// fd. That argv is with the identical override appended to the finished
    /// command. There it sits after `--`, so bwrap hands it to the jailed binary
    /// as arguments and never sets the variable. Output only — the assertions
    /// live in the tests above.
    #[test]
    fn capture_ci_fd_argv() {
        let mut with_fd = plan_fixture(vec![]);
        with_fd.ci_host_fd = Some(7);
        let fixed: Vec<String> = bwrap_command(&with_fd)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        let no_fd = plan_fixture(vec![]);
        let mut pre_fix = bwrap_command(&no_fd);
        pre_fix
            .arg("--setenv")
            .arg(crate::ci_host::CI_HOST_FD_ENV)
            .arg("7");
        let pre_fix: Vec<String> = pre_fix
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        eprintln!("fixed argv:   {fixed:?}");
        eprintln!("pre-fix argv: {pre_fix:?}");
    }

    /// Seatbelt inherits the environment, so the same fd rides as a command env
    /// setting — never as an argument the jailed pager would have to ignore.
    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_hands_the_ci_host_fd_to_the_jail_as_a_command_env() {
        let mut plan = plan_fixture(vec![]);
        plan.ci_host_fd = Some(7);
        let cmd = seatbelt_command(&plan);
        let value = cmd
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == crate::ci_host::CI_HOST_FD_ENV)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(
            value.as_deref(),
            Some("7"),
            "the jailed pager reads the fd from this env var"
        );
        assert!(
            !cmd.get_args()
                .any(|arg| arg.to_string_lossy() == crate::ci_host::CI_HOST_FD_ENV),
            "the fd must never be an argument of the jailed program"
        );
        plan.ci_host_fd = None;
        assert!(
            seatbelt_command(&plan)
                .get_envs()
                .all(|(key, _)| key.to_string_lossy() != crate::ci_host::CI_HOST_FD_ENV),
            "no worker means no fd to advertise"
        );
    }

    /// Regression guards for the rules that make the jail startable at all. A
    /// read grant on `/` is what fixes it. Metadata rules on each grant's
    /// ancestors are what let the jailed process resolve paths it was handed.
    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_grants_the_root_and_ancestor_metadata_so_the_jail_can_start() {
        let plan = plan_fixture(vec![Mount {
            access: Access::Rw,
            path: PathBuf::from("/work/deep"),
        }]);
        let profile = seatbelt_profile(&plan);

        let deny = profile
            .find("(deny file-read*)")
            .expect("the blanket read deny must be emitted");
        let root = profile
            .find("(allow file-read* (literal \"/\"))")
            .expect("the root must be readable, or the jail cannot start");
        assert!(
            deny < root,
            "the root grant must FOLLOW the blanket deny (SBPL: last match wins): {profile}"
        );

        // Ancestors of every grant, as metadata-only literals.
        for ancestor in ["/work", "/home/u", "/home", "/opt/grok/bin", "/opt"] {
            assert!(
                profile.contains(&format!(
                    "(allow file-read-metadata (literal \"{ancestor}\"))"
                )),
                "missing ancestor metadata for {ancestor}: {profile}"
            );
        }
        // And the program sandbox-exec execs, which the system base and
        // $GROK_HOME need not cover.
        assert!(
            profile.contains("(allow file-read* (literal \"/opt/grok/bin/grok\"))"),
            "the jailed binary must be readable by path: {profile}"
        );

        // The halves of the confinement must survive all of that: ancestor
        // grants are metadata-only and name one directory each (never a
        // subtree).
        assert!(
            !profile.contains("(allow file-read-metadata (subpath"),
            "an ancestor must be granted as a literal, not a subtree: {profile}"
        );
        for ancestor in ["/home", "/work"] {
            assert!(
                !profile.contains(&format!("(allow file-read* (subpath \"{ancestor}\"))")),
                "an ancestor must never be granted as a subtree: {profile}"
            );
        }
        assert!(
            !profile.contains("(allow file-read* (subpath \"/\"))"),
            "the root grant must name the root, not the whole filesystem: {profile}"
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
        // Last matching rule wins: the rw read comes before its own write, the ro read comes after the rw read, the base before both.
        assert!(base_read < rw_read, "base must precede user mounts");
        assert!(rw_read < rw_write, "read then write for an rw mount");
        assert!(rw_read < ro_read, "user mounts keep their order");
        assert!(ro_read < home, "grok home must win over a user mount");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_reads_cover_the_macos_system_base() {
        // macOS needs /System, /Library and the real /private tree readable or the binary cannot dyld-load.
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

    // ── cwd rw default.

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
        assert_eq!(
            front.access,
            Access::Rw,
            "pathbox (no mounts) must mount the cwd rw"
        );
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
        // The default is a plan-level injection, not a flag: the parsed request must carry exactly the user's mounts.
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
        let plan = build_plan(
            &request,
            &JailDefaults::default(),
            argv(&["--sandbox=pathbox", "--ro", "."]),
        )
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
        // Prints the plan + platform backend for the cases so the rw-vs-ro
        // difference is visible in captured output, not asserted.
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
                .map(|m| format!("{:?} {}", m.access, m.path.display()))
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

    // ── `[jail]` config defaults
    // ──────────────────────────────────────────.

    /// Resolve the `[jail]` table of an in-memory config string.
    fn config_defaults(toml: &str) -> JailDefaults {
        let value: toml::Value = toml::from_str(toml).expect("fixture toml parses");
        JailDefaults::from_config(&value)
    }

    const RELEASE: JailDefaults = JailDefaults {
        cwd: Access::Rw,
        grok_home: Access::Rw,
        tmp: TmpHandling::Overlay,
        cache: TmpHandling::Overlay,
        system: Access::Ro,
    };

    #[test]
    fn no_config_yields_the_release_defaults() {
        assert_eq!(JailDefaults::default(), RELEASE);
        // No `[jail]` section, or an empty file, still resolves to the release struct that every builder consumes.
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
        // /tmp as an empty tmpfs.
        let d = config_defaults("[jail]\ntmp = \"tmpfs\"");
        assert_eq!(
            d,
            JailDefaults {
                tmp: TmpHandling::Tmpfs,
                ..RELEASE
            }
        );
        // The caches take the same tokens as /tmp.
        for (token, handling) in [
            ("tmpfs", TmpHandling::Tmpfs),
            ("rw", TmpHandling::Rw),
            ("ro", TmpHandling::Ro),
        ] {
            assert_eq!(
                config_defaults(&format!("[jail]\ncache = \"{token}\"")),
                JailDefaults {
                    cache: handling,
                    ..RELEASE
                }
            );
        }
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
    fn every_axis_together_and_explicit_rewrite_back_to_release() {
        let d = config_defaults(
            "[jail]\ncwd = \"ro\"\ngrok_home = \"ro\"\ntmp = \"rw\"\ncache = \"tmpfs\"\nsystem = \"rw\"",
        );
        assert_eq!(
            d,
            JailDefaults {
                cwd: Access::Ro,
                grok_home: Access::Ro,
                tmp: TmpHandling::Rw,
                cache: TmpHandling::Tmpfs,
                system: Access::Rw,
            }
        );
        // Explicitly spelling the release values round-trips to release.
        assert_eq!(
            config_defaults(
                "[jail]\ncwd = \"rw\"\ngrok_home = \"rw\"\ntmp = \"overlay\"\ncache = \"overlay\"\nsystem = \"ro\""
            ),
            RELEASE
        );
    }

    #[test]
    fn bogus_values_and_unknown_section_keys_fall_back_not_guess() {
        // A misspelled value is a no-op (keeps the release access), never a jail the user did not literally ask for.
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
        assert_eq!(
            config_defaults("[profiles.strict]\ndeny = [\".env\"]"),
            RELEASE
        );
    }

    /// A throwaway directory for a fixture `config.toml`.
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
                cache: TmpHandling::Overlay,
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
        // Config says ro, but --rw. on the line must win.
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
        // Config says rw, but --ro. on the line must win.
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
            cache: TmpHandling::Ro,
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
        // The shipped profile with grok home ro must keep the home readable and emit NO write-allow for it.
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
        // Release defaults: grok home is writable.
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
        // system = "rw" emits a write-allow over the whole platform base (the user's explicit weakening), while keeping the read side present.
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
        assert!(
            allow_work < deny_secrets,
            "deny must come after the allow: {profile}"
        );
    }

    #[test]
    fn rn_on_the_working_directory_is_refused() {
        // --rn. hides the cwd, but the jail must chdir into it, so refuse.
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
        // sink over the exact path, grounded on the shared deny-sink dir.
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
            !args
                .windows(3)
                .any(|w| w == ["--bind", "/work/secrets", "/work/secrets"]),
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
            args.windows(3)
                .any(|w| w[0] == "--ro-bind-try" && w[1] == "/usr"),
            "release system base must be ro-bind-try: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]),
            "release /tmp must be a tmpfs: {args:?}"
        );
        assert!(
            args.windows(3)
                .any(|w| w == ["--bind", "/home/u/.grok", "/home/u/.grok"]),
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
            args.windows(3)
                .any(|w| w == ["--ro-bind", "/home/u/.grok", "/home/u/.grok"]),
            "grok_home=ro must --ro-bind the home: {args:?}"
        );
        assert!(
            !args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]),
            "tmp=rw must not leave the tmpfs: {args:?}"
        );
    }

    fn bwrap_args(plan: &JailPlan) -> Vec<String> {
        bwrap_command(plan)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    fn position_of(args: &[String], window: &[&str]) -> Option<usize> {
        args.windows(window.len()).position(|w| w == window)
    }

    #[test]
    fn bwrap_overlays_tmp_and_the_cache_when_bwrap_can() {
        let mut plan = plan_fixture(Vec::new());
        plan.overlay = true;
        let args = bwrap_args(&plan);
        for dir in ["/tmp", "/home/u/.cache"] {
            assert!(
                position_of(&args, &["--overlay-src", dir, "--tmp-overlay", dir]).is_some(),
                "{dir} must be an overlay of the host directory: {args:?}"
            );
            assert!(
                position_of(&args, &["--tmpfs", dir]).is_none(),
                "an overlay leaves no tmpfs on {dir}: {args:?}"
            );
        }
    }

    #[test]
    fn bwrap_mounts_a_tmpfs_where_it_cannot_overlay() {
        let plan = plan_fixture(Vec::new());
        let args = bwrap_args(&plan);
        for dir in ["/tmp", "/home/u/.cache"] {
            assert!(
                position_of(&args, &["--tmpfs", dir]).is_some(),
                "without overlay support {dir} must be a tmpfs: {args:?}"
            );
        }
        assert!(
            !args.iter().any(|a| a == "--overlay-src"),
            "an unproven overlay must never be emitted: {args:?}"
        );
    }

    #[test]
    fn a_mount_over_a_parent_cannot_expose_the_host_cache_or_tmp() {
        // `--rw /home/u` and `--rw /` contain the cache and /tmp. The scratch
        // mounts must come after them, or the jail writes the host directories.
        let mut plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/"),
            },
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/home/u"),
            },
        ]);
        plan.overlay = true;
        let args = bwrap_args(&plan);
        let root = position_of(&args, &["--bind", "/", "/"]).expect("root bind");
        let home = position_of(&args, &["--bind", "/home/u", "/home/u"]).expect("home bind");
        let tmp = position_of(&args, &["--overlay-src", "/tmp"]).expect("tmp overlay");
        let cache =
            position_of(&args, &["--overlay-src", "/home/u/.cache"]).expect("cache overlay");
        assert!(root < tmp, "/tmp must cover the root bind: {args:?}");
        assert!(home < cache, "the cache must cover the home bind: {args:?}");
    }

    #[test]
    fn a_mount_on_the_scratch_directory_itself_still_wins() {
        // A user who names the directory, or a path in it, gets the host one.
        let plan = plan_fixture(vec![
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/home/u"),
            },
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/home/u/.cache/go-build"),
            },
            Mount {
                access: Access::Rw,
                path: PathBuf::from("/tmp"),
            },
        ]);
        let args = bwrap_args(&plan);
        let cache = position_of(&args, &["--tmpfs", "/home/u/.cache"]).expect("cache tmpfs");
        let go_build = position_of(
            &args,
            &[
                "--bind",
                "/home/u/.cache/go-build",
                "/home/u/.cache/go-build",
            ],
        )
        .expect("go-build bind");
        let tmp_tmpfs = position_of(&args, &["--tmpfs", "/tmp"]).expect("tmp tmpfs");
        let tmp_bind = position_of(&args, &["--bind", "/tmp", "/tmp"]).expect("tmp bind");
        assert!(
            cache < go_build,
            "a mount inside the cache must win: {args:?}"
        );
        assert!(
            tmp_tmpfs < tmp_bind,
            "--rw /tmp must win over the tmpfs: {args:?}"
        );
    }

    #[test]
    fn a_cwd_inside_the_cache_stays_bound() {
        // The injected cwd mount is the first index. With no mount over the
        // cache, the cache mount goes before it, so the cwd bind is not hidden.
        let mut plan = plan_fixture(vec![Mount {
            access: Access::Rw,
            path: PathBuf::from("/home/u/.cache/project"),
        }]);
        plan.cwd = PathBuf::from("/home/u/.cache/project");
        let args = bwrap_args(&plan);
        let cache = position_of(&args, &["--tmpfs", "/home/u/.cache"]).expect("cache tmpfs");
        let cwd = position_of(
            &args,
            &["--bind", "/home/u/.cache/project", "/home/u/.cache/project"],
        )
        .expect("cwd bind");
        assert!(
            cache < cwd,
            "the cwd must stay bound over the cache: {args:?}"
        );
    }

    #[test]
    fn host_cache_dirs_skips_a_dir_that_contains_home() {
        let home = FixtureHome::new("xdg-home");
        let canonical = dunce::canonicalize(&home.0).unwrap();
        assert_eq!(
            cache_dirs_for(Some(home.0.clone()), Some(home.0.join("xdg-cache"))).unwrap(),
            vec![canonical.join("xdg-cache"), canonical.join(".cache")],
            "$XDG_CACHE_HOME first, then ~/.cache, each created"
        );
        assert_eq!(
            cache_dirs_for(Some(home.0.clone()), Some(home.0.clone())).unwrap(),
            vec![canonical.join(".cache")],
            "an overlay on a dir that contains home would expose all of home"
        );
        assert_eq!(
            cache_dirs_for(Some(home.0.clone()), Some(PathBuf::from("relative"))).unwrap(),
            vec![canonical.join(".cache")],
            "a relative $XDG_CACHE_HOME is invalid per the XDG spec"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_never_lets_the_jail_write_the_host_tmp_or_cache() {
        let mut plan = plan_fixture(vec![Mount {
            access: Access::Rw,
            path: PathBuf::from("/home/u"),
        }]);
        plan.cache_redirect = Some(PathBuf::from("/home/u/.grok/sandbox-tmp/1/cache"));
        let profile = seatbelt_profile(&plan);
        let home_write = profile
            .find("(allow file-write* (subpath \"/home/u\"))")
            .expect("home write");
        for dir in ["/private/tmp", "/home/u/.cache"] {
            let read = profile
                .find(&format!("(allow file-read* (subpath \"{dir}\"))"))
                .unwrap_or_else(|| panic!("{dir} stays readable as the lower layer: {profile}"));
            let deny = profile
                .find(&format!("(deny file-write* (subpath \"{dir}\"))"))
                .unwrap_or_else(|| panic!("{dir} must not be writable: {profile}"));
            assert!(read < deny, "{profile}");
            if dir == "/home/u/.cache" {
                assert!(
                    home_write < deny,
                    "the deny must follow the home grant: {profile}"
                );
            }
        }
        assert!(
            profile.contains("(allow file-write* (subpath \"/home/u/.grok/sandbox-tmp/1/cache\"))"),
            "the clone is the writable layer: {profile}"
        );
        let cache_env = seatbelt_command(&plan)
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == "XDG_CACHE_HOME")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(
            cache_env.as_deref(),
            Some("/home/u/.grok/sandbox-tmp/1/cache")
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn seatbelt_tmpfs_hides_the_host_cache() {
        let mut plan = plan_fixture(Vec::new());
        plan.defaults.cache = TmpHandling::Tmpfs;
        let profile = seatbelt_profile(&plan);
        assert!(
            profile.contains("(deny file-read* (subpath \"/home/u/.cache\"))"),
            "tmpfs means the host cache is not visible: {profile}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn the_session_reaper_removes_the_directory_once_the_jail_and_its_children_exit() {
        let root = FixtureHome::new("reaper");
        let dir = root.0.join("session");
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        let fd = spawn_session_reaper(&dir).expect("start the reaper");
        // A process the jail started inherits the write end, like any tool would.
        #[allow(clippy::disallowed_methods)]
        let mut child = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .expect("spawn a child");
        // SAFETY: the fd the reaper call leaked for the exec; nothing else owns it.
        unsafe { libc::close(fd) };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            dir.exists(),
            "a live child still holds the write end, so the directory must stay"
        );
        child.wait().expect("wait for the child");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while dir.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            !dir.exists(),
            "the reaper must remove the session directory once every holder exits"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn clone_tree_makes_an_independent_copy() {
        let root = FixtureHome::new("clone");
        let source = root.0.join("source");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/file"), "host").unwrap();
        let target = root.0.join("target");
        clone_tree(&source, &target).expect("clonefile on APFS");
        assert_eq!(
            std::fs::read_to_string(target.join("nested/file")).unwrap(),
            "host"
        );
        std::fs::write(target.join("nested/file"), "jail").unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("nested/file")).unwrap(),
            "host",
            "a write to the clone must never reach the source"
        );
    }
}
