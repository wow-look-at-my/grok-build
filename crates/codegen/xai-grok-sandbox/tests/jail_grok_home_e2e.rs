//! A settings save inside the real jail, through a symlinked `~/.grok` and a
//! `config.toml` that links into a dotfiles folder.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use xai_grok_sandbox::jail::{JailDefaults, build_plan, parse_jail_args};

/// Selects what a re-run of this binary does: `plan`, `plan-unfixed` or `save`.
const MODE_ENV: &str = "JAIL_GROK_HOME_MODE";

fn backend_is_usable() -> bool {
    #[cfg(target_os = "linux")]
    let status = Command::new("bwrap")
        .args(["--dev-bind", "/", "/", "true"])
        .status();
    #[cfg(target_os = "macos")]
    let status = Command::new("sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .status();
    status.is_ok_and(|status| status.success())
}

/// Under the target directory: the jail hides or overlays `/tmp`.
fn fixture_root() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "grok-jail-home-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    dunce::canonicalize(&path).unwrap()
}

struct Fixture {
    root: PathBuf,
    /// The real file a `config.toml` save must reach.
    config: PathBuf,
    /// The real folder a new settings file must land in.
    grok_home: PathBuf,
}

impl Fixture {
    /// `home/.grok` links to `dotfiles/grok`, whose `config.toml` links to
    /// `dotfiles/conf/config.toml`.
    fn new() -> Self {
        let root = fixture_root();
        for dir in ["home", "dotfiles/grok", "dotfiles/conf", "work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("dotfiles/conf/config.toml"), "a = 1\n").unwrap();
        std::os::unix::fs::symlink(root.join("dotfiles/grok"), root.join("home/.grok")).unwrap();
        std::os::unix::fs::symlink(
            root.join("dotfiles/conf/config.toml"),
            root.join("dotfiles/grok/config.toml"),
        )
        .unwrap();
        Fixture {
            config: root.join("dotfiles/conf/config.toml"),
            grok_home: root.join("dotfiles/grok"),
            root,
        }
    }

    /// An ordinary `home/.grok` folder with a plain `config.toml`.
    fn plain() -> Self {
        let root = fixture_root();
        for dir in ["home/.grok", "work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("home/.grok/config.toml"), "a = 1\n").unwrap();
        Fixture {
            config: root.join("home/.grok/config.toml"),
            grok_home: root.join("home/.grok"),
            root,
        }
    }

    /// Run this binary as a fresh process with the fixture as its home.
    fn run(&self, mode: &str) -> (bool, String) {
        let output = Command::new(std::env::current_exe().unwrap())
            .env(MODE_ENV, mode)
            .env("HOME", self.root.join("home"))
            .env_remove("GROK_HOME")
            .env_remove("XDG_CACHE_HOME")
            .current_dir(self.root.join("work"))
            .args(["--ignored", "--exact", "--nocapture", "jail_child"])
            .output()
            .expect("re-run the test binary");
        (
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    }

    fn config(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap()
    }

    fn assert_saved(&self, out: &str) {
        assert!(out.contains("SAVED"), "{out}");
        assert_eq!(
            self.config(),
            "a = 2\n",
            "the save must reach the real config.toml: {out}"
        );
        assert_eq!(
            std::fs::read_to_string(self.grok_home.join("pager.toml")).unwrap(),
            "b = 1\n",
            "a new settings file must land in the real grok home: {out}"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn settings_save_through_a_symlinked_grok_home_and_a_linked_config() {
    if !backend_is_usable() {
        eprintln!("SKIP: the sandbox backend cannot build a jail on this host");
        return;
    }
    let fixture = Fixture::new();

    // Without the binds, Linux reports success into a throwaway `~/.grok`.
    let (_, out) = fixture.run("plan-unfixed");
    assert_eq!(fixture.config(), "a = 1\n", "{out}");
    assert!(
        !fixture.grok_home.join("pager.toml").exists(),
        "without the binds no save may reach the real grok home: {out}"
    );

    let (ok, out) = fixture.run("plan");
    assert!(ok, "the jailed save must succeed: {out}");
    fixture.assert_saved(&out);
}

#[test]
fn settings_save_through_a_plain_grok_home() {
    if !backend_is_usable() {
        eprintln!("SKIP: the sandbox backend cannot build a jail on this host");
        return;
    }
    let fixture = Fixture::plain();
    let (ok, out) = fixture.run("plan-plain");
    assert!(ok, "the jailed save must succeed: {out}");
    fixture.assert_saved(&out);
}

/// The re-run half. `plan` builds the shipped jail and runs this binary in
/// it in `save` mode. `save` writes settings the way grok does.
#[test]
#[ignore]
fn jail_child() {
    match std::env::var(MODE_ENV).as_deref() {
        Ok("save") => save_settings(),
        Ok("plan-plain") => run_in_jail(false, false),
        Ok(mode @ ("plan" | "plan-unfixed")) => run_in_jail(true, mode == "plan-unfixed"),
        _ => {}
    }
}

fn run_in_jail(symlinked: bool, unfixed: bool) {
    let args = vec![OsString::from("--sandbox=pathbox")];
    let request = parse_jail_args(args.clone()).unwrap();
    let mut plan = build_plan(&request, &JailDefaults::default(), args).unwrap();
    assert_eq!(
        plan.grok_home_alias.is_some(),
        symlinked,
        "only a symlinked ~/.grok needs an alias"
    );
    assert_eq!(
        !plan.grok_home_links.is_empty(),
        symlinked,
        "only a linked config.toml needs its folder"
    );
    if unfixed {
        plan.grok_home_alias = None;
        plan.grok_home_links.clear();
    }
    plan.self_exe = std::env::current_exe().unwrap();
    plan.args = ["--ignored", "--exact", "--nocapture", "jail_child"]
        .into_iter()
        .map(OsString::from)
        .collect();
    #[cfg(target_os = "linux")]
    let mut cmd = xai_grok_sandbox::jail::bwrap_command(&plan);
    #[cfg(target_os = "macos")]
    let mut cmd = xai_grok_sandbox::jail::seatbelt_command(&plan);
    let status = cmd.env(MODE_ENV, "save").status().expect("start the jail");
    assert!(status.success(), "the jailed save failed: {status}");
}

fn save_settings() {
    let grok_home = xai_dirs::grok_home();
    let config =
        xai_grok_config::fs_atomic::bind_follow_destination(&grok_home.join("config.toml"))
            .expect("resolve config.toml inside the jail");
    xai_grok_config::fs_atomic::write_atomically_bound(&config, "a = 2\n", None)
        .expect("save config.toml inside the jail");
    xai_grok_config::fs_atomic::write_atomically_resolved(
        &grok_home.join("pager.toml"),
        "b = 1\n",
        None,
    )
    .expect("save a new settings file inside the jail");
    println!("SAVED");
}
