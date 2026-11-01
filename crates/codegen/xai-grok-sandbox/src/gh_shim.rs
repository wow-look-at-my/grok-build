//! The `gh` a sandboxed session runs. No credential is inside the sandbox.
//! The `gh` on the sandbox `PATH` is this binary: it asks the host worker to
//! run the command outside the sandbox.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};

/// Env var that names the sandbox end of the door.
pub const GH_DOOR_FD_ENV: &str = "GROK_GH_DOOR_FD";
/// Env var that names the directory that holds the `gh` shim.
pub const GH_SHIM_DIR_ENV: &str = "GROK_GH_SHIM_DIR";
/// Env var set on the worker when it holds the door on [`WORKER_DOOR_FD`].
pub(crate) const WORKER_DOOR_ENV: &str = "GROK_GH_DOOR_WORKER";
/// The fd the worker reads the door from.
pub(crate) const WORKER_DOOR_FD: RawFd = 3;

/// One call, as the shim sends it on its private connection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShimRequest {
    pub args: Vec<String>,
    /// The caller's working directory. `gh` finds the repository from its git remote.
    pub cwd: PathBuf,
}

/// The worker's answer to one call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShimResponse {
    Ran(crate::ci_host::GhHostResponse),
    /// The allowlist refused the call. `gh` did not start.
    Refused,
    /// `gh` could not start on the host.
    NoGh(String),
}

/// Whether `argv0` names this binary as `gh`.
pub fn invoked_as_gh(argv0: &std::ffi::OsStr) -> bool {
    Path::new(argv0)
        .file_name()
        .is_some_and(|name| name == "gh")
}

