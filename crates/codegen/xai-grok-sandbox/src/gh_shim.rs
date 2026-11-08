//! The `gh` a sandboxed session runs. No credential is inside the sandbox.

use std::path::{Path, PathBuf};

/// The contract api-cli serves. The only copy of the `gh` allowlist.
pub const GH_CONTRACT: &str = include_str!("../sandbox/gh.xml");

/// Env var that names the directory that holds the `gh` shim.
pub const GH_SHIM_DIR_ENV: &str = "GROK_GH_SHIM_DIR";

/// The variables gh and the GitHub API clients read a token from.
pub const GITHUB_TOKEN_VARS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// How long the host waits for the server's socket.
const SOCKET_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The files of one session's shim, all under `$GROK_HOME`, which every sandbox can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimLayout {
    /// The directory put first on `PATH`. It holds `gh`.
    pub bin: PathBuf,
    pub socket: PathBuf,
    pub contract: PathBuf,
    /// The copy of api-cli the client runs.
    pub api_cli: PathBuf,
}

impl ShimLayout {
    pub fn for_session(grok_home: &Path, pid: u32) -> Self {
        let root = grok_home.join("sandbox-gh");
        let session = root.join(pid.to_string());
        Self {
            bin: session.join("bin"),
            socket: session.join("gh.sock"),
            contract: session.join("gh.xml"),
            api_cli: root.join("api-cli"),
        }
    }
}

/// The `gh` script that runs `api-cli --client`. An APE unpacks its loader
/// into a directory that is writable and sticky. TMPDIR is the directory
/// every sandbox can write, but it is not sticky, so the script makes a
/// sticky, private (a later mode) directory inside it.
pub fn client_script(layout: &ShimLayout) -> String {
    format!(
        "#!/bin/sh\nif [ -z \"${{APE_LOADERDIR:-}}\" ]; then APE_LOADERDIR=\"${{TMPDIR:-/tmp}}/.ape-loader\"; mkdir -m 1700 \"$APE_LOADERDIR\" 2>/dev/null; fi; export APE_LOADERDIR\nexec {} --client {} \"$@\"\n",
        sh_quote(&layout.api_cli),
        sh_quote(&layout.socket)
    )
}

