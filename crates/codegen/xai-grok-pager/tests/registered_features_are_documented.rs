//! `FEATURES` is the source of truth and the operator tables are hand-maintained mirrors with no compile-time check of their own.
//! This test is that check.

use xai_grok_shell::agent::config::FEATURES;

const CONFIG_REFERENCE: &str = include_str!("../docs/user-guide/26-config-reference.md");

#[test]
fn every_registered_feature_reaches_the_operator() {
    for spec in FEATURES {
        assert!(
            CONFIG_REFERENCE.contains(&format!("`features.{}`", spec.key)),
            "{} has no row in 26-config-reference.md",
            spec.key,
        );
        assert!(
            CONFIG_REFERENCE.contains(&format!("`{}`", spec.env)),
            "{} is undocumented in 26-config-reference.md",
            spec.env,
        );
    }
}
