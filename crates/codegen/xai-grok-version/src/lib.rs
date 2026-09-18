//! Installed grok CLI version, lockstepped with shipping binaries.

use semver::Version;
use std::sync::OnceLock;

pub const TEST_VERSION_ENV: &str = "GROK_TEST_VERSION";

/// Byte pattern `xai-grok-stamp` searches the linked binary for.
///
/// The release number is written into the binary AFTER it links, so a build
/// needs no release number and never waits on one. Compiling the number in is
/// what forced the whole release path to run before the build.
pub const STAMP_MAGIC: &[u8; 16] = b"\0GROK-VER-STAMP\0";

/// Payload bytes reserved after the magic: one length byte, then the version.
pub const STAMP_PAYLOAD_LEN: usize = 64;

/// Total slot width. [`STAMP_SLOT`] is this long and the stamper writes within it.
pub const STAMP_SLOT_LEN: usize = STAMP_MAGIC.len() + 1 + STAMP_PAYLOAD_LEN;

/// The slot itself. A zero length byte is the unstamped state, which is what a
/// local build and every CI test build carry.
///
/// `#[used]` and `#[no_mangle]` keep it in the binary: nothing reads it through
/// this symbol, and a plain `static` the optimizer sees no load of is free to
/// disappear.
#[used]
#[no_mangle]
pub static STAMP_SLOT: [u8; STAMP_SLOT_LEN] = build_stamp_slot();

/// The magic followed by a zero length and zero payload.
const fn build_stamp_slot() -> [u8; STAMP_SLOT_LEN] {
    let mut slot = [0u8; STAMP_SLOT_LEN];
    let mut i = 0;
    while i < STAMP_MAGIC.len() {
        slot[i] = STAMP_MAGIC[i];
        i += 1;
    }
    slot
}

/// The stamped release number, or `None` on an unstamped binary.
///
/// The read is volatile because the slot is an immutable `static` whose contents
/// the compiler otherwise knows: it would fold the zero length in at compile
/// time and never look at the bytes the stamper wrote.
fn stamped() -> Option<&'static str> {
    static STAMPED: OnceLock<Option<String>> = OnceLock::new();
    STAMPED
        .get_or_init(|| {
            let slot = unsafe { std::ptr::read_volatile(&STAMP_SLOT) };
            let len = usize::from(slot[STAMP_MAGIC.len()]);
            if len == 0 || len > STAMP_PAYLOAD_LEN {
                return None;
            }
            let start = STAMP_MAGIC.len() + 1;
            let text = std::str::from_utf8(&slot[start..start + len]).ok()?;
            Some(text.to_string())
        })
        .as_deref()
}

/// The version this binary reports: the post-link stamp when it carries one,
/// else the crate's own version.
pub fn version() -> &'static str {
    stamped().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Whether this binary carries a release stamp. False for a local build and for
/// every binary CI tests, which is what makes those builds local-looking.
pub fn is_release_stamped() -> bool {
    stamped().is_some()
}

/// `"<version> (<short commit>)"` — the string `--version` prints.
pub fn version_with_commit() -> &'static str {
    static COMBINED: OnceLock<String> = OnceLock::new();
    COMBINED.get_or_init(|| format!("{} ({})", version(), BUILD_COMMIT_SHORT))
}

/// [`TEST_VERSION_ENV`] override first, then [`version`]. Trimmed so
/// non-semver-aware callers can pass the result straight into parsing.
pub fn installed() -> String {
    std::env::var(TEST_VERSION_ENV)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| version().to_string())
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
    format!("{}{}", version(), channel_label)
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
        // display_version reads the stamp — just verify the label appends
        assert_eq!(display_version(""), version());
        assert!(display_version(" [stable]").ends_with("[stable]"));
    }

    /// The slot the stamper searches for must be in this binary, must carry the
    /// magic, and must read as unstamped until something writes a length.
    #[test]
    fn an_unstamped_slot_reads_as_no_release() {
        let slot = unsafe { std::ptr::read_volatile(&STAMP_SLOT) };
        assert_eq!(&slot[..STAMP_MAGIC.len()], &STAMP_MAGIC[..]);
        assert_eq!(slot[STAMP_MAGIC.len()], 0, "length byte starts at zero");
        assert_eq!(stamped(), None);
        assert!(!is_release_stamped());
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    /// `version_with_commit` is what `--version` prints, so it must carry both
    /// halves in the shape the update checker parses back.
    #[test]
    fn version_with_commit_carries_version_and_commit() {
        let combined = version_with_commit();
        assert!(combined.starts_with(version()), "{combined}");
        assert!(
            combined.ends_with(&format!("({})", BUILD_COMMIT_SHORT)),
            "{combined}",
        );
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
