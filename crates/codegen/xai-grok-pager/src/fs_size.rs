//! Sizes are physical (block-based) on Unix and logical `len()` elsewhere.
use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use xai_fast_worktree::is_worktree_dir;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WalkIssues {
    pub(crate) unreadable_dirs: u64,
    pub(crate) unstatable_entries: u64,
    pub(crate) other_filesystems: u64,
}
impl WalkIssues {
    pub(crate) fn skipped(&self) -> u64 {
        self.unreadable_dirs.saturating_add(self.unstatable_entries)
    }
    pub(crate) fn merge(&mut self, other: Self) {
        self.unreadable_dirs = self.unreadable_dirs.saturating_add(other.unreadable_dirs);
        self.unstatable_entries = self
            .unstatable_entries
            .saturating_add(other.unstatable_entries);
        self.other_filesystems = self
            .other_filesystems
            .saturating_add(other.other_filesystems);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Volume(Option<u64>);
impl Volume {
    pub(crate) fn of(path: &Path) -> Self {
        Self(device_id(path))
    }
    #[cfg(test)]
    pub(crate) fn other_device_for_test(self) -> Self {
        Self(Some(self.0.unwrap_or_default().wrapping_add(1)))
    }
    /// False only on a proven mismatch: an unknown device is not a crossing.
    pub(crate) fn holds(self, path: &Path) -> bool {
        let Some(anchor) = self.0 else {
            return true;
        };
        device_id(path).is_none_or(|device| device == anchor)
    }
}
/// What a directory contributes. Nothing measures an `Elsewhere` path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Measure {
    Counted(BucketSize),
    Elsewhere,
}
impl Measure {
    pub(crate) fn bytes(self) -> Option<u64> {
        match self {
            Self::Counted(size) => Some(size.bytes),
            Self::Elsewhere => None,
        }
    }
    pub(crate) fn last_modified(self) -> Option<i64> {
        match self {
            Self::Counted(size) => size.last_modified,
            Self::Elsewhere => None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct DirSize {
    pub(crate) measure: Measure,
    pub(crate) issues: WalkIssues,
}
pub(crate) fn physical_dir_size(path: &Path, volume: Volume) -> DirSize {
    physical_dir_size_with_policy(path, volume, WalkMode::Standard, None)
}
fn physical_dir_size_with_policy(
    path: &Path,
    volume: Volume,
    mode: WalkMode,
    exclude: Option<&Path>,
) -> DirSize {
    let mut size = BucketSize::default();
    let mut issues = WalkIssues::default();
    let entered = walk(path, volume, mode, exclude, &mut issues, |entry| {
        if let Visit::File(_, meta) = entry {
            size.bytes = size.bytes.saturating_add(physical_file_size(meta));
            size.last_modified = size.last_modified.max(modified_at(meta));
        }
    });
    let measure = if entered {
        Measure::Counted(size)
    } else {
        Measure::Elsewhere
    };
    DirSize { measure, issues }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BucketSize {
    pub(crate) bytes: u64,
    pub(crate) last_modified: Option<i64>,
}
#[derive(Debug)]
pub(crate) struct BucketedSizes {
    pub(crate) total: Measure,
    pub(crate) buckets: HashMap<PathBuf, Measure>,
    pub(crate) issues: WalkIssues,
}
/// A grok worktree is a child of a managed root, or a child of that root's per-repository bucket.
const BUCKET_DEPTH_RANGE: std::ops::RangeInclusive<usize> = 1..=2;

/// Which checkout a walked file's bytes belong to, over one DFS walk.
#[derive(Default)]
struct BucketWalk {
    counted: HashMap<PathBuf, BucketSize>,
    elsewhere: Vec<PathBuf>,
    open: Option<PathBuf>,
    open_depth: usize,
}

impl BucketWalk {
    /// Called for every directory the walk reaches, in walk order.
    fn visit_dir(&mut self, path: &Path, depth: usize, off_volume: bool) {
        if self.open.is_some() && depth > self.open_depth {
            return;
        }
        self.open = None;
        // The same question the filesystem scan asks, so the rows the report
        // lists are the rows this walk has bytes for.
        if !BUCKET_DEPTH_RANGE.contains(&depth) || !is_worktree_dir(path) {
            return;
        }
        let path = path.to_path_buf();
        if off_volume {
            self.elsewhere.push(path.clone());
        } else {
            self.counted.entry(path.clone()).or_default();
        }
        self.open = Some(path);
        self.open_depth = depth;
    }

    fn visit_file(&mut self, depth: usize, bytes: u64, modified: Option<i64>) {
        let Some(bucket) = self.open.as_deref().filter(|_| depth > self.open_depth) else {
            return;
        };
        let bucket = self.counted.entry(bucket.to_path_buf()).or_default();
        bucket.bytes = bucket.bytes.saturating_add(bytes);
        bucket.last_modified = bucket.last_modified.max(modified);
    }

    fn into_buckets(self) -> HashMap<PathBuf, Measure> {
        self.counted
            .into_iter()
            .map(|(path, size)| (path, Measure::Counted(size)))
            .chain(
                self.elsewhere
                    .into_iter()
                    .map(|path| (path, Measure::Elsewhere)),
            )
            .collect()
    }
}
pub(crate) fn physical_buckets(root: &Path, volume: Volume) -> BucketedSizes {
    let mut buckets = BucketWalk::default();
    let mut total = BucketSize::default();
    let mut issues = WalkIssues::default();
    let entered = walk(
        root,
        volume,
        WalkMode::Standard,
        None,
        &mut issues,
        |entry| match entry {
            Visit::Dir(entry) => buckets.visit_dir(entry.path(), entry.depth(), false),
            Visit::Elsewhere(entry) => buckets.visit_dir(entry.path(), entry.depth(), true),
            Visit::File(entry, meta) => {
                let bytes = physical_file_size(meta);
                total.bytes = total.bytes.saturating_add(bytes);
                total.last_modified = total.last_modified.max(modified_at(meta));
                buckets.visit_file(entry.depth(), bytes, modified_at(meta));
            }
        },
    );
    let buckets = buckets.into_buckets();
    BucketedSizes {
        total: if entered {
            Measure::Counted(total)
        } else {
            Measure::Elsewhere
        },
        buckets,
        issues,
    }
}
#[derive(Clone, Copy)]
enum WalkMode {
    Standard,
}
enum Visit<'a> {
    Dir(&'a walkdir::DirEntry),
    Elsewhere(&'a walkdir::DirEntry),
    File(&'a walkdir::DirEntry, &'a Metadata),
}
fn walk(
    root: &Path,
    volume: Volume,
    mode: WalkMode,
    exclude: Option<&Path>,
    issues: &mut WalkIssues,
    mut visit: impl FnMut(Visit<'_>),
) -> bool {
    if !volume.holds(root) {
        tracing::debug!(path = %root.display(), "du: directory is on another filesystem");
        issues.other_filesystems = issues.other_filesystems.saturating_add(1);
        return false;
    }
    let mut walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .same_file_system(cfg!(unix))
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::debug!(path = ?e.path(), error = %e, "du: directory could not be read");
                issues.unreadable_dirs = issues.unreadable_dirs.saturating_add(1);
                continue;
            }
        };
        if entry.depth() > 0 && entry.file_type().is_dir() && exclude == Some(entry.path()) {
            walker.skip_current_dir();
            continue;
        }
        if entry.file_type().is_dir() {
            if entry.depth() > 0 && !volume.holds(entry.path()) {
                tracing::debug!(path = %entry.path().display(), "du: directory is on another filesystem");
                if matches!(mode, WalkMode::Standard) {
                    issues.other_filesystems = issues.other_filesystems.saturating_add(1);
                    visit(Visit::Elsewhere(&entry));
                }
                continue;
            }
            visit(Visit::Dir(&entry));
            continue;
        }
        match entry.metadata() {
            Ok(meta) => visit(Visit::File(&entry, &meta)),
            Err(e) => {
                tracing::debug!(path = %entry.path().display(), error = %e, "du: file could not be stat'd");
                issues.unstatable_entries = issues.unstatable_entries.saturating_add(1);
            }
        }
    }
    true
}
pub(crate) fn device_id(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(std::fs::metadata(path).ok()?.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
pub(crate) fn modified_at(meta: &Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64),
    )
}
#[cfg(unix)]
pub(crate) fn volume_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    fn widen(v: impl TryInto<u64>) -> Option<u64> {
        v.try_into().ok()
    }
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(cpath.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    let block = widen(st.f_frsize)
        .filter(|&size| size != 0)
        .or_else(|| widen(st.f_bsize))?;
    #[cfg(not(target_os = "linux"))]
    let block = widen(st.f_bsize)?;
    Some((
        widen(st.f_blocks)?.checked_mul(block)?,
        widen(st.f_bavail)?.checked_mul(block)?,
    ))
}
#[cfg(not(unix))]
pub(crate) fn volume_bytes(_path: &Path) -> Option<(u64, u64)> {
    None
}
pub(crate) fn physical_file_size(meta: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        const ST_BLOCKS_UNIT: u64 = 512;
        meta.blocks().saturating_mul(ST_BLOCKS_UNIT)
    }
    #[cfg(not(unix))]
    {
        meta.len()
    }
}
#[cfg(test)]
#[path = "fs_size_tests.rs"]
mod tests;
