use super::*;

#[test]
fn strip_url_credentials_removes_token() {
    let url_with_token = "https://x-access-token:secret-token@github.com/xai-org/example.git";
    assert_eq!(
        strip_url_credentials(url_with_token),
        "https://github.com/xai-org/example.git"
    );
}

#[test]
fn strip_url_credentials_preserves_clean_https_url() {
    let clean_url = "https://github.com/xai-org/example.git";
    assert_eq!(strip_url_credentials(clean_url), clean_url);
}

#[test]
fn strip_url_credentials_preserves_ssh_url() {
    let ssh_url = "git@github.com:xai-org/example.git";
    assert_eq!(strip_url_credentials(ssh_url), ssh_url);
}

#[test]
fn strip_url_credentials_removes_username_password() {
    let url_with_creds = "https://alice:secret@github.com/xai-org/example.git";
    assert_eq!(
        strip_url_credentials(url_with_creds),
        "https://github.com/xai-org/example.git"
    );
}

#[test]
fn scrub_git_output_removes_tokens_from_error_text() {
    // git echoes the remote URL (with any token) in push/fetch errors.
    let raw = "fatal: unable to access \
         'https://x-access-token:ghs_SECRET@github.com/acme/app.git/': \
         The requested URL returned error: 403\n";
    let scrubbed = scrub_git_output(raw);
    assert!(!scrubbed.contains("ghs_SECRET"), "{scrubbed}");
    assert!(!scrubbed.contains("x-access-token"), "{scrubbed}");
    assert!(scrubbed.contains("github.com/acme/app.git"), "{scrubbed}");
    // Non-URL text and line structure are preserved.
    assert!(scrubbed.contains("error: 403"));
}

#[test]
fn scrub_git_output_leaves_clean_output_untouched() {
    let raw =
        "To https://github.com/acme/app.git\n ! [rejected]  main -> main (non-fast-forward)\n";
    assert_eq!(scrub_git_output(raw), raw);
}

#[tokio::test]
#[cfg_attr(
    not(unix),
    ignore = "test invokes git CLI which is not always available"
)]
async fn commit_turn_if_dirty_creates_real_commit() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    git_cli(tmp.path(), &["init", "-b", "main"]).await.unwrap();
    git_cli(tmp.path(), &["config", "user.email", "t@example.com"])
        .await
        .unwrap();
    git_cli(tmp.path(), &["config", "user.name", "t"])
        .await
        .unwrap();
    std::fs::write(tmp.path().join("a.txt"), "one").unwrap();
    git_cli(tmp.path(), &["add", "a.txt"]).await.unwrap();
    git_cli(tmp.path(), &["commit", "-m", "base"])
        .await
        .unwrap();
    std::fs::write(tmp.path().join("a.txt"), "two").unwrap();
    let sha = commit_turn_if_dirty(tmp.path(), 7)
        .await
        .expect("dirty turn commits");
    let head = get_current_commit(tmp.path()).await.unwrap();
    assert_eq!(sha, head);
    let msg = git_cli(tmp.path(), &["log", "-1", "--pretty=%s"])
        .await
        .unwrap();
    assert_eq!(msg.trim(), "turn 7");
    assert!(
        commit_turn_if_dirty(tmp.path(), 8).await.is_none(),
        "clean tree must not create an empty commit"
    );
}

#[tokio::test]
#[cfg_attr(
    not(unix),
    ignore = "test invokes git CLI which is not always available"
)]
async fn commit_turn_if_dirty_ignores_ambient_gpgsign() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    git_cli(tmp.path(), &["init", "-b", "main"]).await.unwrap();
    git_cli(tmp.path(), &["config", "user.email", "t@example.com"])
        .await
        .unwrap();
    git_cli(tmp.path(), &["config", "user.name", "t"])
        .await
        .unwrap();
    // Ambient signing on with no usable key would make an unattended commit fail; the helper must force `commit.gpgsign=false` and still commit
    git_cli(tmp.path(), &["config", "commit.gpgsign", "true"])
        .await
        .unwrap();
    git_cli(tmp.path(), &["config", "gpg.program", "/bin/false"])
        .await
        .unwrap();
    std::fs::write(tmp.path().join("a.txt"), "one").unwrap();
    let sha = commit_turn_if_dirty(tmp.path(), 11)
        .await
        .expect("turn commit must succeed despite ambient commit.gpgsign=true");
    assert_eq!(sha, get_current_commit(tmp.path()).await.unwrap());
}

#[tokio::test]
#[cfg_attr(
    not(unix),
    ignore = "test invokes git CLI which is not always available"
)]
async fn commit_turn_if_dirty_skips_when_operation_in_progress() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    git_cli(tmp.path(), &["init", "-b", "main"]).await.unwrap();
    git_cli(tmp.path(), &["config", "user.email", "t@example.com"])
        .await
        .unwrap();
    git_cli(tmp.path(), &["config", "user.name", "t"])
        .await
        .unwrap();
    std::fs::write(tmp.path().join("a.txt"), "one").unwrap();
    git_cli(tmp.path(), &["add", "a.txt"]).await.unwrap();
    git_cli(tmp.path(), &["commit", "-m", "base"])
        .await
        .unwrap();
    let base = get_current_commit(tmp.path()).await.unwrap();

    // Simulate an in-progress merge and a dirty tree: the turn commit must refuse rather than finalize the merge
    std::fs::write(tmp.path().join(".git").join("MERGE_HEAD"), &base).unwrap();
    std::fs::write(tmp.path().join("a.txt"), "two").unwrap();

    assert!(
        commit_turn_if_dirty(tmp.path(), 9).await.is_none(),
        "must not commit while a merge is in progress"
    );
    assert_eq!(
        base,
        get_current_commit(tmp.path()).await.unwrap(),
        "HEAD must be unchanged when a git operation is in progress"
    );
}

