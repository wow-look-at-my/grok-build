//! Pins the auto-update hard-disable.
//!
//! Every entry point below returns early, ahead of its real body. Tests that
//! drove the updater through them were deleted rather than kept red; these
//! assert the refusal itself, so re-enabling any path has to be deliberate and
//! shows up here first.

use std::path::Path;

use xai_grok_update::auto_update::{download_silent, download_with_progress, get_installer};
use xai_grok_update::version::{UpdateConfig, fetch_latest_version, get_latest_version};

#[tokio::test]
async fn get_installer_reports_no_installer() {
	assert_eq!(
		get_installer().await,
		None,
		"auto-update is disabled: no installer may be advertised"
	);
}

#[tokio::test]
async fn version_lookups_refuse() {
	let config = UpdateConfig {
		proxy_base_url: "http://test.invalid/v1".to_string(),
		auth_scope: "test".to_string(),
		deployment_key: None,
		alpha_test_key: None,
		channel: "stable".to_string(),
		npm_registry: None,
	};
	for installer in ["npm", "gh-release", "internal"] {
		let err = fetch_latest_version(installer, &config)
			.await
			.expect_err("fetch_latest_version must refuse while updates are disabled");
		assert!(
			err.to_string().contains("update checks disabled"),
			"unexpected error for {installer}: {err}"
		);

		let err = get_latest_version(installer, &config)
			.await
			.expect_err("get_latest_version must refuse while updates are disabled");
		assert!(
			err.to_string().contains("update checks disabled"),
			"unexpected error for {installer}: {err}"
		);
	}
}

#[tokio::test]
async fn downloads_refuse_before_touching_the_network_or_disk() {
	let temp = tempfile::TempDir::new().unwrap();
	let dest = temp.path().join("grok");
	// A URL that would fail loudly if anything actually dialled it.
	let url = "http://127.0.0.1:1/grok";

	for result in [
		download_silent(url, &dest).await,
		download_with_progress(url, &dest).await,
	] {
		let err = result.expect_err("downloads must refuse while auto-update is disabled");
		assert!(
			err.to_string().contains("auto-update disabled"),
			"unexpected download error: {err}"
		);
	}
	assert!(
		!Path::new(&dest).exists(),
		"a refused download must not leave a destination file behind"
	);
}
