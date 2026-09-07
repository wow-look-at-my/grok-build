//! `--sandbox`: replace this process with itself inside an OS jail.
//!
//! Linux uses `bwrap`, macOS uses `sandbox-exec`. The re-exec happens before
//! any other startup work, so everything the session does — the agent, its
//! tools, every child process — runs inside the jail.
//!
//! The mount list is ordered. A later `--ro`/`--rw` overrides an earlier one
//! for the same path or for a path that contains it. `$GROK_HOME` (`~/.grok`)
//! is bound read-write after the user mounts, so nothing can take it away.

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
/// Where the dedicated tmpfs is mounted on Linux.
const JAIL_TMP: &str = "/tmp";

/// How a path is bound inside the jail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Readable, not writable.
    Ro,
    /// Readable and writable.
    Rw,
}

/// One `--ro`/`--rw` request, in the order the command line gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub access: Access,
    pub path: PathBuf,
}

/// What the command line asked the jail for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JailRequest {
    /// Whether `--sandbox`, `--ro` or `--rw` asked for a jail.
    pub enabled: bool,
    /// User mounts, in command-line order. Later entries win.
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

/// Read `--sandbox`, `--ro` and `--rw` off the raw command line.
///
/// The raw line is what carries the order of the mounts; a parsed struct
/// groups the two flags into separate lists and loses it.
///
/// A bare `--sandbox` asks for the jail. `--sandbox <profile>` keeps its older
/// meaning (a profile name) and asks for nothing here. `--ro` and `--rw` ask
/// for the jail by themselves. Scanning stops at a bare `--`, so a prompt is
/// never read as a flag.
pub fn parse_jail_args<I>(args: I) -> Result<JailRequest, JailError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut request = JailRequest::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else { continue };
        if text == "--" {
            break;
        }
        if let Some(path) = flag_value("ro", text, &mut args)? {
            request.enabled = true;
            request.mounts.push(Mount {
                access: Access::Ro,
                path,
            });
            continue;
        }
        if let Some(path) = flag_value("rw", text, &mut args)? {
            request.enabled = true;
            request.mounts.push(Mount {
                access: Access::Rw,
                path,
            });
            continue;
        }
        if text == "--sandbox" {
            // clap takes the next token as the value unless it looks like a
            // flag. Read it the same way, or the two disagree about what a
            // bare `--sandbox` is.
            let has_value = args
                .peek()
                .and_then(|next| next.to_str())
                .is_some_and(|next| !next.starts_with('-'));
            if has_value {
                args.next();
            } else {
                request.enabled = true;
            }
        }
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
    /// User mounts, in order, canonicalized.
    pub mounts: Vec<Mount>,
    /// `$GROK_HOME`. Bound read-write after the user mounts.
    pub grok_home: PathBuf,
    /// The scratch directory the jail dedicates to this process.
    pub temp_dir: PathBuf,
    /// The binary to run inside the jail.
    pub self_exe: PathBuf,
    /// The directory the jailed process starts in.
    pub cwd: PathBuf,
    /// Arguments for the jailed binary, without `argv[0]`.
    pub args: Vec<OsString>,
}

/// Resolve a request into a plan. Every path must exist: a missing bind is a
/// hole the caller cannot see.
pub fn build_plan(request: &JailRequest, args: Vec<OsString>) -> Result<JailPlan, JailError> {
    let mut mounts = Vec::with_capacity(request.mounts.len());
    for mount in &request.mounts {
        let flag = match mount.access {
            Access::Ro => "ro",
            Access::Rw => "rw",
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
    let temp_dir = dedicated_temp_dir()?;
    let plan = JailPlan {
        mounts,
        grok_home,
        temp_dir,
        self_exe,
        cwd,
        args,
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
#[cfg(target_os = "linux")]
pub fn bwrap_command(plan: &JailPlan) -> std::process::Command {
    let mut cmd = std::process::Command::new("bwrap");
    cmd.arg("--die-with-parent");
    cmd.arg("--cap-drop").arg("ALL");
    for path in SYSTEM_RO_BASE {
        cmd.arg("--ro-bind-try").arg(path).arg(path);
    }
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--dev").arg("/dev");
    cmd.arg("--tmpfs").arg(JAIL_TMP);
    for mount in &plan.mounts {
        let flag = match mount.access {
            Access::Ro => "--ro-bind",
            Access::Rw => "--bind",
        };
        cmd.arg(flag).arg(&mount.path).arg(&mount.path);
    }
    cmd.arg("--bind").arg(&plan.grok_home).arg(&plan.grok_home);
    cmd.arg("--ro-bind-try")
        .arg(&plan.self_exe)
        .arg(&plan.self_exe);
    cmd.arg("--chdir").arg(&plan.cwd);
    cmd.arg("--setenv").arg(JAIL_ENV_VAR).arg("1");
    cmd.arg("--setenv").arg(BWRAP_ENV_VAR).arg("1");
    cmd.arg("--").arg(&plan.self_exe).args(&plan.args);
    cmd
}

/// Build the Seatbelt profile for the plan.
///
/// Seatbelt confines WRITES here, not reads: `(allow default)` keeps the
/// process readable so it can still run, and `(deny file-write*)` takes every
/// write away. `--rw` gives one back and `--ro` takes one away again. SBPL
/// gives the last matching rule, so emitting the mounts in order is what
/// makes a later flag win.
#[cfg(target_os = "macos")]
pub fn seatbelt_profile(plan: &JailPlan) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    // A terminal, a PTY and /dev/null are writes every tool makes.
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    for mount in &plan.mounts {
        let rule = match mount.access {
            Access::Ro => "deny",
            Access::Rw => "allow",
        };
        profile.push_str(&format!(
            "({rule} file-write* (subpath \"{}\"))\n",
            sbpl_escape(&mount.path)
        ));
    }
    profile.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        sbpl_escape(&plan.grok_home)
    ));
    profile.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        sbpl_escape(&plan.temp_dir)
    ));
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
    if !request.enabled {
        return;
    }
    let plan = match build_plan(&request, argv) {
        Ok(plan) => plan,
        Err(e) => fail(&e.to_string()),
    };
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
    fn bare_sandbox_enables_the_jail() {
        assert!(parse(&["--sandbox"]).enabled);
        assert!(parse(&["--sandbox", "--minimal"]).enabled);
    }

    #[test]
    fn sandbox_with_a_profile_value_is_the_older_flag() {
        let request = parse(&["--sandbox", "strict"]);
        assert!(!request.enabled, "a profile name must not build a jail");
        assert!(!parse(&["--sandbox=strict"]).enabled);
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
            self_exe: PathBuf::from("/opt/grok/bin/grok"),
            cwd: PathBuf::from("/work"),
            args: vec![OsString::from("--sandbox")],
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
    fn seatbelt_denies_writes_and_ends_with_grok_home() {
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
        let rw = profile
            .find("(allow file-write* (subpath \"/work\"))")
            .expect("rw rule");
        let ro = profile
            .find("(deny file-write* (subpath \"/work/secrets\"))")
            .expect("ro rule");
        let home = profile
            .find("(allow file-write* (subpath \"/home/u/.grok\"))")
            .expect("grok home rule");
        assert!(profile.contains("(deny file-write*)\n"));
        assert!(rw < ro, "the last matching SBPL rule wins");
        assert!(ro < home);
    }

    #[test]
    fn sbpl_escapes_a_quote_in_a_path() {
        assert_eq!(sbpl_escape(Path::new("/a\"b")), "/a\\\"b");
    }
}