#[tokio::test]
#[cfg_attr(
    not(unix),
    ignore = "test invokes git CLI which is not always available"
)]
async fn git_cli_scrubs_token_from_transport_failure() {
    // Same guard as the other git-CLI fixture tests (unavailable under Bazel).
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    git_cli(tmp.path(), &["init", "-b", "main"]).await.unwrap();

    // Fetch from a remote whose URL embeds a token and that refuses immediately (port 1)
    // git echoes the full URL (including the token) in its stderr
    // The `git_cli` failure branch must scrub it before returning/logging, so even a routine EnsureBinding fetch failure cannot leak the token
    let err = git_cli(
        tmp.path(),
        &[
            "fetch",
            "https://x-access-token:ghs_SECRET@127.0.0.1:1/acme/app.git",
            "main",
        ],
    )
    .await
    .expect_err("fetch to a refused endpoint must fail");
    let msg = err.to_string();
    assert!(!msg.contains("ghs_SECRET"), "token leaked: {msg}");
    assert!(!msg.contains("x-access-token"), "userinfo leaked: {msg}");
}

#[test]
fn detect_default_branch_prefers_remote_head_then_config() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    crate::test_git::init(dir);
    // Repo-local value shields the assertions from the host's global config.
    crate::test_git::git(dir, &["config", "init.defaultBranch", "trunk"]);
    let oid = crate::test_git::commit(dir, "initial");
    let repo = || crate::git_repo::discover(dir).unwrap();

    // With no remote HEAD, detection falls back to config
    assert_eq!(detect_default_branch(&repo()).as_deref(), Some("trunk"));

    // A lone remote-tracking branch without `origin/HEAD` still falls back to config
    // Detection reads the `origin/HEAD` symbolic ref and never guesses from the remote-tracking branches
    crate::test_git::git(dir, &["update-ref", "refs/remotes/origin/main", &oid]);
    assert_eq!(detect_default_branch(&repo()).as_deref(), Some("trunk"));

    // `origin/HEAD` symbolic ref wins over config.
    crate::test_git::git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    assert_eq!(detect_default_branch(&repo()).as_deref(), Some("main"));
}

#[test]
fn test_resolve_persisted_session_git_metadata_collects_sorted_unique_remotes() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());
    let add_remote =
        |name: &str, url: &str| crate::test_git::git(tmp.path(), &["remote", "add", name, url]);
    add_remote(
        "origin",
        "https://x-access-token:secret-token@github.com/xai-org/example.git",
    );
    // Use a different host to avoid CI insteadOf rules collapsing URLs.
    add_remote("backup", "https://gitlab.com/xai-org/example.git");
    // Same effective URL as origin after credential stripping, so it tests dedup
    add_remote("duplicate", "https://github.com/xai-org/example.git");

    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());

    assert_eq!(
        dunce::canonicalize(Path::new(metadata.git_root_dir.as_deref().unwrap())).unwrap(),
        dunce::canonicalize(tmp.path()).unwrap(),
    );
    // Sorted, deduplicated, credentials stripped.
    assert_eq!(
        metadata.git_remotes,
        vec![
            "https://github.com/xai-org/example.git".to_string(),
            "https://gitlab.com/xai-org/example.git".to_string(),
        ]
    );
}

#[test]
fn test_resolve_persisted_session_git_metadata_captures_head() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());
    crate::test_git::git(
        tmp.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/xai-org/example.git",
        ],
    );

    // Before any commit, HEAD is unborn, so both fields are None
    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());
    assert!(metadata.head_commit.is_none());
    assert!(metadata.head_branch.is_none());

    // Create initial commit on default branch.
    let commit_oid = crate::test_git::commit(tmp.path(), "initial");

    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());
    assert_eq!(metadata.head_commit.as_deref(), Some(commit_oid.as_str()));
    assert!(metadata.head_branch.is_some());

    // Create a second commit and verify HEAD updates.
    let commit2 = crate::test_git::commit(tmp.path(), "second");

    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());
    assert_eq!(metadata.head_commit.as_deref(), Some(commit2.as_str()));
}

#[test]
fn test_resolve_persisted_session_git_metadata_detached_head() {
    let tmp = tempfile::tempdir().unwrap();
    let commit_oid = crate::test_git::init_with_commit(tmp.path());

    crate::test_git::git(tmp.path(), &["checkout", "--detach", &commit_oid]);

    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());
    assert_eq!(metadata.head_commit.as_deref(), Some(commit_oid.as_str()),);
    // Detached HEAD yields None for branch, matching `git branch --show-current` in the git watcher path
    assert!(metadata.head_branch.is_none());
}

