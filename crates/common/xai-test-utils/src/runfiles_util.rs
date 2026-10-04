//! Bazel runfiles helpers for locating test data.

use std::path::PathBuf;

/// Try to resolve a runfiles path to an absolute directory.
///
/// Returns `Some(path)` when running under Bazel (with the `bazel` feature
/// enabled) and the runfiles entry exists, `None` otherwise.
pub fn try_resolve_runfiles(_path: &str) -> Option<PathBuf> {
    #[cfg(feature = "bazel")]
    {
        let r = runfiles::Runfiles::create().ok()?;
        runfiles::rlocation!(r, _path)
    }
    #[cfg(not(feature = "bazel"))]
    {
        None
    }
}

/// Resolve the crate root directory, working under both `bazel test` and
/// `cargo test`.
#[macro_export]
macro_rules! crate_root {
    ($runfiles_path:expr) => {
        $crate::runfiles_util::try_resolve_runfiles($runfiles_path)
            .unwrap_or_else(|| ::std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    };
}
