use std::ffi::OsString;
use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone)]
pub(super) struct ApprovedRoot {
    path: PathBuf,
    directory: Arc<File>,
}

pub(super) struct OpenedRegularFile {
    pub file: File,
    pub path: PathBuf,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DirectoryVisit {
    pub visited: usize,
    pub complete: bool,
}

impl ApprovedRoot {
    pub fn new(path: &Path) -> Option<Self> {
        let path = dunce::canonicalize(path).ok()?;
        #[cfg(unix)]
        let directory = unix::open_directory_path(&path)?;
        #[cfg(windows)]
        let directory = windows::open_directory_path(&path)?;
        #[cfg(not(any(unix, windows)))]
        let directory: File = return None;

        let metadata = directory.metadata().ok()?;
        if !metadata.is_dir() || has_reparse_point(&metadata) {
            return None;
        }
        #[cfg(windows)]
        if !windows::directory_path_matches(&path, &directory) {
            return None;
        }
        Some(Self {
            path,
            directory: Arc::new(directory),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }

    #[allow(dead_code)]
    pub fn modified(&self) -> Option<SystemTime> {
        self.directory.metadata().ok()?.modified().ok()
    }

    pub fn subroot(&self, path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            let relative = self.relative_path(path)?;
            let directory = unix::open_directory_relative(&self.directory, &relative)?;
            let metadata = directory.metadata().ok()?;
            if !metadata.is_dir() {
                return None;
            }
            Some(Self {
                path: self.path.join(relative),
                directory: Arc::new(directory),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }

    pub fn for_each_entry(&self, visit: impl FnMut(OsString)) -> bool {
        #[cfg(unix)]
        {
            unix::visit_directory_names(&self.directory, visit)
        }
        #[cfg(not(unix))]
        {
            let _ = visit;
            false
        }
    }

    pub fn for_each_entry_bounded(
        &self,
        max_entries: usize,
        visit: impl FnMut(OsString),
    ) -> DirectoryVisit {
        #[cfg(unix)]
        {
            unix::visit_directory_names_bounded(&self.directory, max_entries, visit)
        }
        #[cfg(not(unix))]
        {
            let _ = (max_entries, visit);
            DirectoryVisit {
                visited: 0,
                complete: false,
            }
        }
    }

    fn relative_path(&self, path: &Path) -> Option<PathBuf> {
        let relative = if path.is_absolute() {
            // `self.path` is always canonical. Prefer a pure strip so openat
            // paths still resolve after the on-disk entry is replaced
            // (symlink swap); fall back to canonicalize for non-canonical
            // absolute inputs.
            path.strip_prefix(&self.path)
                .ok()
                .map(|r| r.to_path_buf())
                .or_else(|| {
                    let absolute = dunce::canonicalize(path).ok()?;
                    absolute
                        .strip_prefix(&self.path)
                        .ok()
                        .map(|r| r.to_path_buf())
                })?
        } else {
            path.to_path_buf()
        };
        relative
            .components()
            .all(|component| {
                matches!(
                    component,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
            .then_some(relative)
    }

    pub fn resolve_regular_file(&self, path: &Path) -> Option<(PathBuf, Metadata)> {
        let opened = self.open_regular_file(path)?;
        Some((opened.path, opened.metadata))
    }

    pub fn open_regular_file(&self, path: &Path) -> Option<OpenedRegularFile> {
        #[cfg(unix)]
        {
            let relative = self.relative_path(path)?;
            let file = unix::open_regular_relative(&self.directory, &relative)?;
            let metadata = file.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some(OpenedRegularFile {
                file,
                path: self.path.join(relative),
                metadata,
            })
        }
        #[cfg(windows)]
        {
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.join(path)
            };
            let parent = dunce::canonicalize(path.parent()?).ok()?;
            if !parent.starts_with(&self.path) {
                return None;
            }
            let path = parent.join(path.file_name()?);
            let expected = std::fs::symlink_metadata(&path).ok()?;
            if !expected.is_file()
                || expected.file_type().is_symlink()
                || has_reparse_point(&expected)
            {
                return None;
            }
            let canonical_path = dunce::canonicalize(&path).ok()?;
            if canonical_path.parent() != Some(parent.as_path())
                || !canonical_path.starts_with(&self.path)
            {
                return None;
            }
            let file = windows::open_regular_path(&canonical_path)?;
            let metadata = file.metadata().ok()?;
            if !metadata.is_file()
                || has_reparse_point(&metadata)
                || !windows::canonical_file_matches(&canonical_path, &file)
            {
                return None;
            }
            return Some(OpenedRegularFile {
                file,
                path: canonical_path,
                metadata,
            });
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            None
        }
    }
}

#[cfg(windows)]
fn has_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn has_reparse_point(_metadata: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests;
