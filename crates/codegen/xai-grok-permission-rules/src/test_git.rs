//! Repository fixtures for tests, built with the `git` CLI and the machine's own config masked.

use std::path::Path;

pub(crate) use xai_test_utils::git::run_git as git;

/// The empty tree, which every repository has without storing it.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// `git init` at `path`, creating the directory first.
pub(crate) fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    xai_test_utils::git::init_git_repo(path);
}

/// `git init --bare` at `path`, creating the directory first.
pub(crate) fn init_bare(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--bare"]);
}

/// Commit the index as it stands, empty or not, and return the new HEAD.
pub(crate) fn commit(path: &Path, message: &str) -> String {
    git(path, &["commit", "--allow-empty", "-m", message]);
    git(path, &["rev-parse", "HEAD"])
}

/// [`init`] plus one empty commit. Returns the commit.
pub(crate) fn init_with_commit(path: &Path) -> String {
    init(path);
    commit(path, "init")
}

/// Point the HEAD branch of the bare repository at `bare` to a new empty-tree commit.
pub(crate) fn commit_empty_bare(bare: &Path) -> String {
    let commit = git(bare, &["commit-tree", EMPTY_TREE, "-m", "init"]);
    git(bare, &["update-ref", "HEAD", &commit]);
    commit
}

/// Add a linked worktree at `path` on a new branch named after the last path component.
pub(crate) fn worktree_add(repo: &Path, path: &Path) {
    git(repo, &["worktree", "add", path.to_str().unwrap()]);
}

/// Add a linked worktree at `path` that checks out the existing `branch`.
pub(crate) fn worktree_add_branch(repo: &Path, path: &Path, branch: &str) {
    git(repo, &["worktree", "add", path.to_str().unwrap(), branch]);
}
