//! Proves that grok saves its own config files from inside the macOS Seatbelt profile sandbox.

#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

/// The mode a spawned child runs in. Absent means "the parent".
#[cfg(target_os = "macos")]
const MODE_ENV: &str = "GROK_PROFILE_CONFIG_WRITE_MODE";
#[cfg(target_os = "macos")]
const WORKSPACE_ENV: &str = "GROK_PROFILE_CONFIG_WRITE_WORKSPACE";
#[cfg(target_os = "macos")]
const MODE_CONFINED: &str = "confined";

/// Every line a child reports so the parent can read it back.
#[cfg(target_os = "macos")]
const REPORT: &str = "profile-config-write: ";

const TEST_NAME: &str = "a_seatbelt_confined_session_saves_its_config_through_the_worker";

/// What the confined child saves through the worker.
#[cfg(target_os = "macos")]
const SAVED_CONFIG: &str =
    "[models]\ndefault = \"model-b\"\n\n[ui]\npermission_mode = \"always-approve\"\n";
#[cfg(target_os = "macos")]
const SAVED_TRUST: &str = "version = 1\n";

fn main() {
    // The worker child re-enters this binary with the marker set, exactly as the pager's `main` dispatches it.
    if xai_grok_sandbox::ci_host::is_ci_host_subprocess() {
        xai_grok_sandbox::ci_host::run_ci_host_worker();
        std::process::exit(0);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--list") {
        if !args.iter().any(|arg| arg == "--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    #[cfg(target_os = "macos")]
    match std::env::var(MODE_ENV).as_deref() {
        Ok(MODE_CONFINED) => confined_child(),
        _ => parent(),
    }
    #[cfg(not(target_os = "macos"))]
    println!("skip: the profile sandbox here is the macOS Seatbelt profile");
}

/// The unconfined side: start the worker the shipped way, then send a child
/// into the real `workspace` profile to save its settings.
#[cfg(target_os = "macos")]
fn parent() {
    if !xai_grok_sandbox::SandboxManager::support_info().is_supported {
        println!("{REPORT}skip: no kernel sandbox on this host");
        return;
    }
    let grok_home = fixture_dir("grok-home");
    let workspace = fixture_dir("workspace");
    std::fs::write(grok_home.join("config.toml"), "").expect("seed config.toml");
    // SAFETY: this binary runs one test on one thread, before any child exists.
    unsafe { std::env::set_var("GROK_HOME", &grok_home) };

    let fd = xai_grok_sandbox::ci_host::start_ci_host_for_session(&workspace, false)
        .expect("the hand-off must start a worker");
    xai_grok_sandbox::ci_host::inherit_across_exec(fd).expect("clear close-on-exec");

    let output = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .env(MODE_ENV, MODE_CONFINED)
        .env(WORKSPACE_ENV, &workspace)
        .env("GROK_HOME", &grok_home)
        .env(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV, fd.to_string())
        .output()
        .expect("spawn the confined child");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    println!("{stdout}");
    let report = |key: &str| -> String {
        stdout
            .lines()
            .filter_map(|line| line.strip_prefix(REPORT))
            .find_map(|rest| rest.strip_prefix(key))
            .unwrap_or("?")
            .to_string()
    };
    let context = format!("stdout={stdout} stderr={stderr}");

    assert_eq!(
        report("applied="),
        "1",
        "the child must apply the workspace profile: {context}"
    );
    assert_eq!(
        report("direct_write="),
        "denied",
        "Seatbelt must deny a direct write of config.toml, or this test proves nothing: {context}"
    );
    assert_eq!(
        report("in_process_write="),
        "denied",
        "with no host writer the atomic write must still be denied: {context}"
    );
    assert_eq!(
        report("config_write="),
        "ok",
        "the config.toml save must land: {context}"
    );
    assert_eq!(
        report("trust_write="),
        "ok",
        "the folder-trust save must land: {context}"
    );
    assert_eq!(
        report("other_write="),
        "denied",
        "the worker must not write a file that is not a grok config file: {context}"
    );
    assert_eq!(
        std::fs::read_to_string(grok_home.join("config.toml")).expect("read config.toml"),
        SAVED_CONFIG
    );
    assert_eq!(
        std::fs::read_to_string(grok_home.join("trusted_folders.toml")).expect("read trust store"),
        SAVED_TRUST
    );
    assert!(!grok_home.join("sandbox.toml").exists());
    println!("{REPORT}PASS");
}

/// The confined side: apply the real profile to this process, then save the
/// way grok's config writers do.
#[cfg(target_os = "macos")]
fn confined_child() {
    use xai_grok_config::fs_atomic::write_atomically;
    use xai_grok_sandbox::{ProfileName, SandboxManager};

    let workspace = PathBuf::from(std::env::var(WORKSPACE_ENV).expect(WORKSPACE_ENV));
    let grok_home = PathBuf::from(std::env::var("GROK_HOME").expect("GROK_HOME"));
    let config = grok_home.join("config.toml");

    let mut sandbox = SandboxManager::new(ProfileName::Workspace, &workspace);
    let applied = sandbox.apply(&workspace).is_ok() && sandbox.is_applied();
    println!("{REPORT}applied={}", u8::from(applied));

    let outcome = |result: std::io::Result<()>| match result {
        Ok(()) => "ok".to_string(),
        Err(error) => {
            println!("{REPORT}error={error}");
            "denied".to_string()
        }
    };
    println!(
        "{REPORT}direct_write={}",
        outcome(std::fs::write(&config, "direct"))
    );
    println!(
        "{REPORT}in_process_write={}",
        outcome(write_atomically(&config, "in-process", None))
    );

    xai_grok_sandbox::ci_host::install_host_config_writer();
    println!(
        "{REPORT}config_write={}",
        outcome(write_atomically(&config, SAVED_CONFIG, None))
    );
    println!(
        "{REPORT}trust_write={}",
        outcome(write_atomically(
            &grok_home.join("trusted_folders.toml"),
            SAVED_TRUST,
            Some(0o600)
        ))
    );
    println!(
        "{REPORT}other_write={}",
        outcome(write_atomically(
            &grok_home.join("sandbox.toml"),
            "x = 1\n",
            None
        ))
    );
}

/// A unique directory, canonicalized so the profile's literal deny paths match
/// what Seatbelt resolves (`/var` is a symlink to `/private/var`).
#[cfg(target_os = "macos")]
fn fixture_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "grok-profile-config-write-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create fixture dir");
    dunce::canonicalize(&path).unwrap_or_else(|_| Path::new(&path).to_path_buf())
}
