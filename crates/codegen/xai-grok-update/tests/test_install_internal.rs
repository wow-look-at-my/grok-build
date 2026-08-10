//! Failure and validation paths of `install_internal` that do not need a
//! working download.
//!
//! What survives here is the rollback behavior when swapping the agent symlink
//! fails, and pinned-version validation. The install pipeline itself (fetch,
//! download, chmod, symlink, cleanup) cannot run with auto-update disabled --
//! see test_disabled_pins.rs.
//!
//! The function reads `grok_home()` (a process-wide `OnceLock`), so all tests
//! in this binary share a single `GROK_HOME` and run serially via `#[serial]`.

#![cfg(unix)]

mod common;

use serial_test::serial;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{reset_home, test_home};
use xai_grok_update::UpdateConfig;
use xai_grok_update::auto_update::{install_internal_from_base, install_internal_from_bases};

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

/// Mount GCS endpoints for a given version. Returns the `MockServer`.
async fn mount_gcs(version: &str, platform: &str) -> MockServer {
    let server = MockServer::start().await;

    // Channel pointer: stable returns this version.
    Mock::given(method("GET"))
        .and(path("/stable"))
        .respond_with(ResponseTemplate::new(200).set_body_string(version))
        .mount(&server)
        .await;

    // Main grok binary download.
    Mock::given(method("GET"))
        .and(path(format!("/grok-{version}-{platform}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"#!/bin/sh\nexit 0\n".to_vec()))
        .mount(&server)
        .await;

    server
}

// ─────────────────────────────────────────────────────────────────────────────
// Happy-path
// ─────────────────────────────────────────────────────────────────────────────

/// Rollback regression: if `agent` swap fails after `grok` succeeded,
/// `grok` must roll back to its prior target (all-or-nothing).
#[tokio::test]
#[serial]
async fn install_internal_rolls_back_grok_when_agent_swap_fails() {
    let _ = test_home();
    reset_home();
    let platform = host_platform();
    let server = mount_gcs("0.1.181", &platform).await;
    let cfg = make_config("stable");

    let home = test_home();
    let bin_dir = home.join("bin");
    let download_dir = home.join("downloads");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::create_dir_all(&download_dir).unwrap();
    let old_binary = download_dir.join(format!("grok-0.1.180-{platform}"));
    std::fs::write(&old_binary, b"#!/bin/sh\nexit 0\n").unwrap();
    let rel_old = std::path::Path::new("..")
        .join("downloads")
        .join(format!("grok-0.1.180-{platform}"));
    std::os::unix::fs::symlink(&rel_old, bin_dir.join("grok")).unwrap();

    // Sabotage the agent swap: non-empty directory → rename fails with EISDIR.
    let agent_dir = bin_dir.join("agent");
    std::fs::create_dir(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("blocker"), b"x").unwrap();

    let err = install_internal_from_base(Some("0.1.181"), &cfg, &server.uri())
        .await
        .expect_err("agent swap must fail when target is a non-empty dir");
    drop(err);

    // grok must be rolled back to the prior version.
    let grok_target = std::fs::read_link(bin_dir.join("grok")).unwrap();
    assert_eq!(
        grok_target.file_name().unwrap(),
        format!("grok-0.1.180-{platform}").as_str(),
        "grok must be rolled back when agent swap fails"
    );
}

/// Absent-prior rollback regression: fresh install (no prior `grok` /
/// `agent`), sabotaged `agent` swap must *remove* the just-created `grok`
/// link so we don't leave it on the new binary while `agent` is absent.
#[tokio::test]
#[serial]
async fn install_internal_rollback_removes_absent_prior_grok_link() {
    let _ = test_home();
    reset_home();
    let platform = host_platform();
    let server = mount_gcs("0.1.181", &platform).await;
    let cfg = make_config("stable");

    let home = test_home();
    let bin_dir = home.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // No prior `grok`. Sabotage `agent` swap: non-empty directory → EISDIR.
    let agent_dir = bin_dir.join("agent");
    std::fs::create_dir(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("blocker"), b"x").unwrap();
    assert!(
        !bin_dir.join("grok").exists() && !bin_dir.join("grok").is_symlink(),
        "precondition: grok must not exist before install",
    );

    let err = install_internal_from_base(Some("0.1.181"), &cfg, &server.uri())
        .await
        .expect_err("agent swap must fail when target is a non-empty dir");
    drop(err);

    let grok_path = bin_dir.join("grok");
    assert!(
        !grok_path.is_symlink() && !grok_path.exists(),
        "grok must be removed on rollback when there was no prior link",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Failure paths
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn install_internal_rejects_invalid_pinned_version() {
    let _ = test_home();
    reset_home();
    let server = MockServer::start().await;
    let cfg = make_config("stable");

    let err = install_internal_from_base(Some("not-a-version"), &cfg, &server.uri())
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("invalid version format"), "msg: {msg}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Cleanup integration: install v1, then v2, verify N-1 retention.
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Multi-base URL fallback: install_internal_from_bases tries each base in
// preference order, falling through to the next on failure.
// ─────────────────────────────────────────────────────────────────────────────

/// Regression: a local failure after a successful download (sabotaged
/// `agent` swap) must fail the install immediately — the fallback base must
/// never be contacted for a pointless re-download.
#[tokio::test]
#[serial]
async fn install_internal_from_bases_does_not_redownload_on_local_swap_failure() {
    let _ = test_home();
    reset_home();
    let platform = host_platform();

    let primary = mount_gcs("0.1.181", &platform).await;
    let fallback = mount_gcs("0.1.181", &platform).await;
    let cfg = make_config("stable");

    let home = test_home();
    let bin_dir = home.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // Sabotage activation: agent as a non-empty dir fails the swap's
    // rollback capture (read_link on a directory) before any rename.
    let agent_dir = bin_dir.join("agent");
    std::fs::create_dir(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("blocker"), b"x").unwrap();

    install_internal_from_bases(
        Some("0.1.181"),
        &cfg,
        &[primary.uri().as_str(), fallback.uri().as_str()],
    )
    .await
    .expect_err("swap failure must fail the install");

    let fallback_requests = fallback
        .received_requests()
        .await
        .expect("request recording is enabled on MockServer::start()");
    assert!(
        fallback_requests.is_empty(),
        "local swap failure must not fall through to the next base: {} request(s)",
        fallback_requests.len()
    );
}
