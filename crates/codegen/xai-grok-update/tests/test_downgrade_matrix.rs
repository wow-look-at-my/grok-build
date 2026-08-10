//! Rollback/downgrade decisions that survive the auto-update disable.
//!
//! What remains is the reporting and disk-inspection side: whether a rolled-back
//! pointer is advertised as an update, and what the on-disk symlink says is
//! installed. Anything that had to fetch a version or install a binary is gone
//! with the updater itself -- see test_disabled_pins.rs.

#![cfg(unix)]

mod common;

use serial_test::serial;

use common::{FakeBinGuard, reset_home, set_test_version, test_home};
use xai_grok_update::UpdateConfig;
use xai_grok_update::auto_update::{auto_update_target, check_update_status, ensure_latest_on_disk};
use xai_grok_update::version::installed_on_disk_version;

fn host_platform() -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        panic!("unsupported test platform");
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        panic!("unsupported test arch");
    };
    format!("{os}-{arch}")
}

fn make_config(channel: &str) -> UpdateConfig {
    UpdateConfig {
        proxy_base_url: "http://test.invalid/v1".to_string(),
        auth_scope: "test".to_string(),
        deployment_key: None,
        alpha_test_key: None,
        channel: channel.to_string(),
        npm_registry: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// check_update_status across installer × version direction, driven by fake
// npm/gh binaries. The internal (GCS) path has hardcoded URLs and cannot be
// reached this way; its update-detection logic lives in the needs_update unit
// tests.
// ─────────────────────────────────────────────────────────────────────────────

fn setup_npm(current_version: &str) -> FakeBinGuard {
    let _ = test_home();
    reset_home();
    set_test_version(current_version);
    // SAFETY: serial_test ensures no race; reset_home clears this between tests.
    unsafe { std::env::set_var("GROK_INSTALLER", "npm") };
    FakeBinGuard::install_npm()
}

fn setup_gh(current_version: &str) -> FakeBinGuard {
    let _ = test_home();
    reset_home();
    set_test_version(current_version);
    // SAFETY: serial_test ensures no race; reset_home clears this between tests.
    unsafe { std::env::set_var("GROK_INSTALLER", "gh-release") };
    FakeBinGuard::install_gh()
}

// ── npm: never downgrades ──

#[tokio::test]
#[serial]
async fn npm_same_version_no_update() {
    let g = setup_npm("0.2.7");
    g.set_stdout("\"0.2.7\"");

    let status = check_update_status(&make_config("stable")).await;
    assert!(!status.update_available);
}

#[tokio::test]
#[serial]
async fn npm_rollback_does_not_report_update() {
    // Stable pointer rolled back 0.2.7 → 0.2.5. npm user on 0.2.7 must NOT
    // see an update — stale registries make this path unsafe.
    let g = setup_npm("0.2.7");
    g.set_stdout("\"0.2.5\"");

    let status = check_update_status(&make_config("stable")).await;
    assert!(
        !status.update_available,
        "npm must never report a downgrade: current={} latest={:?}",
        status.current_version, status.latest_version
    );
}

#[tokio::test]
#[serial]
async fn npm_drastically_old_registry_does_not_report_update() {
    // Corporate registry returns ancient version.
    let g = setup_npm("0.2.7");
    g.set_stdout("\"0.1.4\"");

    let status = check_update_status(&make_config("stable")).await;
    assert!(!status.update_available);
}

// ── gh-release: --check is upgrade-only; rollback handled by auto-install ──

#[tokio::test]
#[serial]
async fn gh_release_same_version_no_update() {
    let g = setup_gh("0.2.7");
    g.set_stable_only_stdout("v0.2.7\n");

    let status = check_update_status(&make_config("stable")).await;
    assert!(!status.update_available);
}

// ─────────────────────────────────────────────────────────────────────────────
// auto_update_target: the leader/background auto-install decision
//
// Unlike the upgrade-only `check_update_status` report, this is the
// downgrade-aware convergence decision. It gates on the installer, so
// authoritative installers (gh-release/internal) follow a rolled-back pointer
// while npm never downgrades. `fetch_latest_version` keeps these hermetic.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn auto_update_target_gh_release_same_version_returns_none() {
    let g = setup_gh("0.2.7");
    g.set_stable_only_stdout("v0.2.7\n");

    assert_eq!(auto_update_target(&make_config("stable")).await, None);
}

#[tokio::test]
#[serial]
async fn auto_update_target_npm_rollback_returns_none() {
    // npm registries can serve stale versions — never downgrade npm installs.
    let g = setup_npm("0.2.26");
    g.set_stdout("\"0.2.22\"");

    assert_eq!(
        auto_update_target(&make_config("stable")).await,
        None,
        "npm must never be downgraded even when the registry reports an older version"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Disk-aware convergence: ensure_latest_on_disk + installed_on_disk_version
//
// Concurrent updaters (TUI background download, leader hourly checker,
// explicit `grok update`) must decide staleness from the on-disk install, not
// their own compiled-in version — a binary another process already installed
// is never downloaded a second time, but a stale running process still gets
// the relaunch signal.
// ─────────────────────────────────────────────────────────────────────────────

/// Lay down a managed-install layout in the test GROK_HOME:
/// `bin/grok -> ../downloads/grok-<version>-<platform>` (what
/// `install_internal_from_base` produces).
fn fake_managed_install(version: &str) {
    let home = test_home();
    let downloads = home.join("downloads");
    let bin = home.join("bin");
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let name = format!("grok-{version}-{}", host_platform());
    std::fs::write(downloads.join(&name), b"#!/bin/sh\nexit 0\n").unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("../downloads").join(&name),
        bin.join("grok"),
    )
    .unwrap();
}

#[tokio::test]
#[serial]
async fn installed_on_disk_version_reads_symlink_target() {
    let _ = test_home();
    reset_home();
    assert_eq!(installed_on_disk_version(), None, "no install yet");

    fake_managed_install("0.2.7");
    assert_eq!(installed_on_disk_version().as_deref(), Some("0.2.7"));
}

#[tokio::test]
#[serial]
async fn ensure_latest_noop_when_running_and_disk_current() {
    let g = setup_gh("0.2.7");
    g.set_stable_only_stdout("v0.2.7\n");
    fake_managed_install("0.2.7");

    let outcome = ensure_latest_on_disk(&make_config("stable")).await.unwrap();
    assert_eq!(outcome.installed, None);
    assert!(!outcome.relaunch_needed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Pointer-flip timing scenarios
//
// These test the race between a user opening grok (which caches the version)
// and a pointer flip happening. The 30-min TTL means the user won't see the
// new pointer until the cache expires, but once it does, the correct behavior
// must kick in.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Double-rollback scenario
// ─────────────────────────────────────────────────────────────────────────────
