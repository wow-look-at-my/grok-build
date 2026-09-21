//! Drives `ci/cache-deps.sh prune` over a synthetic target directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
	// CARGO_MANIFEST_DIR is <root>/crates/codegen/xai-ci-scripts.
	Path::new(env!("CARGO_MANIFEST_DIR"))
		.ancestors()
		.nth(3)
		.expect("manifest dir has a repo root above it")
		.to_path_buf()
}

/// A `cargo` that answers `metadata` and nothing else, so the check states its own
/// workspace rather than depending on what this repo happens to contain today.
fn stub_cargo(dir: &Path, metadata: &str) -> PathBuf {
	let bin = dir.join("bin");
	fs::create_dir_all(&bin).unwrap();
	let cargo = bin.join("cargo");
	fs::write(&cargo, format!("#!/bin/sh\ncat <<'JSON'\n{metadata}\nJSON\n")).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
	}
	bin
}

fn touch(path: PathBuf) {
	fs::create_dir_all(path.parent().unwrap()).unwrap();
	fs::write(path, b"x").unwrap();
}

#[test]
#[cfg_attr(not(unix), ignore = "cache-deps.sh is a POSIX shell script")]
fn prune_keeps_registry_artifacts_and_takes_every_workspace_one() {
	let tmp = tempfile::tempdir().unwrap();
	let profile = tmp.path().join("target/debug");

	let workspace = [
		"deps/libxai_grok_pager-aaaa.rlib",
		"deps/xai_grok_pager-aaaa.d",
		"deps/libxai_grok_shell-cccc.rlib",
		"deps/libxai_grok_pager_pty_harness-dddd.rlib",
		// Stemmed by TARGET name, with no package name in it anywhere.
		"deps/pty_e2e_smoke-bbbb",
		".fingerprint/xai-grok-pager-aaaa/lib-xai_grok_pager",
		".fingerprint/xai-grok-pager-pty-harness-dddd/lib",
		"build/xai-grok-shell-cccc/output",
	];
	// `xai_grok_pagerish` is the near miss: a bare prefix match takes it and must not.
	let registry = [
		"deps/libserde-1111.rlib",
		"deps/libtokio-2222.rlib",
		"deps/libxai_grok_pagerish-3333.rlib",
		".fingerprint/serde-1111/lib-serde",
		"build/aws-lc-sys-4444/output",
	];
	for rel in workspace.iter().chain(registry.iter()) {
		touch(profile.join(rel));
	}

	let metadata = r#"{"packages":[
 {"name":"xai-grok-pager","targets":[{"name":"xai-grok-pager"},{"name":"pty_e2e_smoke"}]},
 {"name":"xai-grok-shell","targets":[{"name":"xai-grok-shell"}]},
 {"name":"xai-grok-pager-pty-harness","targets":[{"name":"xai-grok-pager-pty-harness"}]}]}"#;
	let bin = stub_cargo(tmp.path(), metadata);
	let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());

	let out = Command::new(repo_root().join("ci/cache-deps.sh"))
		.args(["prune", profile.to_str().unwrap()])
		.env("PATH", path)
		.output()
		.expect("cache-deps.sh runs");
	assert!(
		out.status.success(),
		"prune failed: {}",
		String::from_utf8_lossy(&out.stderr)
	);

	for rel in workspace {
		assert!(
			!profile.join(rel).exists(),
			"{rel} survived the prune, so the cache entry would carry a workspace artifact"
		);
	}
	for rel in registry {
		assert!(
			profile.join(rel).exists(),
			"{rel} was pruned, so the cache entry loses a registry artifact it exists to hold"
		);
	}
}
