//! Installed grok CLI version, lockstepped with shipping binaries.

use semver::Version;

pub const TEST_VERSION_ENV: &str = "GROK_TEST_VERSION";

pub const VERSION: &str = match option_env!("GROK_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// [`TEST_VERSION_ENV`] override first, then [`VERSION`]. Trimmed so
/// non-semver-aware callers can pass the result straight into parsing.
pub fn installed() -> String {
    std::env::var(TEST_VERSION_ENV)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| VERSION.to_string())
}

pub fn installed_semver() -> Result<Version, semver::Error> {
    Version::parse(&installed())
}

/// Format the compiled version with a channel label for user-facing display.
///
/// `channel_label` is a pre-formatted suffix such as `" [alpha]"`, `" [stable]"`,
/// or `""` (empty when no cached pointer is available). Obtain it from
/// `xai_grok_update::channel_label()`.
///
/// Example: `"0.2.5 [stable]"` or `"0.2.5 [alpha]"`.
pub fn display_version(channel_label: &str) -> String {
    format!("{}{}", VERSION, channel_label)
}

/// Format a version-with-commit string with a channel label.
///
/// Same semantics as [`display_version`] but for the full
/// `"0.2.5 (abc1234)"` string.
pub fn display_version_with_commit(version_with_commit: &str, channel_label: &str) -> String {
    format!("{}{}", version_with_commit, channel_label)
}

/// The full 40-char commit hash the binary was built from, stamped by `build.rs`
/// via `cargo:rustc-env=BUILD_COMMIT`. Falls back to `"unknown"` when the build
/// ran outside a git worktree (e.g. a tarball).
pub const BUILD_COMMIT: &str = match option_env!("BUILD_COMMIT") {
    Some(c) => c,
    None => "unknown",
};

/// The short commit hash the binary was built from (same source as
/// [`BUILD_COMMIT`] but truncated by `git rev-parse --short`). Falls back to
/// `"unknown"` outside a git worktree.
pub const BUILD_COMMIT_SHORT: &str = match option_env!("BUILD_COMMIT_SHORT") {
    Some(c) => c,
    None => "unknown",
};

/// The fixed GitHub owner/repo for the `wow-look-at-my/grok-build` remote.
const GITHUB_REPO: &str = "wow-look-at-my/grok-build";

/// Build the GitHub commit URL for a given commit hash.
///
/// Returns `https://github.com/wow-look-at-my/grok-build/commit/<hash>`.
/// When `hash` is `"unknown"` (build ran outside a git worktree) this returns
/// `None`, signalling that the caller should render plain text without a link
/// rather than emitting a malformed `…/commit/unknown` hyperlink.
pub fn commit_github_url(hash: &str) -> Option<String> {
    if hash.is_empty() || hash == "unknown" {
        return None;
    }
    Some(format!(
        "https://github.com/{}/commit/{}",
        GITHUB_REPO, hash
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display formatting invariant matrix — verifies label appending
    /// works correctly across all label states (alpha, stable, empty).
    #[test]
    fn test_display_version_formatting_matrix() {
        let cases: &[(&str, &str, &str)] = &[
            // (version_with_commit,    label,        expected_suffix)
            ("0.2.5 (abc1234)", " [alpha]", "0.2.5 (abc1234) [alpha]"),
            ("0.2.5 (abc1234)", " [stable]", "0.2.5 (abc1234) [stable]"),
            ("0.2.5 (abc1234)", "", "0.2.5 (abc1234)"),
            (
                "0.1.220-alpha.2 (def0)",
                " [alpha]",
                "0.1.220-alpha.2 (def0) [alpha]",
            ),
        ];
        for (vwc, label, expected) in cases {
            assert_eq!(
                display_version_with_commit(vwc, label),
                *expected,
                "display_version_with_commit({:?}, {:?})",
                vwc,
                label,
            );
        }
        // display_version uses compiled VERSION — just verify the label appends
        assert_eq!(display_version(""), VERSION);
        assert!(display_version(" [stable]").ends_with("[stable]"));
    }

    /// `commit_github_url` with a full 40-char hash — the primary use case,
    /// since the link target must point at the unambiguous full commit.
    #[test]
    fn test_commit_github_url_full_hash() {
        let hash = "11cc538ef81131e8a6a730a431e36784c0d488b9";
        assert_eq!(
            commit_github_url(hash).as_deref(),
            Some("https://github.com/wow-look-at-my/grok-build/commit/11cc538ef81131e8a6a730a431e36784c0d488b9"),
        );
    }

    /// `commit_github_url` with a short hash — still produces a valid link
    /// (GitHub resolves short hashes in commit URLs).
    #[test]
    fn test_commit_github_url_short_hash() {
        assert_eq!(
            commit_github_url("11cc538").as_deref(),
            Some("https://github.com/wow-look-at-my/grok-build/commit/11cc538"),
        );
    }

    /// `"unknown"` fallback (build outside a git worktree) must NOT produce a
    /// link — returning `None` so the caller renders plain text.
    #[test]
    fn test_commit_github_url_unknown_returns_none() {
        assert_eq!(commit_github_url("unknown"), None);
    }

    /// Empty string is also treated as "no link available".
    #[test]
    fn test_commit_github_url_empty_returns_none() {
        assert_eq!(commit_github_url(""), None);
    }
}