/// The `gh` script when the host cannot serve `gh`: every call fails and says why.
pub fn unavailable_script(reason: &str) -> String {
    let message = format!(
        "gh (grok sandbox): the sandbox holds no GitHub credential, and the host cannot run gh for it: {reason}. Install api-cli on the host (brew or apt package api-cli) and start a new session."
    );
    format!(
        "#!/bin/sh\nprintf '%s\\n' {} >&2\nexit 1\n",
        sh_quote(Path::new(&message))
    )
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Host side, before the sandbox starts: start the server, write the shim and
/// put it first on this process's `PATH`. A failure still installs a shim, one
/// that fails every call loudly, so the real `gh` never runs credential-less.
pub fn start() {
    let layout = ShimLayout::for_session(&crate::paths::grok_home(), std::process::id());
    sweep_dead_sessions(layout.api_cli.parent());
    let script = match start_server(&layout) {
        Ok(()) => client_script(&layout),
        Err(reason) => {
            eprintln!("warning: sandbox: gh cannot reach the host: {reason}");
            unavailable_script(&reason)
        }
    };
    // The server took its environment when it started. The session that becomes the sandbox keeps no token.
    for name in GITHUB_TOKEN_VARS {
        // SAFETY: this runs on the startup path, before the session starts its threads.
        unsafe { std::env::remove_var(name) };
    }
    if let Err(error) = write_script(&layout.bin.join("gh"), &script) {
        eprintln!(
            "warning: sandbox: could not install the gh shim in {}: {error}",
            layout.bin.display()
        );
        return;
    }
    publish(&layout.bin);
}

fn start_server(layout: &ShimLayout) -> Result<(), String> {
    let host_api_cli = find_on_path("api-cli").ok_or("api-cli is not on PATH")?;
    let session = layout.bin.parent().ok_or("no session directory")?;
    std::fs::create_dir_all(session).map_err(|e| format!("{}: {e}", session.display()))?;
    std::fs::write(&layout.contract, GH_CONTRACT)
        .map_err(|e| format!("{}: {e}", layout.contract.display()))?;
    copy_if_changed(&host_api_cli, &layout.api_cli)
        .map_err(|e| format!("copy {}: {e}", host_api_cli.display()))?;
    let _ = std::fs::remove_file(&layout.socket);
    // The server lives while this pipe is open. The write end crosses the exec into the sandbox and closes when the session ends.
    let (read_end, write_end) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut cmd = command_for(&host_api_cli);
    cmd.arg("--config")
        .arg(&layout.contract)
        .arg("--server")
        .arg(&layout.socket)
        .stdin(read_end)
        .stdout(std::process::Stdio::null());
    // The server outlives this process image by design, so nothing waits on it.
    #[allow(clippy::disallowed_methods)]
    cmd.spawn()
        .map_err(|e| format!("start {}: {e}", host_api_cli.display()))?;
    use std::os::fd::IntoRawFd as _;
    let fd = write_end.into_raw_fd();
    crate::ci_host::inherit_across_exec(fd).ok_or("cannot keep the server pipe open")?;
    let deadline = std::time::Instant::now() + SOCKET_WAIT;
    while !layout.socket.exists() {
        if std::time::Instant::now() > deadline {
            return Err(format!(
                "api-cli --server did not open {} in {}s",
                layout.socket.display(),
                SOCKET_WAIT.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(())
}

/// The magic every APE opens with. execve on macOS refuses such a file, and a shell runs it.
const APE_MAGIC: &[u8] = b"MZqFpD='";

/// A command that starts `program`, through `/bin/sh` when it is an APE. The
/// std fix is in the org's rust fork, and this goes once grok builds with a
/// std that has it.
pub fn command_for(program: &Path) -> std::process::Command {
    use std::io::Read as _;
    let mut head = [0u8; APE_MAGIC.len()];
    let is_ape = std::fs::File::open(program)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok()
        && head == APE_MAGIC;
    if is_ape {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg(program);
        cmd
    } else {
        std::process::Command::new(program)
    }
}

/// Copy `from` to `to` unless `to` already has the same size and modification time.
fn copy_if_changed(from: &Path, to: &Path) -> std::io::Result<()> {
    let source = std::fs::metadata(from)?;
    if let Ok(target) = std::fs::metadata(to)
        && target.len() == source.len()
        && target.modified().ok() == source.modified().ok()
    {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = to.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::copy(from, &temp)?;
    if let Ok(modified) = source.modified() {
        std::fs::File::options()
            .write(true)
            .open(&temp)?
            .set_modified(modified)?;
    }
    std::fs::rename(&temp, to)
}

fn write_script(path: &Path, body: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, body)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Remove the session directories of processes that are gone.
fn sweep_dead_sessions(root: Option<&Path>) {
    let Some(entries) = root.and_then(|root| std::fs::read_dir(root).ok()) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        // SAFETY: kill with signal 0 only checks that the process exists.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The first executable `name` on `PATH`, skipping the shim directory.
fn find_on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let shim = std::env::var_os(GH_SHIM_DIR_ENV);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| shim.as_deref() != Some(dir.as_os_str()))
        .map(|dir| dir.join(name))
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// Put `bin` first on this process's `PATH` and name it.
fn publish(bin: &Path) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut entries = vec![bin.to_path_buf()];
    entries.extend(std::env::split_paths(&path).filter(|entry| entry != bin));
    let Ok(path) = std::env::join_paths(entries) else {
        eprintln!("warning: sandbox: the gh shim is not on PATH: a PATH entry holds a ':'");
        return;
    };
    // SAFETY: this runs on the startup path, before the session starts its threads.
    unsafe {
        std::env::set_var(GH_SHIM_DIR_ENV, bin);
        std::env::set_var("PATH", path);
    }
}

/// The command a sandboxed shell runs. A login shell rebuilds `PATH` from the
/// user's profile, which puts the real `gh` first again. The prefix runs after
/// the profile, so the shim stays first.
pub fn prefix_shell_command(command: &str) -> String {
    match std::env::var_os(GH_SHIM_DIR_ENV) {
        Some(dir) => prefix_with_shim_dir(Path::new(&dir), command),
        None => command.to_string(),
    }
}

fn prefix_with_shim_dir(dir: &Path, command: &str) -> String {
    format!("PATH={}:\"$PATH\"; export PATH\n{command}", sh_quote(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_puts_the_shim_first_after_the_login_profile() {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(prefix_with_shim_dir(
                Path::new("/it's/shims"),
                "echo \"$PATH\"",
            ))
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "/it's/shims:/usr/bin:/bin\n"
        );
    }

    #[test]
    fn the_layout_keeps_every_file_under_grok_home() {
        let layout = ShimLayout::for_session(Path::new("/g"), 42);
        assert_eq!(layout.bin, PathBuf::from("/g/sandbox-gh/42/bin"));
        assert_eq!(layout.socket, PathBuf::from("/g/sandbox-gh/42/gh.sock"));
        assert_eq!(layout.api_cli, PathBuf::from("/g/sandbox-gh/api-cli"));
    }

    #[test]
    fn the_client_script_runs_api_cli_against_the_socket() {
        let layout = ShimLayout::for_session(Path::new("/g"), 42);
        assert_eq!(
            client_script(&layout),
            "#!/bin/sh\nif [ -z \"${APE_LOADERDIR:-}\" ]; then APE_LOADERDIR=\"${TMPDIR:-/tmp}/.ape-loader\"; mkdir -m 1700 \"$APE_LOADERDIR\" 2>/dev/null; fi; export APE_LOADERDIR\nexec '/g/sandbox-gh/api-cli' --client '/g/sandbox-gh/42/gh.sock' \"$@\"\n"
        );
    }

    #[test]
    fn the_unavailable_script_fails_every_call_and_says_why() {
        let dir = std::env::temp_dir().join(format!("gh-shim-missing-{}", std::process::id()));
        let gh = dir.join("gh");
        write_script(&gh, &unavailable_script("api-cli is not on PATH")).unwrap();
        let out = std::process::Command::new(&gh)
            .args(["pr", "view"])
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("api-cli is not on PATH"),
            "{out:?}"
        );
    }

    #[test]
    fn the_contract_declares_only_reads() {
        for write in [
            "merge", "create", "edit", "close", "comment", "review", "rerun", "cancel", "delete",
            "token",
        ] {
            assert!(
                !GH_CONTRACT.contains(&format!("name=\"{write}\"")),
                "gh.xml must not declare {write}"
            );
        }
        for flag in ["method", "input", "header"] {
            assert!(
                !GH_CONTRACT.contains(&format!("name=\"{flag}\"")),
                "gh api must not take --{flag}"
            );
        }
    }

    #[test]
    fn command_for_starts_an_ape_through_sh() {
        let dir = std::env::temp_dir().join(format!("gh-shim-ape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ape = dir.join("ape");
        std::fs::write(&ape, b"MZqFpD='\n").unwrap();
        let native = dir.join("native");
        std::fs::write(&native, b"\x7fELF").unwrap();
        let cmd = command_for(&ape);
        assert_eq!(cmd.get_program(), "/bin/sh");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), vec![ape.as_os_str()]);
        assert_eq!(command_for(&native).get_program(), native.as_os_str());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_if_changed_replaces_a_stale_copy() {
        let dir = std::env::temp_dir().join(format!("gh-shim-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("a");
        let to = dir.join("sub/b");
        std::fs::write(&from, "v1").unwrap();
        copy_if_changed(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "v1");
        std::fs::write(&from, "v22").unwrap();
        copy_if_changed(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "v22");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
