//! Runs the shipped `gh` shim inside the real jail: bwrap on Linux, Seatbelt
//! on macOS.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use xai_grok_sandbox::gh_shim::{GH_CONTRACT, ShimLayout, client_script, command_for};
use xai_grok_sandbox::jail::{Access, JailDefaults, JailPlan, Mount};

fn fixture_dir(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grok-gh-shim-{}-{name}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    dunce::canonicalize(&path).unwrap()
}

fn executable(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn host_api_cli() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("api-cli"))
        .find(|candidate| candidate.is_file())
        .expect("api-cli must be on PATH: the gh shim runs it on both sides of the jail")
}

#[cfg(target_os = "linux")]
fn jail_is_usable() -> bool {
    Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "true"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn jail_is_usable() -> bool {
    Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn jail_command(plan: &JailPlan) -> Command {
    xai_grok_sandbox::jail::bwrap_command(plan)
}

#[cfg(target_os = "macos")]
fn jail_command(plan: &JailPlan) -> Command {
    xai_grok_sandbox::jail::seatbelt_command(plan)
}

#[test]
fn the_shim_runs_gh_on_the_host_and_the_jail_holds_no_token() {
    if !jail_is_usable() {
        eprintln!("SKIP: this host cannot build the jail");
        return;
    }
    let grok_home = fixture_dir("home");
    let work = fixture_dir("work");
    let host_bin = fixture_dir("hostbin");
    executable(
        &host_bin.join("gh"),
        "#!/bin/sh\necho \"argv: $*\"\necho \"token: ${GH_TOKEN:-none}\"\n",
    );
    let layout = ShimLayout::for_session(&grok_home, std::process::id());
    std::fs::create_dir_all(layout.socket.parent().unwrap()).unwrap();
    std::fs::write(&layout.contract, GH_CONTRACT).unwrap();
    let api_cli = host_api_cli();
    std::fs::copy(&api_cli, &layout.api_cli).unwrap();
    executable(&layout.bin.join("gh"), &client_script(&layout));

    let path = format!(
        "{}:{}",
        host_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut server = command_for(&api_cli)
        .arg("--config")
        .arg(&layout.contract)
        .arg("--server")
        .arg(&layout.socket)
        .env("PATH", path)
        .env("GH_TOKEN", "host-only")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !layout.socket.exists() {
        if let Some(status) = server.try_wait().unwrap() {
            panic!("api-cli --server exited before it listened: {status}");
        }
        assert!(std::time::Instant::now() < deadline, "no socket");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let script = format!(
        "PATH={bin}:\"$PATH\"\n\
         echo \"inner_token=${{GH_TOKEN:-none}}\"\n\
         gh pr view 7 --json title\n\
         gh pr merge 7; echo \"merge_rc=$?\"\n",
        bin = layout.bin.display(),
    );
    let plan = JailPlan {
        mounts: vec![Mount {
            access: Access::Rw,
            path: work.clone(),
        }],
        grok_home: grok_home.clone(),
        temp_dir: grok_home.join("sandbox-tmp"),
        deny_sink: grok_home.join("sandbox-tmp").join("deny-sink"),
        self_exe: PathBuf::from("/bin/sh"),
        cwd: work.clone(),
        args: vec![OsString::from("-c"), OsString::from(script)],
        defaults: JailDefaults::default(),
        ci_host_fd: None,
        cache_dirs: Vec::new(),
        overlay: false,
        cache_redirect: None,
        grok_home_alias: None,
        grok_home_links: Vec::new(),
    };
    std::fs::create_dir_all(&plan.deny_sink).unwrap();
    let output = jail_command(&plan)
        .env_remove("GH_TOKEN")
        .output()
        .expect("the jail must run");
    let _ = server.kill();
    let _ = server.wait();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "inner_token=none",
        "argv: pr view 7 --json=title",
        "token: host-only",
    ] {
        assert!(
            stdout.contains(expected),
            "missing '{expected}'\nstdout={stdout}\nstderr={stderr}"
        );
    }
    assert!(
        !stdout.contains("argv: pr merge"),
        "pr merge reached the host gh: {stdout}"
    );
    assert!(
        stdout.contains("merge_rc=") && !stdout.contains("merge_rc=0"),
        "pr merge must fail inside the jail: {stdout}"
    );
    for dir in [&grok_home, &work, &host_bin] {
        let _ = std::fs::remove_dir_all(dir);
    }
}