#[test]
fn test_resolve_persisted_session_git_metadata_worktree_resolves_remotes() {
    // Set up a main repo with a remote, then create a worktree
    // resolve_persisted_session_git_metadata_sync on the worktree cwd must follow the commondir back to the shared config
    let tmp = tempfile::tempdir().unwrap();
    let main_path = tmp.path().join("main-repo");
    std::fs::create_dir_all(&main_path).unwrap();

    crate::test_git::init(&main_path);
    crate::test_git::git(
        &main_path,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/xai-org/example.git",
        ],
    );

    // Create an initial commit so we can create a worktree.
    crate::test_git::commit(&main_path, "init");

    // Create a new branch for the worktree (can't check out the same branch).
    crate::test_git::git(&main_path, &["branch", "wt-branch"]);

    let wt_path = tmp.path().join("my-worktree");
    crate::test_git::worktree_add_branch(&main_path, &wt_path, "wt-branch");

    // Verify from the worktree cwd: the remotes resolve through the commondir
    let metadata = resolve_persisted_session_git_metadata_sync(&wt_path);

    assert_eq!(
        dunce::canonicalize(Path::new(metadata.git_root_dir.as_deref().unwrap())).unwrap(),
        dunce::canonicalize(&wt_path).unwrap(),
    );
    assert_eq!(
        metadata.git_remotes,
        vec!["https://github.com/xai-org/example.git".to_string()],
    );
}

fn init_repo_on_branch(path: &Path, branch: &str) {
    crate::test_git::init(path);
    std::fs::write(path.join("README"), "test\n").unwrap();
    crate::test_git::git(path, &["add", "README"]);
    crate::test_git::commit(path, "init");
    let current = crate::test_git::git(path, &["branch", "--show-current"]);
    if current != branch {
        crate::test_git::git(path, &["checkout", "-f", "-B", branch]);
    }
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_all(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn checkout_named_branch(repo_path: &Path, branch: &str) {
    if crate::test_git::git(repo_path, &["branch", "--list", branch]).is_empty() {
        crate::test_git::git(repo_path, &["branch", branch]);
    }
    crate::test_git::git(repo_path, &["checkout", "-f", branch]);
}

fn collapse_home_for_test(path: &Path) -> String {
    collapse_home_path(path, std::env::var("HOME").ok().as_deref().map(Path::new))
}

#[test]
fn collapse_home_path_requires_whole_component() {
    let home = Path::new("/Users/u");
    assert_eq!(
        collapse_home_path(Path::new("/Users/user/xai"), Some(home)),
        "/Users/user/xai"
    );
    assert_eq!(
        collapse_home_path(Path::new("/Users/u/src/repo"), Some(home)),
        "~/src/repo"
    );
}

#[tokio::test]
async fn get_worktree_info_standalone_grok_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let clone = tmp.path().join("clone");
    init_repo_on_branch(&main, "main-only");
    copy_dir_all(&main, &clone);
    checkout_named_branch(&clone, "wt-branch");
    std::fs::write(
        clone.join(".git").join("grok-worktree-source"),
        main.display().to_string(),
    )
    .unwrap();
    assert!(clone.join(".git").is_dir());
    assert!(clone.join(".git").join("grok-worktree-source").is_file());

    let (is_wt, main_repo) = get_worktree_info(&clone).await.expect("clone is a repo");
    assert!(is_wt);
    assert_eq!(
        main_repo.as_deref(),
        Some(collapse_home_for_test(&main).as_str())
    );
    assert_eq!(get_branch(&clone).await.as_deref(), Some("wt-branch"));

    let nested = clone.join("sub").join("dir");
    std::fs::create_dir_all(&nested).unwrap();
    let (nested_wt, nested_main) = get_worktree_info(&nested)
        .await
        .expect("nested path is in the clone");
    assert!(nested_wt);
    assert_eq!(
        nested_main.as_deref(),
        Some(collapse_home_for_test(&main).as_str())
    );
}

#[tokio::test]
async fn get_worktree_info_nested_plain_repo_does_not_inherit_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let clone = tmp.path().join("clone");
    init_repo_on_branch(&main, "main-only");
    copy_dir_all(&main, &clone);
    checkout_named_branch(&clone, "wt-branch");
    std::fs::write(
        clone.join(".git").join("grok-worktree-source"),
        main.display().to_string(),
    )
    .unwrap();

    let nested = clone.join("vendor").join("dep");
    init_repo_on_branch(&nested, "dep-branch");
    let (is_wt, main_repo) = get_worktree_info(&nested)
        .await
        .expect("nested init is a repo");
    assert!(!is_wt);
    assert!(main_repo.is_none());
    assert_eq!(get_branch(&nested).await.as_deref(), Some("dep-branch"));
}

#[tokio::test]
async fn get_worktree_info_tilde_collapses_home_prefix() {
    let Some(home) = xai_dirs::home_dir() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("clone");
    init_repo_on_branch(&clone, "wt-branch");
    let fake_main = home.join("xai-fake-main-repo-for-wt-display");
    std::fs::write(
        clone.join(".git").join("grok-worktree-source"),
        fake_main.display().to_string(),
    )
    .unwrap();
    let (is_wt, main_repo) = get_worktree_info(&clone).await.expect("clone is a repo");
    assert!(is_wt);
    assert_eq!(
        main_repo.as_deref(),
        Some("~/xai-fake-main-repo-for-wt-display")
    );
}

#[tokio::test]
async fn get_worktree_info_plain_repo_is_not_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_on_branch(tmp.path(), "main");
    assert_eq!(get_worktree_info(tmp.path()).await, Some((false, None)));
}

fn block_on_worktree_info(cwd: &Path) -> Option<(bool, Option<String>)> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(get_worktree_info(cwd))
}

