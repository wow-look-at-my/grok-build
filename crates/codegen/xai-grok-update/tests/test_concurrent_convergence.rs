//! On-disk install probing, the part of the concurrent-updater model that
//! outlives auto-update.
//!
//! `installed_on_disk_version` reads the managed-install symlink, so updaters
//! decide staleness from what is actually installed rather than from their own
//! compiled-in version. The convergence and download-race tests that surrounded
//! it went with the updater -- see test_disabled_pins.rs.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use serial_test::serial;

use common::{
    host_platform, reset_home, small_good_artifact, test_home,
};
use xai_grok_update::version::installed_on_disk_version;

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
    std::fs::write(downloads.join(&name), small_good_artifact()).unwrap();
    std::fs::set_permissions(
        downloads.join(&name),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("../downloads").join(&name),
        bin.join("grok"),
    )
    .unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// Convergence: ensure_latest_on_disk downloads once, then every subsequent
// pass (the leader's hourly re-entry) converges without re-downloading.
// This is the e2e companion to the decision-level tests in
// test_downgrade_matrix.rs — it asserts on actual download invocations.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Convergence: explicit `grok update` (the Ctrl+U fallback path) finds the
// binary another process already installed and skips the download — while
// still returning the target version so stale leaders get signalled.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Installer gating: the disk-version probe must only be trusted for
// installers that actually maintain the managed `~/.grok/bin/grok` symlink
// (internal, gh-release). For npm, a symlink left over from a previous
// internal install LIES about the npm install's version — and in the worst
// direction (leftover "newer" than the registry) it would silently suppress
// npm updates forever.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn disk_probe_preserves_prerelease_versions() {
    let _ = test_home();
    reset_home();
    // An alpha install must read back as the full pre-release version —
    // truncating to "0.1.220" would mask the alpha → stable update.
    fake_managed_install("0.1.220-alpha.4");
    assert_eq!(
        installed_on_disk_version().as_deref(),
        Some("0.1.220-alpha.4")
    );
}

#[tokio::test]
#[serial]
async fn disk_probe_rejects_dangling_symlink() {
    // If the symlink survives but its target binary was deleted (manual
    // ~/.grok/downloads cleanup), the probe must report None — otherwise
    // every updater would claim "already up to date" forever while no
    // runnable binary exists, and nothing would ever repair the install.
    let home = test_home();
    reset_home();
    let platform = host_platform();
    fake_managed_install("0.2.7");
    assert_eq!(installed_on_disk_version().as_deref(), Some("0.2.7"));

    std::fs::remove_file(
        home.join("downloads")
            .join(format!("grok-0.2.7-{platform}")),
    )
    .unwrap();

    assert_eq!(
        installed_on_disk_version(),
        None,
        "a dangling symlink must not report an installed version"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Race integrity: the accepted same-instant race must stay harmless. Two (or
// three) installers running concurrently — even for DIFFERENT versions —
// must never leave a corrupt active binary. Pre-fix, all 0.1.x downloads
// shared one `grok-0.1.tmp`, so a concurrent racer could atomically rename a
// half-written file into place.
// ─────────────────────────────────────────────────────────────────────────────
