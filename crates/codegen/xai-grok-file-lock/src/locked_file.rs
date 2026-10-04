use std::fs::File;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

/// An exclusive advisory lock on a file. Sites write holder stamps through
/// `DerefMut<Target = File>`.
#[must_use]
#[derive(Debug)]
pub struct LockedFile {
    file: File,
    path: PathBuf,
    unlocked: bool,
}

impl LockedFile {
    pub(crate) fn new(file: File, path: PathBuf) -> Self {
        LockedFile {
            file,
            path,
            unlocked: false,
        }
    }

    /// The lock file's path as passed to `lock_file`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Release the lock and surface the error `Drop` would only log. The file closes on return.
    pub fn unlock(mut self) -> io::Result<()> {
        self.unlocked = true;
        self.file.unlock()
    }
}

impl Deref for LockedFile {
    type Target = File;

    fn deref(&self) -> &File {
        &self.file
    }
}

impl DerefMut for LockedFile {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl AsRef<File> for LockedFile {
    fn as_ref(&self) -> &File {
        &self.file
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        if self.unlocked {
            return;
        }
        if let Err(e) = self.file.unlock() {
            tracing::debug!(
                path = %self.path.display(),
                error = %e,
                "failed to release file lock on drop"
            );
        }
    }
}