fn register_db_worktree(home: &Path, wt: &Path, source: &Path, label: &str) {
    let db = xai_fast_worktree::db::WorktreeDb::open(home).unwrap();
    db.register(&xai_fast_worktree::db::WorktreeRecord {
        id: "db-wt".into(),
        path: dunce::canonicalize(wt).unwrap_or_else(|_| wt.to_path_buf()),
        source_repo: source.to_path_buf(),
        repo_name: "main-repo".into(),
        kind: xai_fast_worktree::db::WorktreeKind::Session,
        creation_mode: "standalone".into(),
        git_ref: None,
        head_commit: None,
        session_id: None,
        creator_pid: None,
        created_at: 1,
        last_accessed_at: None,
        status: xai_fast_worktree::db::WorktreeStatus::Alive,
        metadata: Some(serde_json::json!({ "label": label })),
    })
    .unwrap();
}

#[test]
fn get_worktree_info_db_record_without_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let home = dunce::canonicalize(tmp.path()).unwrap().join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::LockedTestEnv::lock().set("GROK_HOME", &home);

    let wt = tmp.path().join("clone");
    init_repo_on_branch(&wt, "wt-branch");
    let source = PathBuf::from("/src/main-repo");
    register_db_worktree(&home, &wt, &source, "db-label");

    let (is_wt, main_repo) = block_on_worktree_info(&wt).expect("clone is a repo");
    assert!(is_wt);
    assert_eq!(
        main_repo.as_deref(),
        Some(collapse_home_for_test(&source).as_str())
    );
}

#[test]
fn get_worktree_info_nested_repo_does_not_inherit_db_record() {
    let tmp = tempfile::tempdir().unwrap();
    let home = dunce::canonicalize(tmp.path()).unwrap().join("grok-home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::LockedTestEnv::lock().set("GROK_HOME", &home);

    let wt = tmp.path().join("clone");
    init_repo_on_branch(&wt, "wt-branch");
    register_db_worktree(&home, &wt, Path::new("/src/main-repo"), "db-label");

    let nested = wt.join("vendor").join("dep");
    init_repo_on_branch(&nested, "dep-branch");
    let (is_wt, main_repo) = block_on_worktree_info(&nested).expect("nested init is a repo");
    assert!(!is_wt);
    assert!(main_repo.is_none());
}

#[tokio::test]
async fn get_worktree_info_linked_git_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    init_repo_on_branch(&main, "main-only");
    crate::test_git::git(&main, &["branch", "wt-branch"]);
    let wt_path = tmp.path().join("my-worktree");
    crate::test_git::worktree_add_branch(&main, &wt_path, "wt-branch");
    assert!(wt_path.join(".git").is_file());

    let (is_wt, main_repo) = get_worktree_info(&wt_path)
        .await
        .expect("linked worktree is a repo");
    assert!(is_wt);
    let main_repo = main_repo.expect("linked worktree has main_repo");
    let expected = collapse_home_for_test(&main);
    let expected_canon = dunce::canonicalize(&main)
        .map(|p| collapse_home_for_test(&p))
        .unwrap_or_else(|_| expected.clone());
    assert!(
        main_repo == expected || main_repo == expected_canon,
        "main_repo={main_repo}, expected {expected} or {expected_canon}"
    );
}

#[test]
fn test_strip_prefix_canonicalized_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let child = root.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();

    let result = strip_prefix_canonicalized(&child, root);
    assert_eq!(result.as_deref(), Some(Path::new("a/b")));
}

#[test]
fn test_strip_prefix_canonicalized_same_dir_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert!(strip_prefix_canonicalized(dir, dir).is_none());
}

#[test]
fn test_strip_prefix_canonicalized_unrelated_returns_none() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    assert!(strip_prefix_canonicalized(a.path(), b.path()).is_none());
}

#[test]
fn test_strip_prefix_canonicalized_nonexistent_child() {
    // Deleted files can't be canonicalized, so the helper falls back to a raw match
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let deleted = root.join("gone").join("file.txt");
    // Don't create `deleted` on disk.
    let result = strip_prefix_canonicalized(&deleted, root);
    assert_eq!(result.as_deref(), Some(Path::new("gone/file.txt")));
}

#[test]
fn test_effective_worktree_path_no_git_root() {
    let wt = Path::new("/worktrees/repo/abc");
    let result = effective_worktree_path(wt, Path::new("/repo/src"), None);
    assert_eq!(result, wt);
}

#[test]
fn test_effective_worktree_path_at_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // source_cwd == git_root, so there is no offset
    let result = effective_worktree_path(Path::new("/wt"), root, Some(root));
    assert_eq!(result, Path::new("/wt"));
}

#[test]
fn test_effective_worktree_path_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sub = root.join("pkg").join("foo");
    std::fs::create_dir_all(&sub).unwrap();

    let wt = Path::new("/worktrees/repo/abc");
    let result = effective_worktree_path(wt, &sub, Some(root));
    assert_eq!(result, wt.join("pkg/foo"));
}

#[test]
fn test_effective_worktree_path_non_prefix() {
    // source_cwd is not under git_root, so it returns worktree_root unchanged
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let wt = Path::new("/wt");
    let result = effective_worktree_path(wt, a.path(), Some(b.path()));
    assert_eq!(result, wt);
}

#[test]
fn test_effective_worktree_cwd_empty_offset() {
    let result = effective_worktree_cwd("/home/user/.grok/worktrees/repo/ab-123-a", Path::new(""));
    assert_eq!(result, "/home/user/.grok/worktrees/repo/ab-123-a");
}

#[test]
fn test_effective_worktree_cwd_single_level_offset() {
    let result =
        effective_worktree_cwd("/home/user/.grok/worktrees/repo/ab-123-a", Path::new("src"));
    assert_eq!(result, "/home/user/.grok/worktrees/repo/ab-123-a/src");
}

