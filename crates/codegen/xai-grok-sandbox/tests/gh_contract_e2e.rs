//! Serves the shipped `gh.xml` with the real api-cli and calls it as the
//! sandbox does.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use xai_grok_sandbox::gh_shim::{GH_CONTRACT, command_for};

struct Server {
    dir: PathBuf,
    socket: PathBuf,
    api_cli: PathBuf,
    child: std::process::Child,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn api_cli() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("api-cli"))
        .find(|candidate| candidate.is_file())
        .expect("api-cli must be on PATH: it serves gh.xml, and this test checks that contract")
}

fn start(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("gh-contract-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let host_bin = dir.join("hostbin");
    std::fs::create_dir_all(&host_bin).unwrap();
    let fake_gh = host_bin.join("gh");
    std::fs::write(
        &fake_gh,
        "#!/bin/sh\necho \"argv: $*\"\necho \"token: ${GH_TOKEN:-none}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let contract = dir.join("gh.xml");
    std::fs::write(&contract, GH_CONTRACT).unwrap();
    let socket = dir.join("gh.sock");
    let api_cli = api_cli();
    let path = format!(
        "{}:{}",
        host_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let child = command_for(&api_cli)
        .arg("--config")
        .arg(&contract)
        .arg("--server")
        .arg(&socket)
        .env("PATH", path)
        .env("GH_TOKEN", "host-only")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server {
        dir,
        socket,
        api_cli,
        child,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !server.socket.exists() {
        if let Some(status) = server.child.try_wait().unwrap() {
            panic!("api-cli --server exited before it listened: {status}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "api-cli --server never opened its socket"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    server
}

/// Run `gh <args>` as the sandbox does, with no token in the environment.
fn gh(server: &Server, args: &[&str]) -> (i32, String, String) {
    let out = command_for(&server.api_cli)
        .arg("--client")
        .arg(&server.socket)
        .args(args)
        .env_remove("GH_TOKEN")
        .current_dir(Path::new("/"))
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn reads_run_on_the_host_with_the_hosts_token() {
    let server = start("reads");
    for (args, argv) in [
        (
            &["pr", "view", "12", "--json", "title", "--repo", "o/r"][..],
            "pr view 12 --json=title --repo=o/r",
        ),
        (
            &["run", "list", "-L", "5", "--branch", "main"][..],
            "run list --branch=main --limit=5",
        ),
        (
            &["api", "repos/o/r/pulls", "--paginate"][..],
            "api --method=GET repos/o/r/pulls --paginate",
        ),
        (&["auth", "status"][..], "auth status"),
    ] {
        let (code, out, err) = gh(&server, args);
        assert_eq!(code, 0, "gh {args:?}: {err}");
        assert_eq!(
            out,
            format!("argv: {argv}\ntoken: host-only\n"),
            "gh {args:?}"
        );
    }
}

#[test]
fn writes_and_undeclared_flags_never_reach_gh() {
    let server = start("writes");
    for args in [
        &["pr", "merge", "12"][..],
        &["pr", "view", "12", "--admin"][..],
        &["api", "-X", "POST", "repos/o/r/issues"][..],
        &["api", "repos/o/r/issues", "--field", "title=x"][..],
        &["api", "repos/o/r/issues", "--method", "POST"][..],
        &[
            "api",
            "repos/o/r/issues",
            "--header",
            "X-HTTP-Method-Override: DELETE",
        ][..],
        &["auth", "token"][..],
        &["issue", "create"][..],
        &["run", "rerun", "1"][..],
        &["--config", "/tmp/other.xml", "pr", "view", "1"][..],
    ] {
        let (code, out, err) = gh(&server, args);
        assert_ne!(code, 0, "gh {args:?} must be refused");
        assert!(
            !out.contains("argv:"),
            "gh {args:?} reached the real gh: {out}"
        );
        assert!(!err.is_empty(), "gh {args:?} must say why it was refused");
    }
}
