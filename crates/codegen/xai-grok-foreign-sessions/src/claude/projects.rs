use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) const MAX_PROJECT_DIRS: usize = 16;
pub(super) const MAX_SANITIZED_LENGTH: usize = 200;

pub(super) fn scoped_project_dirs(config_dir: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![cwd.to_path_buf()];
    if let Some(repository) = xai_gix_status::discover::discover(cwd) {
        if let Some(workdir) = repository.workdir() {
            paths.push(dunce::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf()));
            let common_dir = xai_gix_status::discover::common_dir(&repository);
            if xai_gix_status::discover::is_linked_worktree(&repository)
                && let Some(main_workdir) = common_dir.parent()
            {
                paths.push(
                    dunce::canonicalize(main_workdir)
                        .unwrap_or_else(|_| main_workdir.to_path_buf()),
                );
            }
        }
        if let Ok(worktrees) = repository.worktrees() {
            for worktree in worktrees.iter().take(MAX_PROJECT_DIRS) {
                if let Ok(path) = worktree.base() {
                    paths.push(dunce::canonicalize(&path).unwrap_or(path));
                }
            }
        }
    }
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .take(MAX_PROJECT_DIRS)
        .filter_map(|path| project_dir_for_path(config_dir, &path))
        .collect()
}

pub(super) fn project_dir_for_path(config_dir: &Path, path: &Path) -> Option<PathBuf> {
    let project_dir = project_dir_path(config_dir, path)?;
    let metadata = std::fs::symlink_metadata(&project_dir).ok()?;
    (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some(project_dir)
}

pub(super) fn project_dir_path(config_dir: &Path, path: &Path) -> Option<PathBuf> {
    let sanitized = path
        .to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    if sanitized.len() > MAX_SANITIZED_LENGTH {
        return None;
    }
    Some(config_dir.join("projects").join(sanitized))
}