#[test]
fn test_effective_worktree_cwd_nested_offset() {
    let result = effective_worktree_cwd(
        "/home/user/.grok/worktrees/repo/ab-123-b",
        Path::new("packages/frontend/src"),
    );
    assert_eq!(
        result,
        "/home/user/.grok/worktrees/repo/ab-123-b/packages/frontend/src"
    );
}

#[test]
fn test_effective_worktree_cwd_no_trailing_slash() {
    let root = "/worktree/path";
    let result = effective_worktree_cwd(root, Path::new(""));
    assert!(!result.ends_with('/'));
}

#[test]
fn test_compute_subdir_offset_at_git_root() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path();
    crate::test_git::init(repo_root);

    let (offset, git_root) = compute_subdir_offset(&repo_root.to_string_lossy());
    assert!(
        offset.as_os_str().is_empty(),
        "offset should be empty at repo root, got {:?}",
        offset
    );
    assert_eq!(
        dunce::canonicalize(Path::new(&git_root)).unwrap(),
        dunce::canonicalize(repo_root).unwrap(),
    );
}

#[test]
fn test_compute_subdir_offset_in_subdirectory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path();
    crate::test_git::init(repo_root);

    let sub = repo_root.join("packages").join("frontend");
    std::fs::create_dir_all(&sub).unwrap();

    let (offset, git_root) = compute_subdir_offset(&sub.to_string_lossy());
    assert_eq!(
        offset,
        Path::new("packages/frontend"),
        "offset should be the relative path from git root to the subdir"
    );
    assert_eq!(
        dunce::canonicalize(Path::new(&git_root)).unwrap(),
        dunce::canonicalize(repo_root).unwrap(),
    );
}

#[test]
fn test_compute_subdir_offset_deeply_nested() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path();
    crate::test_git::init(repo_root);

    let deep = repo_root.join("a").join("b").join("c").join("d");
    std::fs::create_dir_all(&deep).unwrap();

    let (offset, _git_root) = compute_subdir_offset(&deep.to_string_lossy());
    assert_eq!(offset, Path::new("a/b/c/d"));
}

#[test]
fn test_compute_subdir_offset_not_a_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let non_git = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&non_git).unwrap();

    let cwd_str = non_git.to_string_lossy().to_string();
    let (offset, root) = compute_subdir_offset(&cwd_str);
    assert!(offset.as_os_str().is_empty());
    assert_eq!(root, cwd_str);
}

#[test]
fn test_effective_cwd_roundtrip_with_compute_offset() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_root = tmp.path();
    crate::test_git::init(repo_root);

    let sub = repo_root.join("src").join("lib");
    std::fs::create_dir_all(&sub).unwrap();

    let (offset, _git_root) = compute_subdir_offset(&sub.to_string_lossy());

    let worktree_root = "/home/user/.grok/worktrees/myrepo/ab-test-a";
    let effective = effective_worktree_cwd(worktree_root, &offset);
    assert_eq!(effective, format!("{}/src/lib", worktree_root));
}

// Tests for find_git_root_from_path, the function new_session uses to populate isGitRepo / gitRoot in the session metadata

#[test]
fn test_find_git_root_from_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());

    let root = find_git_root_from_path(tmp.path()).unwrap();
    // Canonicalize both sides so macOS /private/... symlinks don't trip up the comparison.
    assert_eq!(
        dunce::canonicalize(&root).unwrap(),
        dunce::canonicalize(tmp.path()).unwrap()
    );
}

#[test]
fn test_find_git_root_from_subdir_returns_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());

    let sub = tmp.path().join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();

    let root = find_git_root_from_path(&sub).unwrap();
    assert_eq!(
        dunce::canonicalize(&root).unwrap(),
        dunce::canonicalize(tmp.path()).unwrap()
    );
}

#[test]
fn test_find_git_root_outside_repo_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    // No git init, just a plain directory
    assert!(find_git_root_from_path(tmp.path()).is_err());
}

// Tests for discover_git_root, the typed wrapper new_session uses to decide whether to show the non-git warning

#[test]
fn test_discover_git_root_found_at_repo_root() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());

    match discover_git_root(tmp.path()) {
        GitDiscoveryResult::Found(root) => {
            assert_eq!(
                dunce::canonicalize(&root).unwrap(),
                dunce::canonicalize(tmp.path()).unwrap()
            );
        }
        other => panic!("expected Found, got {:?}", std::mem::discriminant(&other)),
    }
}

#[test]
fn test_discover_git_root_found_from_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());

    let sub = tmp.path().join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();

    match discover_git_root(&sub) {
        GitDiscoveryResult::Found(root) => {
            assert_eq!(
                dunce::canonicalize(&root).unwrap(),
                dunce::canonicalize(tmp.path()).unwrap()
            );
        }
        other => panic!("expected Found, got {:?}", std::mem::discriminant(&other)),
    }
}

#[test]
fn test_discover_git_root_not_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    // No git init.
    assert!(matches!(
        discover_git_root(tmp.path()),
        GitDiscoveryResult::NotARepo
    ));
}

#[test]
fn test_discover_git_root_bare_repo_returns_discovery_failed() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init_bare(tmp.path());

    // Bare repos have no workdir: the repo exists but a working directory can't be extracted
    assert!(
        matches!(
            discover_git_root(tmp.path()),
            GitDiscoveryResult::DiscoveryFailed(_)
        ),
        "bare repo should return DiscoveryFailed, not NotARepo"
    );
}

