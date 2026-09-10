//! Drives the shipped `[jail]` config -> plan -> bwrap/Seatbelt builders end
//! to end against a fixture `config.toml` under an isolated `$GROK_HOME`, and
//! optionally writes the emitted plan + profile to a capture file (the goal's
//! verification artifact).
//!
//! This is a *defaults/subsystem-level* test: it resolves the four defaults from
//! a fixture home and materializes the real plan/builder output the jail would
//! use. It does not exec a nested jail -- a sandboxed process cannot wrap itself
//! again (that is a separate e2e concern, handled by `jail_bwrap_e2e.rs` on a
//! real Linux host). The builder functions it drives are the shipped ones.

#![cfg(unix)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use xai_grok_sandbox::jail::{Access, JailDefaults, build_plan, parse_jail_args};

const GROK_ENV: &str = "JAIL_CFG_GROK_HOME";
const HOME_ENV: &str = "JAIL_CFG_HOME";
const OUT_ENV: &str = "JAIL_CFG_CAPTURE_OUT";
const SECTION_ENV: &str = "JAIL_CFG_SECTION";

/// A fixture home under the caller-owned temp dir (never a shared fixed path).
fn fixture_home(tag: &str) -> (PathBuf, TempGuard) {
    let dir = std::env::temp_dir().join(format!(
        "grok-jail-config-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture home");
    (dir.clone(), TempGuard(dir))
}

struct TempGuard(PathBuf);
impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Re-run this test binary as a subprocess with an isolated `HOME`/`GROK_HOME`
/// and return its captured stdout. `section` is the `[jail]` body to write.
fn run_capture(home: &Path, section: &str) -> (bool, String) {
    let exe = std::env::current_exe().expect("current_exe");
    let grok_home = home.join("grok");
    std::fs::create_dir_all(&grok_home).unwrap();
    let cfg = format!("{section}\n");
    std::fs::write(grok_home.join("config.toml"), cfg).unwrap();
    let out = Command::new(exe)
        .env(GROK_ENV, &grok_home)
        .env(HOME_ENV, home)
        .env(SECTION_ENV, section)
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .arg("config_subprocess")
        .output()
        .expect("spawn subprocess");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The shipped path resolved against the fixture `$GROK_HOME`. Runs only when
/// invoked by the parent test via `run_capture`; asserts on markers via stdout.
#[test]
#[ignore]
fn config_subprocess() {
    let grok_home = std::env::var(GROK_ENV).map(PathBuf::from).expect(GROK_ENV);
    // Set HOME/GROK_HOME before the config OnceLock is first touched, exactly
    // the order a real jailed launch would (isolated subprocess).
    // SAFETY: isolated subprocess; no other test shares this process.
    unsafe {
        if let Ok(h) = std::env::var(HOME_ENV) {
            std::env::set_var("HOME", &h);
        }
        std::env::set_var("GROK_HOME", &grok_home);
        // Fallback used by SOME home resolvers before the canonical GROK_HOME.
        std::env::set_var("XDG_CONFIG_HOME", std::env::temp_dir().join("jail-e2e-xdg"));
    }

    let section = std::env::var(SECTION_ENV).unwrap_or_default();
    let defaults = JailDefaults::load(&grok_home);
    println!("section:\n{}", section);
    println!("defaults: cwd={:?} grok_home={:?} tmp={:?} system={:?}",
        defaults.cwd, defaults.grok_home, defaults.tmp, defaults.system);

    // A bare jail request, as `maybe_reexec_into_jail` would see it.
    let request = parse_jail_args(vec![OsString::from("--sandbox=pathbox")]).unwrap();
    let plan =
        build_plan(&request, &defaults, vec![OsString::from("--sandbox=pathbox")]).unwrap();
    println!("plan:");
    println!("  cwd          = {}", plan.cwd.display());
    println!("  grok_home    = {}", plan.grok_home.display());
    println!("  grok_home_ro = {:?}", defaults.grok_home == Access::Ro);
    for (i, m) in plan.mounts.iter().enumerate() {
        println!("  mount[{i}] {:?} {}", m.access, m.path.display());
    }

    #[cfg(target_os = "linux")]
    {
        let cmd = xai_grok_sandbox::jail::bwrap_command(&plan);
        let argv: Vec<String> =
            cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        println!("bwrap argv: {argv:?}");
    }
    #[cfg(target_os = "macos")]
    {
        println!("seatbelt profile:\n{}", xai_grok_sandbox::jail::seatbelt_profile(&plan));
    }

    // Emit machine-checkable markers so the parent can assert the override
    // actually changed the materialized plan, not just the parsed defaults.
    use std::fmt::Write;
    let mut markers = String::new();
    writeln!(markers, "MARK cwd_default={:?}", defaults.cwd).unwrap();
    writeln!(markers, "MARK grok_home_ro={}", defaults.grok_home == Access::Ro).unwrap();
    writeln!(markers, "MARK tmp={:?}", defaults.tmp).unwrap();
    writeln!(markers, "MARK system_rw={}", defaults.system == Access::Rw).unwrap();
    writeln!(markers, "MARK ring={defaults:?}").unwrap();
    println!("{markers}");
}

/// Capture a default-config and an override-config end to end and write a
/// transcript (plan + platform profile) plus assertions into
/// `$JAIL_CFG_CAPTURE_OUT` when set (the goal scratch artifact). Always asserts
/// the override changed the emitted jail.
#[test]
fn config_defaults_supply_and_override_are_materialized() {
    let (home, _g) = fixture_home("src");

    // 1) No `[jail]` section -> the historical jail.
    let (ok, txt) = run_capture(&home, "");
    assert!(ok, "no-config capture failed:\n{txt}");
    // 2) Every axis overridden -> the struct round-ends and the emitted rules
    //    on this platform reflect a read-only grok home / writable system base.
    let over = "[jail]\ncwd = \"ro\"\ngrok_home = \"ro\"\ntmp = \"rw\"\nsystem = \"rw\"\n";
    let (ok, txt2) = run_capture(&home, over);
    assert!(ok, "override capture failed:\n{txt2}");

    assert!(
        txt2.contains("MARK grok_home_ro=true"),
        "config grok_home=ro must surface in the plan: {txt2}"
    );
    assert!(
        txt2.contains("MARK system_rw=true"),
        "config system=rw must surface in the plan: {txt2}"
    );
    assert!(
        txt2.contains("MARK tmp=Rw"),
        "config tmp=rw must surface in the plan: {txt2}"
    );
    // Contrast: the no-config run keeps release defaults.
    assert!(
        txt.contains("MARK grok_home_ro=false") && txt.contains("MARK system_rw=false"),
        "no config must keep release defaults:\n{txt}"
    );
    assert!(
        txt2.contains("system_rw=true"),
        "system=rw must be in the capture: {txt2}"
    );

    #[cfg(target_os = "macos")]
    {
        // On the macOS Seatbelt backend an ro grok home keeps the read rule and
        // drops the write-allow; system=rw grants writes over /usr.
        assert!(
            txt.contains("(allow file-read* (subpath \"/usr\"))")
                && !txt.contains("(allow file-write* (subpath \"/usr\"))"),
            "no-config profile: read-only base with no /usr write:\n{txt}"
        );
        assert!(
            txt2.contains("(allow file-read* (subpath \"/usr\"))")
                && txt2.contains("(allow file-write* (subpath \"/usr\"))"),
            "override profile: system=rw must also grant /usr writes:\n{txt2}"
        );
    }

    // Persist the transcript for the goal verification artifact.
    if let Ok(out) = std::env::var(OUT_ENV) {
        std::fs::write(
            out,
            format!("### no-config\n{txt}\n### all-axes override\n{txt2}\n"),
        )
        .expect("write capture file");
    }
}