/// The shim entry point. Returns the exit code.
pub fn run_shim(args: Vec<OsString>) -> i32 {
    let mut strings = Vec::with_capacity(args.len());
    for arg in args {
        match arg.into_string() {
            Ok(arg) => strings.push(arg),
            Err(arg) => {
                eprintln!("gh (grok sandbox): argument {arg:?} is not UTF-8");
                return 2;
            }
        }
    }
    let Some(door) = std::env::var(GH_DOOR_FD_ENV)
        .ok()
        .and_then(|raw| raw.parse::<RawFd>().ok())
    else {
        eprintln!(
            "gh (grok sandbox): {GH_DOOR_FD_ENV} is not set, so no host worker can run gh. \
             This gh only works inside a grok sandbox."
        );
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let request = ShimRequest { args: strings, cwd };
    match call(door, &request) {
        Ok(ShimResponse::Ran(response)) => {
            if response.truncated {
                eprintln!("gh (grok sandbox): output is clipped to its last 1 MiB");
            }
            let _ = std::io::stdout().write_all(response.stdout.as_bytes());
            let _ = std::io::stderr().write_all(response.stderr.as_bytes());
            response.code
        }
        Ok(ShimResponse::Refused) => {
            eprintln!(
                "gh (grok sandbox): `gh {}` is not allowed. The sandbox holds no GitHub \
                 credential, so the host runs gh for it, and the host runs only read-only \
                 commands: {}.",
                request.args.join(" "),
                crate::ci_host::allowed_commands_summary()
            );
            1
        }
        Ok(ShimResponse::NoGh(error)) => {
            eprintln!("gh (grok sandbox): the host could not start gh: {error}");
            1
        }
        Err(error) => {
            eprintln!("gh (grok sandbox): the host worker did not answer: {error}");
            1
        }
    }
}

/// Send one request through the door and read its answer.
pub fn call(door: RawFd, request: &ShimRequest) -> std::io::Result<ShimResponse> {
    let (mut ours, theirs) = UnixStream::pair()?;
    send_fd(door, theirs.as_raw_fd())?;
    drop(theirs);
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    ours.write_all(&line)?;
    ours.shutdown(std::net::Shutdown::Write)?;
    let mut body = Vec::new();
    ours.read_to_end(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Send `fd` as one one-byte datagram with `SCM_RIGHTS`.
fn send_fd(door: RawFd, fd: RawFd) -> std::io::Result<()> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: an all-zero msghdr is valid. Every pointer set below lives until sendmsg returns.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer holds one cmsg of CMSG_SPACE(sizeof fd) bytes.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
    }
    // SAFETY: msg is fully initialised above.
    let sent = unsafe { libc::sendmsg(door, &msg, 0) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Receive one fd from the door. `Ok(None)` for a datagram that carried none.
fn recv_fd(door: RawFd) -> std::io::Result<Option<OwnedFd>> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    // SAFETY: CMSG_SPACE only computes a size.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: an all-zero msghdr is valid. Every pointer set below lives until recvmsg returns.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: msg is fully initialised above.
    let got = unsafe { libc::recvmsg(door, &mut msg, 0) };
    if got < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if got == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    }
    // SAFETY: recvmsg filled msg. The loop reads only headers inside msg_controllen.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let fd = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>());
                return Ok(Some(OwnedFd::from_raw_fd(fd)));
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok(None)
}

/// Worker side: answer every call that arrives on `door`, each on its own thread.
pub fn serve_door(door: OwnedFd) {
    loop {
        match recv_fd(door.as_raw_fd()) {
            Ok(Some(fd)) => {
                std::thread::spawn(move || serve_call(UnixStream::from(fd)));
            }
            Ok(None) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// Answer one call on its private connection.
fn serve_call(stream: UnixStream) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let response = match serde_json::from_str::<ShimRequest>(line.trim()) {
        Ok(request) => answer(&request),
        Err(_) => ShimResponse::Refused,
    };
    if let Ok(body) = serde_json::to_vec(&response) {
        let _ = out.write_all(&body);
    }
}

/// Run one call if the allowlist admits it.
pub fn answer(request: &ShimRequest) -> ShimResponse {
    if !crate::ci_host::gh_args_allowed(&request.args) {
        return ShimResponse::Refused;
    }
    let cwd = if request.cwd.is_dir() {
        request.cwd.as_path()
    } else {
        Path::new("/")
    };
    match crate::ci_host::spawn_gh_in(cwd, request.args.iter().map(String::as_str)) {
        Ok(output) => ShimResponse::Ran(crate::ci_host::response_from_output(output)),
        Err(error) => ShimResponse::NoGh(error.to_string()),
    }
}

/// Worker side: start the door thread when the parent handed one over.
pub(crate) fn start_worker_door() {
    if std::env::var_os(WORKER_DOOR_ENV).is_none() {
        return;
    }
    // SAFETY: the parent dup2'd the door onto WORKER_DOOR_FD before exec.
    let door = unsafe { OwnedFd::from_raw_fd(WORKER_DOOR_FD) };
    std::thread::spawn(move || serve_door(door));
}

/// Host side: make the door pair. The first fd is the sandbox end, open across
/// exec, so the session and every child it starts inherit it. The second is
/// the worker end, close-on-exec.
pub(crate) fn door_pair() -> Option<(RawFd, OwnedFd)> {
    let (sandbox_end, worker_end) = UnixDatagram::pair().ok()?;
    let sandbox_fd = std::os::unix::io::IntoRawFd::into_raw_fd(sandbox_end);
    if crate::ci_host::inherit_across_exec(sandbox_fd).is_none() {
        // SAFETY: the fd is ours and nothing else holds it.
        drop(unsafe { OwnedFd::from_raw_fd(sandbox_fd) });
        return None;
    }
    Some((sandbox_fd, OwnedFd::from(worker_end)))
}

/// Host side: put the `gh` shim first on this process's `PATH` and name the
/// door. Runs before the sandbox starts, so the shim directory can be made.
pub(crate) fn publish(door: RawFd) -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    let dir = shim_dir_for(&crate::paths::grok_home(), &exe);
    install_shim(&dir, &exe).ok()?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut entries = vec![dir.clone()];
    entries.extend(std::env::split_paths(&path).filter(|entry| entry != &dir));
    let path = std::env::join_paths(entries).ok()?;
    // SAFETY: this runs on the startup path, before the session starts its threads.
    unsafe {
        std::env::set_var(GH_DOOR_FD_ENV, door.to_string());
        std::env::set_var(GH_SHIM_DIR_ENV, &dir);
        std::env::set_var("PATH", path);
    }
    Some(())
}

/// The shim directory for `exe`: one per binary path, so concurrent sessions
/// of different builds never point each other's `gh` at the wrong binary.
pub fn shim_dir_for(grok_home: &Path, exe: &Path) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    exe.hash(&mut hasher);
    grok_home
        .join("sandbox-shims")
        .join(format!("{:016x}", hasher.finish()))
}

/// Make `dir/gh` a symlink to `exe`. The rename replaces a stale link atomically.
pub fn install_shim(dir: &Path, exe: &Path) -> std::io::Result<()> {
    let link = dir.join("gh");
    if std::fs::read_link(&link).is_ok_and(|target| target == exe) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".gh.{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    std::os::unix::fs::symlink(exe, &temp)?;
    std::fs::rename(&temp, &link)
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
    let dir = dir.to_string_lossy().replace('\'', r"'\''");
    format!("PATH='{dir}':\"$PATH\"; export PATH\n{command}")
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
                "PATH=/usr/bin:$PATH; echo \"$PATH\"",
            ))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("/usr/bin:/it's/shims:"),
            "{out:?}"
        );
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(prefix_with_shim_dir(Path::new("/s"), "echo \"$PATH\""))
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/s:/usr/bin:/bin\n");
    }

    fn door_with_worker() -> RawFd {
        let (sandbox_end, worker_end) = UnixDatagram::pair().unwrap();
        std::thread::spawn(move || serve_door(OwnedFd::from(worker_end)));
        std::os::unix::io::IntoRawFd::into_raw_fd(sandbox_end)
    }

    #[test]
    fn a_refused_call_never_starts_gh() {
        let door = door_with_worker();
        for args in [
            vec!["pr", "merge", "1"],
            vec!["api", "-X", "POST", "repos/o/r/issues"],
            vec!["auth", "token"],
        ] {
            let request = ShimRequest {
                args: args.iter().map(|arg| arg.to_string()).collect(),
                cwd: PathBuf::from("/"),
            };
            assert_eq!(
                call(door, &request).unwrap(),
                ShimResponse::Refused,
                "{args:?} must be refused before gh starts"
            );
        }
    }

    #[test]
    fn concurrent_calls_each_get_their_own_answer() {
        let door = door_with_worker();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                std::thread::spawn(move || {
                    let request = ShimRequest {
                        args: vec!["pr".into(), format!("merge{i}")],
                        cwd: PathBuf::from("/"),
                    };
                    call(door, &request).unwrap()
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), ShimResponse::Refused);
        }
    }

    #[test]
    fn an_allowed_call_runs_on_the_worker_side() {
        let door = door_with_worker();
        let request = ShimRequest {
            args: vec!["auth".into(), "status".into()],
            cwd: PathBuf::from("/"),
        };
        match call(door, &request).unwrap() {
            ShimResponse::Ran(_) | ShimResponse::NoGh(_) => {}
            ShimResponse::Refused => panic!("`gh auth status` is on the allowlist"),
        }
    }

    #[test]
    fn invoked_as_gh_reads_the_file_name() {
        assert!(invoked_as_gh("/x/sandbox-shims/ab/gh".as_ref()));
        assert!(invoked_as_gh("gh".as_ref()));
        assert!(!invoked_as_gh("/usr/bin/grok".as_ref()));
        assert!(!invoked_as_gh("/usr/bin/ghx".as_ref()));
    }

    #[test]
    fn install_shim_links_gh_to_the_binary_and_replaces_a_stale_link() {
        let root = std::env::temp_dir().join(format!("gh-shim-test-{}", std::process::id()));
        let dir = shim_dir_for(&root, Path::new("/bin/grok"));
        install_shim(&dir, Path::new("/old/grok")).unwrap();
        install_shim(&dir, Path::new("/bin/grok")).unwrap();
        assert_eq!(
            std::fs::read_link(dir.join("gh")).unwrap(),
            PathBuf::from("/bin/grok")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