#[test]
fn test_parse_numstat_basic() {
    let output = "10\t2\tsrc/main.rs\n3\t0\tREADME.md\n";
    let stats = parse_numstat(output);
    assert_eq!(stats.get("src/main.rs"), Some(&(10, 2)));
    assert_eq!(stats.get("README.md"), Some(&(3, 0)));
}

#[test]
fn test_parse_numstat_binary() {
    let output = "-\t-\timage.png\n";
    let stats = parse_numstat(output);
    assert_eq!(stats.get("image.png"), Some(&(0, 0)));
}

#[test]
fn test_parse_numstat_empty() {
    let stats = parse_numstat("");
    assert!(stats.is_empty());
}

#[tokio::test]
async fn diffs_head_to_working_reports_untracked_file_additions() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());
    std::fs::write(tmp.path().join("README.md"), "hello\n").unwrap();
    crate::test_git::git(tmp.path(), &["add", "README.md"]);
    crate::test_git::commit(tmp.path(), "init");

    std::fs::write(tmp.path().join("new.txt"), "line1\nline2\nline3\n").unwrap();

    let data = diffs(tmp.path(), None, "HEAD", "working", false, false, false)
        .await
        .unwrap();

    let file = data
        .files
        .iter()
        .find(|f| f.path == "new.txt")
        .expect("untracked file missing from HEAD→working diff");
    assert_eq!(file.additions, 3);
    assert_eq!(file.deletions, 0);
    assert!(matches!(file.change_type, ChangeType::Untracked));
}

#[test]
fn test_parse_porcelain_v2_ordinary() {
    let output = "1 M. N... 100644 100644 100644 abc123 def456 src/lib.rs\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::from([("src/lib.rs".to_string(), (10, 2))]),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 1);
    assert_eq!(unstaged.len(), 0);
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .path,
        "src/lib.rs"
    );
    assert!(matches!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .change_type,
        ChangeType::Edit
    ));
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .additions,
        10
    );
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .deletions,
        2
    );
}

#[test]
fn test_parse_porcelain_v2_both_staged_and_unstaged() {
    let output = "1 MM N... 100644 100644 100644 abc123 def456 src/lib.rs\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 1);
    assert_eq!(unstaged.len(), 1);
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .path,
        "src/lib.rs"
    );
    assert_eq!(
        unstaged
            .first()
            .unwrap_or_else(|| panic!("expected unstaged item"))
            .path,
        "src/lib.rs"
    );
}

#[test]
fn test_parse_porcelain_v2_added() {
    let output = "1 A. N... 000000 100644 100644 0000000 abc123 new_file.rs\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 1);
    assert_eq!(unstaged.len(), 0);
    assert!(matches!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .change_type,
        ChangeType::Create
    ));
}

#[test]
fn test_parse_porcelain_v2_deleted() {
    let output = "1 D. N... 100644 000000 100644 abc123 0000000 removed.rs\n";
    let (staged, _unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 1);
    assert!(matches!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .change_type,
        ChangeType::Delete
    ));
}

#[test]
fn test_parse_porcelain_v2_untracked() {
    let output = "? untracked.txt\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 0);
    assert_eq!(unstaged.len(), 1);
    assert_eq!(
        unstaged
            .first()
            .unwrap_or_else(|| panic!("expected unstaged item"))
            .path,
        "untracked.txt"
    );
    assert!(matches!(
        unstaged
            .first()
            .unwrap_or_else(|| panic!("expected unstaged item"))
            .change_type,
        ChangeType::Untracked
    ));
}

#[test]
fn test_parse_porcelain_v2_untracked_excluded() {
    let output = "? untracked.txt\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        false, // include_untracked = false
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 0);
    assert_eq!(unstaged.len(), 0);
}

#[test]
fn test_parse_porcelain_v2_rename() {
    let output = "2 R. N... 100644 100644 100644 abc123 def456 R100 new_name.rs\told_name.rs\n";
    let (staged, _unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    assert_eq!(staged.len(), 1);
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .path,
        "new_name.rs"
    );
    assert_eq!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .old_path
            .as_deref(),
        Some("old_name.rs")
    );
    assert!(matches!(
        staged
            .first()
            .unwrap_or_else(|| panic!("expected staged item"))
            .change_type,
        ChangeType::Rename
    ));
}

/// `status()` reads a split index, whether through gix or through the CLI fallback.
/// Skipped under Bazel sandbox tests where the `git` CLI is unavailable (set `BAZEL_TEST=1` to skip; cargo runs the test normally).
#[tokio::test]
#[cfg_attr(
    not(unix),
    ignore = "test invokes git CLI which is not always available"
)]
async fn test_status_with_split_index() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init_with_commit(tmp.path());

    // Enable split index via CLI; this writes the `link` extension
    git_cli(tmp.path(), &["update-index", "--split-index"])
        .await
        .expect("failed to enable split index");

    std::fs::write(tmp.path().join("test.txt"), "hello").unwrap();

    // status() succeeds with a split index
    let result = status(tmp.path(), true, true, false, false).await;
    assert!(result.is_ok(), "status() failed: {:?}", result.err());

    let data = result.unwrap();
    assert!(data.root.is_some());
    assert!(data.commit.is_some());
    assert!(
        data.unstaged.iter().any(|f| f.path == "test.txt"),
        "expected test.txt in unstaged, got: {:?}",
        data.unstaged
    );
}

