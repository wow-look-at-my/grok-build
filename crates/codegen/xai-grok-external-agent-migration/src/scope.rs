use std::path::{Path, PathBuf};

#[must_use]
pub fn find_project_root(cwd: &Path) -> PathBuf {
    xai_gix_status::discover::discover_workdir(cwd).unwrap_or_else(|| cwd.to_path_buf())
}
