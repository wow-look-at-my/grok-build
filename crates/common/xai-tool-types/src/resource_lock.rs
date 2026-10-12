//! Inter-agent resource locking: the resource a subagent declares, the lock key derived from it.

use std::path::{Component, Path, PathBuf};

use crate::task::SubagentIsolationMode;

/// The resource a subagent declares it will touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentResource {
    /// A repository or worktree path the child writes under.
    RepoPath(String),
    /// A git branch the child commits to.
    Branch(String),
    /// A declared set of files the child edits.
    FileSet(Vec<String>),
}

/// Identity of one lockable resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceKey(String);

impl ResourceKey {
    /// Key for a repository or worktree path.
    pub fn repo_path(path: &str) -> Self {
        Self(format!("path:{}", normalize_path(path)))
    }

    /// Key for a git branch.
    pub fn branch(name: &str) -> Self {
        Self(format!("branch:{}", name.trim()))
    }

    /// Key for a declared file set. Order does not matter, so the paths are
    /// sorted and de-duplicated before they are joined.
    pub fn file_set(files: &[String]) -> Self {
        let mut sorted: Vec<String> = files.iter().map(|file| normalize_path(file)).collect();
        sorted.sort();
        sorted.dedup();
        Self(format!("files:{}", sorted.join("\n")))
    }

    /// Key for a declared [`SubagentResource`].
    pub fn from_resource(resource: &SubagentResource) -> Self {
        match resource {
            SubagentResource::RepoPath(path) => Self::repo_path(path),
            SubagentResource::Branch(name) => Self::branch(name),
            SubagentResource::FileSet(files) => Self::file_set(files),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Lexically normalize a path so equivalent spellings collide. This is
/// deliberately not `canonicalize`: the resource may not exist yet, and a
/// lock key must not depend on the filesystem.
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

/// What to do about a spawn that collides with a live resource holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceConflictResolution {
    /// Give the new agent its own temporary git worktree.
    SeparateWorktree,
    /// A separate worktree is not possible: wait for the holder to release, within a bounded budget.
    Wait,
}

/// Apply the automatic conflict policy. `isolation` is the spawn's effective
/// isolation, so a role or persona `default_isolation` of `worktree` is
/// honored here too. A separate worktree is chosen when it is allowed, the
/// spawn is not already isolated. No explicit `cwd` pins the child to the
/// contended directory (a worktree and an explicit cwd are mutually
/// exclusive).
pub fn resolve_resource_conflict(
    isolation: SubagentIsolationMode,
    has_explicit_cwd: bool,
    allow_separate_worktree: bool,
) -> ResourceConflictResolution {
    let already_isolated = isolation == SubagentIsolationMode::Worktree;
    if allow_separate_worktree && !already_isolated && !has_explicit_cwd {
        ResourceConflictResolution::SeparateWorktree
    } else {
        ResourceConflictResolution::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalent_path_spellings_share_one_key() {
        assert_eq!(
            ResourceKey::repo_path("/repo/sub/")
                .as_str()
                .replace("path:", ""),
            ResourceKey::repo_path("/repo/sub")
                .as_str()
                .replace("path:", "")
        );
        assert_eq!(
            ResourceKey::repo_path("/repo/./sub"),
            ResourceKey::repo_path("/repo/sub")
        );
        assert_eq!(
            ResourceKey::repo_path("/repo/a/../sub"),
            ResourceKey::repo_path("/repo/sub")
        );
    }

    #[test]
    fn distinct_paths_do_not_collide() {
        assert_ne!(
            ResourceKey::repo_path("/repo/a"),
            ResourceKey::repo_path("/repo/b")
        );
    }

    #[test]
    fn a_path_and_a_branch_never_collide() {
        assert_ne!(ResourceKey::repo_path("main"), ResourceKey::branch("main"));
    }

    #[test]
    fn file_set_order_does_not_matter() {
        let a = ResourceKey::file_set(&["b.rs".into(), "a.rs".into()]);
        let b = ResourceKey::file_set(&["a.rs".into(), "b.rs".into(), "a.rs".into()]);
        assert_eq!(a, b);
    }

    #[test]
    fn from_resource_matches_the_typed_constructor() {
        assert_eq!(
            ResourceKey::from_resource(&SubagentResource::RepoPath("/repo".into())),
            ResourceKey::repo_path("/repo")
        );
        assert_eq!(
            ResourceKey::from_resource(&SubagentResource::Branch("dev".into())),
            ResourceKey::branch("dev")
        );
        assert_eq!(
            ResourceKey::from_resource(&SubagentResource::FileSet(vec!["x".into()])),
            ResourceKey::file_set(&["x".into()])
        );
    }

    #[test]
    fn worktree_is_chosen_only_when_it_can_separate_the_resource() {
        assert_eq!(
            resolve_resource_conflict(SubagentIsolationMode::None, false, true),
            ResourceConflictResolution::SeparateWorktree
        );
        // An explicit cwd pins the child to the contended directory.
        assert_eq!(
            resolve_resource_conflict(SubagentIsolationMode::None, true, true),
            ResourceConflictResolution::Wait
        );
        // The environment cannot create a worktree.
        assert_eq!(
            resolve_resource_conflict(SubagentIsolationMode::None, false, false),
            ResourceConflictResolution::Wait
        );
        // Already isolated: a worktree cannot separate this resource further.
        assert_eq!(
            resolve_resource_conflict(SubagentIsolationMode::Worktree, false, true),
            ResourceConflictResolution::Wait
        );
    }
}