#[tokio::test]
async fn test_status_via_cli_on_real_repo() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Create an initial commit so HEAD exists.
    crate::test_git::init_with_commit(tmp.path());

    // Create an unstaged file.
    std::fs::write(tmp.path().join("hello.txt"), "hello").unwrap();

    let result = status_via_cli(tmp.path(), true, true, false).await;
    assert!(result.is_ok(), "status_via_cli failed: {:?}", result.err());

    let data = result.unwrap();
    assert!(data.root.is_some());
    assert!(data.commit.is_some());

    let untracked: Vec<_> = data
        .unstaged
        .iter()
        .filter(|f| f.path == "hello.txt")
        .collect();
    assert_eq!(untracked.len(), 1);
    assert!(matches!(
        untracked
            .first()
            .unwrap_or_else(|| panic!("expected untracked item"))
            .change_type,
        ChangeType::Untracked
    ));
}

#[test]
fn test_parse_porcelain_v2_unmerged() {
    let output = "u UU N... 100644 100644 100644 100644 abc123 def456 789abc conflicted.rs\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    let total = staged.len() + unstaged.len();
    assert!(
        total > 0,
        "unmerged entry (prefix 'u') was silently dropped"
    );
}

#[test]
fn test_parse_porcelain_v2_truncated_line_skipped() {
    let output = "1 M. N... 100644 100644 100644 abc123\n";
    let (staged, unstaged) = parse_porcelain_v2(
        output,
        true,
        false,
        Path::new("/repo"),
        &HashMap::new(),
        &HashMap::new(),
    );
    for change in staged.iter().chain(unstaged.iter()) {
        assert!(
            !change.path.contains("abc123") && !change.path.contains("100644"),
            "truncated line produced GitFileChange with hash/mode as path: {:?}",
            change.path,
        );
    }
}

#[tokio::test]
async fn test_status_double_failure_preserves_original_error() {
    if std::env::var("BAZEL_TEST").is_ok() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init_with_commit(tmp.path());

    git_cli(tmp.path(), &["update-index", "--split-index"])
        .await
        .expect("failed to enable split index");

    let git_dir = tmp.path().join(".git");
    for entry in std::fs::read_dir(&git_dir).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")
        {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }

    let result = status(tmp.path(), true, true, false, false).await;
    assert!(result.is_err(), "expected both gix and CLI to fail");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("link") || err_msg.contains("gix") || err_msg.contains("extension"),
        "double-failure error should mention original gix cause, got: {err_msg}"
    );
}

// ── normalize_repo_url tests ─────────────────────────────────────

