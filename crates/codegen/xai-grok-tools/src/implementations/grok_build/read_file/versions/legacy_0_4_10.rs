
use std::path::Path;

/// Exact historical read failure message for `read_file` in legacy-0.4.10.
pub(crate) fn render_read_error(path: &Path) -> String {
    format!("Failed to read file: {}", path.display())
}

pub(crate) fn allows_gitignored_reads() -> bool {
    true
}