#[test]
fn normalize_ssh_scp_url() {
    assert_eq!(
        normalize_repo_url("git@github.com:xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_https_url() {
    assert_eq!(
        normalize_repo_url("https://github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_ssh_and_https_produce_same_result() {
    let ssh = normalize_repo_url("git@github.com:xai-org/example.git");
    let https = normalize_repo_url("https://github.com/xai-org/example.git");
    assert_eq!(ssh, https);
}

#[test]
fn normalize_https_without_git_suffix() {
    assert_eq!(
        normalize_repo_url("https://github.com/xai-org/example"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_https_with_credentials() {
    assert_eq!(
        normalize_repo_url("https://x-access-token:secret@github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_ssh_scheme_url() {
    assert_eq!(
        normalize_repo_url("ssh://git@github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_ssh_scheme_with_port() {
    assert_eq!(
        normalize_repo_url("ssh://git@github.com:22/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_git_scheme_url() {
    assert_eq!(
        normalize_repo_url("git://github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_http_url() {
    assert_eq!(
        normalize_repo_url("http://github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_strips_trailing_slash() {
    assert_eq!(
        normalize_repo_url("https://github.com/xai-org/example/"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_strips_dot_git_with_trailing_slash() {
    assert_eq!(
        normalize_repo_url("https://github.com/xai-org/example.git/"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_lowercases_host() {
    assert_eq!(
        normalize_repo_url("git@GitHub.COM:xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_file_url_returns_none() {
    assert_eq!(normalize_repo_url("file:///tmp/repo.git"), None);
}

#[test]
fn normalize_empty_returns_none() {
    assert_eq!(normalize_repo_url(""), None);
}

#[test]
fn normalize_whitespace_only_returns_none() {
    assert_eq!(normalize_repo_url("   "), None);
}

#[test]
fn normalize_git_plus_ssh_scheme() {
    assert_eq!(
        normalize_repo_url("git+ssh://git@github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_git_plus_https_scheme() {
    assert_eq!(
        normalize_repo_url("git+https://github.com/xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_scp_no_user() {
    assert_eq!(
        normalize_repo_url("github.com:xai-org/example.git"),
        Some("github.com/xai-org/example".into()),
    );
}

#[test]
fn normalize_https_username_password() {
    assert_eq!(
        normalize_repo_url("https://alice:pass@gitlab.com/org/project.git"),
        Some("gitlab.com/org/project".into()),
    );
}

#[test]
fn normalize_deep_path() {
    assert_eq!(
        normalize_repo_url("https://gitlab.com/group/subgroup/project.git"),
        Some("gitlab.com/group/subgroup/project".into()),
    );
}

#[test]
fn normalize_scp_with_deep_path() {
    assert_eq!(
        normalize_repo_url("git@gitlab.com:group/subgroup/project.git"),
        Some("gitlab.com/group/subgroup/project".into()),
    );
}

#[test]
fn normalize_scp_empty_host_returns_none() {
    assert_eq!(normalize_repo_url("git@:path"), None);
}

#[test]
fn normalize_scp_empty_path_returns_none() {
    assert_eq!(normalize_repo_url("git@host:"), None);
}

#[test]
fn normalize_scp_leading_slash_in_path() {
    assert_eq!(
        normalize_repo_url("git@host:/path.git"),
        Some("host/path".into()),
    );
}

#[test]
fn resolve_normalized_remote_urls_deduplicates_across_transports() {
    let tmp = tempfile::tempdir().unwrap();
    crate::test_git::init(tmp.path());
    crate::test_git::git(
        tmp.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:xai-org/example.git",
        ],
    );
    crate::test_git::git(
        tmp.path(),
        &[
            "remote",
            "add",
            "https-mirror",
            "https://github.com/xai-org/example.git",
        ],
    );

    let urls = resolve_normalized_remote_urls(tmp.path());
    assert_eq!(urls, vec!["github.com/xai-org/example"]);
}

// A well-formed OID that no fresh repo has an object for.
const MISSING_OID: &str = "0123456789abcdef0123456789abcdef01234567";

/// Point HEAD's branch at an `oid` with no backing object (git refuses that through its ref commands, so write the ref file directly).
/// Then assert that peeling fails, the precondition that makes the refs-only tests real.
fn point_head_at_missing_object(dir: &Path, oid: &str) {
    let head_ref = crate::test_git::git(dir, &["symbolic-ref", "HEAD"]);
    std::fs::write(dir.join(".git").join(&head_ref), format!("{oid}\n")).expect("write ref");

    let fresh = crate::git_repo::discover(dir).expect("reopen repo");
    assert!(
        fresh.head_commit().is_err(),
        "peel must fail, else the test misses the refs-only path",
    );
}

#[test]
fn metadata_resolves_head_when_object_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    crate::test_git::init_with_commit(tmp.path());
    let branch = crate::test_git::git(tmp.path(), &["branch", "--show-current"]);

    point_head_at_missing_object(tmp.path(), MISSING_OID);

    let metadata = resolve_persisted_session_git_metadata_sync(tmp.path());
    assert_eq!(metadata.head_commit.as_deref(), Some(MISSING_OID));
    // Commit and branch come from the same ref resolution.
    assert_eq!(metadata.head_branch.as_deref(), Some(branch.as_str()));
}

/// `get_current_commit` returns `None` for an unborn HEAD, and the OID whether the commit object is live or missing.
#[tokio::test]
async fn get_current_commit_reads_head_from_refs() {
    let tmp = tempfile::tempdir().expect("tempdir");

    crate::test_git::init(tmp.path());
    assert!(get_current_commit(tmp.path()).await.is_none());

    // Same dir; init is idempotent, now with a commit.
    let commit_oid = crate::test_git::init_with_commit(tmp.path());
    assert_eq!(
        get_current_commit(tmp.path()).await.as_deref(),
        Some(commit_oid.as_str()),
    );

    point_head_at_missing_object(tmp.path(), MISSING_OID);
    assert_eq!(
        get_current_commit(tmp.path()).await.as_deref(),
        Some(MISSING_OID),
    );
}

/// End-to-end: `status()` reports the HEAD hash even when the commit object is gone.
/// Status tolerates a missing HEAD tree (it diffs against an empty tree), so the refs-only OID read supplies the hash.
#[tokio::test]
async fn status_reports_head_oid_when_object_missing() {
    xai_test_utils::require_git!();
    let tmp = tempfile::tempdir().expect("tempdir");
    crate::test_git::init_with_commit(tmp.path());
    point_head_at_missing_object(tmp.path(), MISSING_OID);

    let data = status(
        tmp.path(),
        /*include_untracked*/ false,
        /*include_stats*/ false,
        /*ignore_submodules*/ true,
        /*include_patches*/ false,
    )
    .await
    .expect("status");
    assert_eq!(data.commit.as_deref(), Some(MISSING_OID));
}

/// A HEAD whose object is missing must not report "already checked out".
/// The fast path falls through to the repair fetch, which fails here because the repo has no origin.
#[tokio::test]
async fn checkout_commit_with_fetch_repairs_missing_head_object() {
    xai_test_utils::require_git!();
    let tmp = tempfile::tempdir().expect("tempdir");
    crate::test_git::init_with_commit(tmp.path());
    point_head_at_missing_object(tmp.path(), MISSING_OID);

    let resp = checkout_commit_with_fetch(tmp.path(), MISSING_OID, /*stash_if_dirty*/ false).await;
    assert!(!resp.checked_out);
}

#[test]
fn git_cli_pin_scan_cwd_follows_c() {
    let base = std::path::Path::new("/repo-a");
    let other = std::path::Path::new("/repo-b");
    assert_eq!(
        git_cli_pin_scan_cwd(base, &["-C", "/repo-b", "status"]).unwrap(),
        other
    );
    assert!(git_cli_pin_scan_cwd(base, &["--git-dir", "/x", "status"]).is_err());
}

#[test]
fn git_cli_filter_pin_plan_cases() {
    assert_eq!(
        super::git_cli_filter_pin_plan(&["status"]),
        GitCliFilterPinPlan::NeedPins
    );
    assert_eq!(
        super::git_cli_filter_pin_plan(&["worktree", "remove", "x"]),
        GitCliFilterPinPlan::NoPins
    );
    assert_eq!(
        super::git_cli_filter_pin_plan(&["--weird", "status"]),
        GitCliFilterPinPlan::Refuse
    );
}
