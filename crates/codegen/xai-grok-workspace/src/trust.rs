use std::path::Path;
use std::sync::Once;

pub(crate) use xai_grok_permission_rules::trust::{Recorded, TrustPersistError, trust_store_home};
pub use xai_grok_permission_rules::trust::{
    TRUST_FILE_NAME, TrustStore, is_home_dir, is_unsafe_trust_root, workspace_key,
};

/// One-time migration of legacy project-hook trust grants into the unified folder-trust store. Idempotent and guarded to run at most once per process.
/// The legacy file is then renamed to `*.migrated` so it is read only once.
pub fn migrate_legacy_hook_trust() {
    // Local/dev builds do NO trust-store I/O: skip the load and the legacy-file rename
    if crate::folder_trust::folder_trust_inert() {
        return;
    }
    static MIGRATED: Once = Once::new();
    MIGRATED.call_once(|| {
        // Same fresh home as the store; do not follow user_grok_home()'s OnceLock.
        let Some(home) = xai_dirs::resolve_grok_home() else {
            return;
        };
        let legacy_file = home.join(xai_grok_config::TRUSTED_HOOK_PROJECTS_FILENAME);
        let mut store = TrustStore::load();
        let migrated = migrate_legacy_hook_trust_in(&legacy_file, &mut store);
        if migrated > 0 {
            tracing::info!(
                migrated,
                "migrated legacy hook-trust grants into folder-trust"
            );
        }
    });
}

/// [`migrate_legacy_hook_trust`] with explicit paths, so the migration is testable without the process-global grok-home cache.
/// Returns the number of grants seeded into `store`.
fn migrate_legacy_hook_trust_in(legacy_file: &Path, store: &mut TrustStore) -> usize {
    // A read error must NOT be mistaken for "no grants": bail without renaming
    // A transient/permission failure then can't permanently consume the legacy file
    let projects = match xai_grok_hooks::trust::list_trusted_projects_with_file(legacy_file) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                path = %legacy_file.display(),
                error = %e,
                "failed to read legacy hook-trust file; leaving it in place for a future run"
            );
            return 0;
        }
    };
    // A failed load is not "no decision": do not seed from an empty stand-in or consume the legacy file.
    if !store.has_store_path() || !store.disk_readable() {
        tracing::warn!(
            path = %legacy_file.display(),
            "leaving legacy hook-trust file in place; folder-trust store has no readable document"
        );
        return 0;
    }
    let mut migrated = 0;
    let mut had_seed_error = false;
    for project in &projects {
        // Never override an existing decision: a folder the user has since trusted or untrusted keeps that decision
        // A re-run after a rename failure then can't silently re-trust a folder the user untrusted in between
        if store.has_decision(project) {
            continue;
        }
        match store.record_decision_strict(project, true) {
            Ok(Recorded::Durable) => migrated += 1,
            Ok(Recorded::Skipped) => {
                // Silent Ok (unsafe root / no path) is not a durable insert; do not consume the legacy file.
                had_seed_error = true;
            }
            Err(e) => {
                tracing::warn!(
                    path = %project.display(),
                    error = %e.as_io(),
                    "failed to migrate a legacy hook-trust grant"
                );
                had_seed_error = true;
            }
        }
    }
    // Rename so the legacy file is consumed exactly once, reached only after a SUCCESSFUL read AND with every grant seeded
    // A seeding write error leaves the file in place for a future run (mirrors the read-error bail)
    // Idempotent: skipped when already migrated/absent
    if had_seed_error {
        tracing::warn!(
            path = %legacy_file.display(),
            "leaving legacy hook-trust file in place after a seeding error; a future run will retry"
        );
    } else if legacy_file.exists() {
        let migrated_file = legacy_file.with_extension("migrated");
        if let Err(e) = std::fs::rename(legacy_file, &migrated_file) {
            tracing::warn!(
                path = %legacy_file.display(),
                error = %e,
                "failed to rename legacy hook-trust file after migration"
            );
        }
    }
    migrated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xai_grok_permission_rules::trust::canonicalize_or_owned;

    #[test]
    fn migrate_legacy_hook_trust_seeds_store_and_renames_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        // A real project dir so set_trusted's canonicalize succeeds.
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        // Legacy file with one canonical project path.
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        let mut store = TrustStore::load_from(store_path.clone());
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 1);
        assert!(store.is_trusted(&project_key), "migrated grant is trusted");

        // Legacy file renamed so it is read only once.
        assert!(!legacy.exists(), "legacy file consumed");
        assert!(
            legacy.with_extension("migrated").exists(),
            "renamed to .migrated"
        );

        // The grant persisted to disk.
        let reloaded = TrustStore::load_from(store_path);
        assert!(reloaded.is_trusted(&project_key));
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_override_existing_decision() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        // The user has ALREADY untrusted this folder in the unified store.
        let mut store = TrustStore::load_from(store_path);
        store.set_untrusted(&project_key).unwrap();

        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "an already-decided folder is not re-seeded");
        assert!(
            !store.is_trusted(&project_key),
            "the user's untrust decision is preserved, not overridden by migration"
        );
        // The legacy file is still consumed (renamed) so it is read only once.
        assert!(!legacy.exists());
        assert!(legacy.with_extension("migrated").exists());
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_rename_without_backing_store() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(
            &legacy,
            format!(
                "{}
",
                canonicalize_or_owned(&project).display()
            ),
        )
        .unwrap();
        let mut store = TrustStore::empty();
        assert_eq!(migrate_legacy_hook_trust_in(&legacy, &mut store), 0);
        assert!(
            legacy.exists(),
            "no-home store must not consume the legacy file"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_does_not_rename_on_unreadable_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::write(&store_path, b"{[[not json").unwrap();
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(
            &legacy,
            format!(
                "{}
",
                canonicalize_or_owned(&project).display()
            ),
        )
        .unwrap();
        let mut store = TrustStore::load_from(store_path);
        assert!(!store.disk_readable());
        assert_eq!(migrate_legacy_hook_trust_in(&legacy, &mut store), 0);
        assert!(
            legacy.exists(),
            "unread store must not consume the legacy file"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_is_noop_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let legacy = tmp.path().join("trusted-hook-projects"); // never created

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0);
        assert!(store.is_empty(), "nothing recorded");
        assert!(
            !legacy.with_extension("migrated").exists(),
            "no rename without source"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_leaves_unreadable_file_in_place() {
        // A legacy file that EXISTS but can't be read must not be consumed
        // A transient read error would otherwise rename it and permanently drop every grant
        // Use a directory at the legacy path: it `exists()` but `read_to_string` errors (non-NotFound), portably simulating the failure
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::create_dir_all(&legacy).unwrap();

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "an unreadable legacy file seeds nothing");
        assert!(store.is_empty(), "nothing recorded on a read failure");
        assert!(legacy.exists(), "unreadable legacy file is left in place");
        assert!(
            !legacy.with_extension("migrated").exists(),
            "unreadable legacy file must not be consumed/renamed"
        );
    }

    #[test]
    fn migrate_legacy_hook_trust_leaves_file_in_place_on_seed_write_error() {
        // A seeding WRITE failure (e.g. a full disk) must not consume the legacy file either: leave it un-renamed so a future run retries the grants.
        // Force set_trusted to error by making the store path a DIRECTORY so its atomic persist rename fails
        // (Same trick as `persist_failure_leaves_memory_unchanged`, robust even when run as root.)
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::create_dir_all(&store_path).unwrap(); // store path is a dir, not a file
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let project_key = canonicalize_or_owned(&project);

        let legacy = tmp.path().join("trusted-hook-projects");
        std::fs::write(&legacy, format!("{}\n", project_key.display())).unwrap();

        let mut store = TrustStore::load_from(store_path);
        let migrated = migrate_legacy_hook_trust_in(&legacy, &mut store);
        assert_eq!(migrated, 0, "a seeding write error seeds nothing");
        assert!(
            legacy.exists(),
            "legacy file is left in place on a seeding write error"
        );
        assert!(
            !legacy.with_extension("migrated").exists(),
            "a seeding write error must not consume/rename the legacy file"
        );
    }

<<<<<<< fork
||||||| base
    #[test]
    fn root_key_is_not_trusted() {
        // Fail closed: a `[folders."/"]` record must not trust every absolute path
        // The root is a prefix of all of them via the cascade, so it is ignored on read even if it reaches the file by hand-edit / migration
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::write(&store_path, "[folders.\"/\"]\ntrusted = true\n").unwrap();

        let store = TrustStore::load_from(store_path);
        assert!(!store.is_empty(), "root-key record should still load");
        assert!(
            !store.is_trusted(Path::new("/any/abs/path")),
            "filesystem root must never be honored as a trust key"
        );
    }

    #[test]
    fn tied_conflicting_aliases_fail_closed() {
        // Two equal-depth, non-canonical aliases of the SAME folder carry CONFLICTING decisions `Path::components()` normalizes the trailing slash so `/a/b` and `/a/b/` tie on depth, yet they
        // load as distinct map keys The tie branch ANDs the tied records, so any untrusted tied alias forces a fail-closed `false` REGARDLESS of map order Asserting BOTH orderings pins this:
        // a last-wins revert (return the LAST equal-depth record) returns `true` for ordering (b) below A single pinned ordering would pass under both the AND-loop and the buggy last-wins form
        let fails_closed = |trusted_ab: bool, trusted_ab_slash: bool| {
            let tmp = tempfile::tempdir().unwrap();
            let store_path = tmp.path().join(TRUST_FILE_NAME);
            std::fs::write(
                &store_path,
                format!(
                    "[folders.'/a/b']\ntrusted = {trusted_ab}\n\
                     [folders.'/a/b/']\ntrusted = {trusted_ab_slash}\n"
                ),
            )
            .unwrap();
            let store = TrustStore::load_from(store_path);
            assert_eq!(store.len(), 2, "both alias records should load distinctly");
            // `/a/b/c` does not exist, so `canonicalize_or_owned` is a no-op; both aliases prefix it and tie on depth.
            !store.is_trusted(Path::new("/a/b/c"))
        };

        // (a) untrusted alias sorts LAST (`/a/b/`): caught by a revert to the original `any(trusted)` form, but NOT by a last-wins revert
        assert!(
            fails_closed(true, false),
            "tie with `/a/b` trusted + `/a/b/` untrusted must fail closed"
        );
        // (b) untrusted alias sorts FIRST (`/a/b`): a last-wins revert would return the last record (`/a/b/`, trusted)
        //     THIS ordering is what catches a last-wins regression; the AND-loop still yields false
        assert!(
            fails_closed(false, true),
            "tie with `/a/b` untrusted + `/a/b/` trusted must STILL fail closed"
        );
    }

    #[test]
    fn home_key_on_disk_is_not_honored() {
        // Serialize with the test in this file that mutates $HOME: its temp $HOME window could otherwise flip is_home_dir mid-test
        // This test mutates no env itself
        let _lock = crate::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // A hand-edited / migrated `[folders."<home>"]` record must not trust repos under $HOME; the read side ignores it, matching set_trusted
        let Some(home) = xai_dirs::home_dir() else {
            return; // no home dir in this environment; nothing to assert
        };
        let canonical_home = canonicalize_or_owned(&home);
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        // TOML literal-string key avoids escaping issues on any platform.
        let body = format!(
            "[folders.'{}']\ntrusted = true\n",
            canonical_home.to_string_lossy()
        );
        std::fs::write(&store_path, body).unwrap();

        let store = TrustStore::load_from(store_path);
        let sub = canonical_home.join("some").join("sub");
        assert!(
            !store.is_trusted(&sub),
            "a home-dir key on disk must not be honored"
        );
    }

    #[cfg(unix)]
    #[test]
    fn set_trusted_canonicalizes_key() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let real = tmp.path().join("real-repo");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link-repo");
        symlink(&real, &link).unwrap();

        // Trust via the symlink alias.
        let mut store = TrustStore::load_from(store_path);
        store.set_trusted(&link).unwrap();

        let canonical_real = canonicalize_or_owned(&real);
        assert!(
            store.is_trusted(&canonical_real),
            "set_trusted must store the canonical path so canonical lookups match"
        );
    }

    #[test]
    fn set_untrusted_records_explicit_deny() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let key = canonicalize_or_owned(&repo);

        let mut store = TrustStore::load_from(store_path.clone());
        store.set_untrusted(&key).unwrap();
        assert!(!store.is_trusted(&key), "an explicit deny is not trusted");
        assert!(!store.is_empty(), "the deny decision is recorded");

        // Reload from disk: the deny record persisted.
        let reloaded = TrustStore::load_from(store_path);
        assert!(!reloaded.is_trusted(&key));
        assert!(!reloaded.is_empty(), "deny record survives reload");
    }

    #[test]
    fn trust_decision_flips() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let key = canonicalize_or_owned(&repo);

        let mut store = TrustStore::load_from(store_path.clone());
        store.set_trusted(&key).unwrap();
        assert!(store.is_trusted(&key));
        store.set_untrusted(&key).unwrap();
        assert!(!store.is_trusted(&key), "untrust flips the stored bool");
        store.set_trusted(&key).unwrap();
        assert!(store.is_trusted(&key), "re-trust flips it back");
        // The insert overwrites: one record per folder, no duplicates
        assert_eq!(store.len(), 1);

        let reloaded = TrustStore::load_from(store_path);
        assert!(reloaded.is_trusted(&key));
        assert_eq!(reloaded.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn is_trusted_canonicalizes_query() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let real = tmp.path().join("real-repo");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link-repo");
        symlink(&real, &link).unwrap();

        let mut store = TrustStore::load_from(store_path);
        store.set_trusted(&canonicalize_or_owned(&real)).unwrap();

        // A query via the symlink alias resolves to the trusted real dir.
        assert!(
            store.is_trusted(&link),
            "is_trusted must canonicalize the query so a symlink alias matches"
        );
    }

    #[test]
    fn concurrent_writers_do_not_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let repo_a = tmp.path().join("repo-a");
        let repo_b = tmp.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        let key_a = canonicalize_or_owned(&repo_a);
        let key_b = canonicalize_or_owned(&repo_b);

        // Two instances loaded while the file is empty: both start with an empty in-memory doc, mimicking two processes that raced the initial load
        let mut s1 = TrustStore::load_from(store_path.clone());
        let mut s2 = TrustStore::load_from(store_path.clone());
        s1.set_trusted(&key_a).unwrap();
        s2.set_trusted(&key_b).unwrap();

        // The locked re-read-merge means s2's write did not clobber s1's.
        let reloaded = TrustStore::load_from(store_path);
        assert!(
            reloaded.is_trusted(&key_a),
            "A must survive a concurrent write"
        );
        assert!(
            reloaded.is_trusted(&key_b),
            "B must survive a concurrent write"
        );
    }

    #[cfg(unix)]
    #[test]
    fn persist_failure_leaves_memory_unchanged() {
        // Make the destination path itself a DIRECTORY so the final atomic rename in persist fails (renaming a file over a directory)
        // This is robust even when tests run as root (a chmod 0o500 dir would be bypassed by root)
        // It exercises the invariant: on a write error the in-memory doc is left unchanged
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::create_dir_all(&store_path).unwrap(); // store path is a dir, not a file
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let key = canonicalize_or_owned(&repo);

        let mut store = TrustStore::load_from(store_path);
        let result = store.set_trusted(&key);
        assert!(
            result.is_err(),
            "persist over a directory destination must fail"
        );
        assert!(
            !store.is_trusted(&key),
            "memory must be unchanged on persist failure"
        );
    }

    #[test]
    fn corrupt_store_is_not_rewritten_by_set_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let good = br#"
[folders."/tmp/keep"]
trusted = true
"#;
        std::fs::write(&store_path, good).unwrap();
        let corrupt = b"this is not toml [[[";
        std::fs::write(&store_path, corrupt).unwrap();
        let before = std::fs::read(&store_path).unwrap();

        let mut store = TrustStore::load_from(store_path.clone());
        assert!(!store.disk_readable());
        assert!(!store.is_trusted(Path::new("/tmp/keep")));
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let err = store.set_trusted(&repo);
        assert!(err.is_err(), "grant over an unreadable store must fail");
        let after = std::fs::read(&store_path).unwrap();
        assert_eq!(
            before, after,
            "failed parse must not shrink or replace the file"
        );
        assert!(!TrustStore::load_from(store_path).is_trusted(&repo));
    }

    #[test]
    fn missing_file_grant_creates_only_the_new_key() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        assert!(!store_path.exists());
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let key = canonicalize_or_owned(&repo);

        let mut store = TrustStore::load_from(store_path.clone());
        assert!(store.disk_readable());
        store.set_trusted(&key).unwrap();
        let reloaded = TrustStore::load_from(store_path);
        assert!(reloaded.is_trusted(&key));
        assert_eq!(reloaded.len(), 1);
    }

    #[test]
    fn whitespace_file_is_empty_document_and_mergeable() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        std::fs::write(&store_path, "  \n\t  ").unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let key = canonicalize_or_owned(&repo);

        let mut store = TrustStore::load_from(store_path.clone());
        assert!(store.disk_readable());
        assert!(store.is_empty());
        store.set_trusted(&key).unwrap();
        assert!(TrustStore::load_from(store_path).is_trusted(&key));
    }

    #[test]
    fn not_a_directory_parent_is_missing_not_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let parent_file = tmp.path().join("not-a-dir");
        std::fs::write(&parent_file, b"x").unwrap();
        let store_path = parent_file.join(TRUST_FILE_NAME);
        let store = TrustStore::load_from(store_path);
        assert!(
            store.disk_readable(),
            "NotADirectory (ENOTDIR) is Missing, not Unreadable"
        );
        assert!(store.is_empty());

        let dir_dest = tmp.path().join("store-is-a-dir");
        std::fs::create_dir_all(&dir_dest).unwrap();
        let eisdir = TrustStore::load_from(dir_dest);
        assert!(
            !eisdir.disk_readable(),
            "IsADirectory (EISDIR) stays Unreadable"
        );
    }

    #[test]
    fn load_on_corrupt_file_does_not_truncate() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join(TRUST_FILE_NAME);
        let bytes = b"not = [valid";
        std::fs::write(&store_path, bytes).unwrap();
        let store = TrustStore::load_from(store_path.clone());
        assert!(!store.disk_readable());
        assert_eq!(std::fs::read(&store_path).unwrap(), bytes);
    }

    #[test]
    fn workspace_key_collapses_linked_worktrees_onto_main_checkout() {
        // Every linked `grok -w` worktree of a repo must share ONE trust key: its main checkout's root
        // Build a real repo and two linked worktrees and assert each collapses onto the main checkout (trusted once, not re-prompted per worktree)
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        let repo = git2::Repository::init(&main).unwrap();

        // Worktree creation requires a valid HEAD, so make an initial commit.
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree = {
            let mut idx = repo.index().unwrap();
            let oid = idx.write_tree().unwrap();
            repo.find_tree(oid).unwrap()
        };
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        // Linked worktrees OUTSIDE the main dir (git2 creates the paths).
        let wt1 = dir.path().join("wt1");
        let wt2 = dir.path().join("wt2");
        repo.worktree("wt1", &wt1, None).unwrap();
        repo.worktree("wt2", &wt2, None).unwrap();

        let main_key = workspace_key(&main);
        // Parity: the main checkout keys off its own workdir.
        assert_eq!(main_key, canonicalize_or_owned(&main));
        assert_eq!(
            workspace_key(&wt1),
            main_key,
            "worktree must collapse onto main checkout"
        );
        assert_eq!(
            workspace_key(&wt2),
            main_key,
            "second worktree must share the same key"
        );
    }

    #[test]
    fn workspace_key_bare_repo_worktree_does_not_widen_to_parent() {
        // A bare repo's `commondir()` is the bare dir itself, so a naive `commondir().parent()` would key off the dir CONTAINING the repo
        // That would trust every sibling via the subdirectory cascade
        // The key must instead fall back to the worktree's OWN dir (narrow, never widened)
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("repo.git");
        let repo = git2::Repository::init_bare(&bare).unwrap();

        // Worktree creation needs a valid HEAD; build an empty commit (bare repo has no index, so use a treebuilder for the empty tree)
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let tree_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        let wt = dir.path().join("wt");
        repo.worktree("wt", &wt, None).unwrap();

        let key = workspace_key(&wt);
        assert_ne!(
            key,
            canonicalize_or_owned(dir.path()),
            "bare-repo worktree key must not widen to the parent dir"
        );
        assert_eq!(
            key,
            canonicalize_or_owned(&wt),
            "bare-repo worktree falls back to its own dir (narrow, safe)"
        );
    }

    #[test]
    fn workspace_key_separate_gitdir_worktree_does_not_widen() {
        // `git init --separate-git-dir` leaves `core.worktree` unset The common gitdir's INFERRED workdir is then
        // the PARENT of the relocated gitdir, not the checkout The layout guard (`<workdir>/.git` must equal the
        // common gitdir) rejects that The key falls back to the worktree's own dir, never widening to the gitdir's parent
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let gitdir = dir.path().join("gitstore");
        std::fs::create_dir_all(&checkout).unwrap();
        let run = |args: &[&str], cwd: &std::path::Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        // If git isn't usable for this layout, skip rather than report a false failure
        if !run(
            &[
                "init",
                "--separate-git-dir",
                gitdir.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
            dir.path(),
        ) || !run(&["commit", "--allow-empty", "-m", "init"], &checkout)
        {
            return;
        }
        let wt = dir.path().join("wt");
        if !run(&["worktree", "add", wt.to_str().unwrap()], &checkout) {
            return;
        }

        // Only assert once the worktree is a linked worktree whose common gitdir is the relocated separate gitdir (the layout this test targets)
        let Ok(repo) = git2::Repository::discover(&wt) else {
            return;
        };
        if !repo.is_worktree() {
            return;
        }

        let key = workspace_key(&wt);
        // The invariant: the key is NOT a broad ancestor of the checkout.
        assert_ne!(
            key,
            canonicalize_or_owned(dir.path()),
            "separate-gitdir worktree key must not widen to the gitdir's parent"
        );
        assert_eq!(
            key,
            canonicalize_or_owned(&wt),
            "separate-gitdir worktree falls back to its own dir (narrow, safe)"
        );
    }

=======
>>>>>>> upstream
    // ── workspace_key registry collapse (grok-managed worktrees) ─────────

    // The crate-shared env lock and env guards travel as ONE value
    // Struct field order (see lib.rs) restores the env before the lock releases, no matter how the caller binds the fixture's return
    use crate::LockedTestEnv;

    /// Point `GROK_HOME` at an isolated tempdir and register one grok-managed worktree at `<home>/worktrees/repo/<name>`.
    /// The worktree dir is a PLAIN directory (NOT a git linked worktree), so only the registry can collapse it.
    fn register_grok_worktree(
        temp: &tempfile::TempDir,
        name: &str,
        source_repo: &Path,
        creation_mode: &str,
    ) -> (LockedTestEnv, PathBuf) {
        use xai_fast_worktree::{WorktreeDb, WorktreeKind, WorktreeRecord, WorktreeStatus};

        // Canonicalize so macOS's `/var` (a symlink to `/private/var`) agrees between the stored record path and the canonicalized lookup query
        let root = dunce::canonicalize(temp.path()).unwrap();
        let home = root.join("grok-home");
        let wt = home.join("worktrees").join("repo").join(name);
        std::fs::create_dir_all(&wt).unwrap();

        // Acquire the lock, then set the env under it (LockedTestEnv restores the env before releasing the lock on drop)
        let env = LockedTestEnv::lock().set("GROK_HOME", &home);

        let db = WorktreeDb::open(&home).unwrap();
        let record = WorktreeRecord {
            id: name.to_string(),
            path: wt.clone(),
            source_repo: source_repo.to_path_buf(),
            repo_name: "repo".to_string(),
            kind: WorktreeKind::Session,
            creation_mode: creation_mode.to_string(),
            git_ref: None,
            head_commit: None,
            session_id: None,
            creator_pid: None,
            created_at: 100,
            last_accessed_at: None,
            status: WorktreeStatus::Alive,
            metadata: None,
        };
        db.register(&record).unwrap();
        (env, wt)
    }

    #[test]
    fn workspace_key_collapses_standalone_grok_worktree_onto_source_repo() {
        // A standalone worktree is a full clone with its OWN `.git`, so git topology can't link it to its source The registry (worktrees.db) must collapse
        // it onto the recorded source repo so trust is shared The worktree dir is a plain dir (no git), proving the REGISTRY path (not git topology) does
        // the collapse `source_repo` is a real git repo (as in production), so the git-root normalization is deterministic regardless of where `$TMPDIR` lives
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let source_repo = root.join("source-repo");
        std::fs::create_dir_all(&source_repo).unwrap();
        crate::test_git::init(&source_repo);

        let (_env, wt) = register_grok_worktree(&temp, "wt", &source_repo, "standalone");

        let expected = canonicalize_or_owned(&source_repo);
        assert_eq!(
            workspace_key(&wt),
            expected,
            "a standalone grok worktree must collapse onto its recorded source repo"
        );
        // A cwd nested below the worktree root collapses onto the same key (the registry walk ascends to the registered worktree)
        let nested = wt.join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            workspace_key(&nested),
            expected,
            "a nested cwd in the worktree collapses onto the same source repo"
        );
    }

    #[test]
    fn workspace_key_collapses_worktree_onto_source_repo_git_root() {
        // The registry records `source_repo` as the launch cwd, which may be a SUBDIR of the repo
        // workspace_key must key on the repo's git ROOT, not on `<repo>/sub`
        // A worktree launched from a subdir then shares ONE key with the source and linked worktrees (which key on the root)
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let repo = root.join("realrepo");
        std::fs::create_dir_all(&repo).unwrap();
        crate::test_git::init(&repo);
        let subdir = repo.join("crates").join("sub");
        std::fs::create_dir_all(&subdir).unwrap();

        let (_env, wt) = register_grok_worktree(&temp, "wt", &subdir, "standalone");

        assert_eq!(
            workspace_key(&wt),
            canonicalize_or_owned(&repo),
            "source_repo recorded as a subdir must collapse onto the repo git root"
        );
    }

    #[test]
    fn workspace_key_collapses_repo_local_grok_worktree_onto_main_checkout() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let main_repo = root.join("thing");
        std::fs::create_dir_all(&main_repo).unwrap();
        crate::test_git::init(&main_repo);

        let wt = main_repo.join(".grok").join("worktrees").join("label");
        std::fs::create_dir_all(&wt).unwrap();
        let home = root.join("grok-home");
        let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
        use xai_fast_worktree::{WorktreeDb, WorktreeKind, WorktreeRecord, WorktreeStatus};
        WorktreeDb::open(&home)
            .unwrap()
            .register(&WorktreeRecord {
                id: "repo-local".to_string(),
                path: wt.clone(),
                source_repo: main_repo.clone(),
                repo_name: "thing".to_string(),
                kind: WorktreeKind::Session,
                creation_mode: "linked".to_string(),
                git_ref: None,
                head_commit: None,
                session_id: None,
                creator_pid: None,
                created_at: 100,
                last_accessed_at: None,
                status: WorktreeStatus::Alive,
                metadata: None,
            })
            .unwrap();

        let expected = canonicalize_or_owned(&main_repo);
        assert_eq!(
            workspace_key(&wt),
            expected,
            "a worktree at <repo>/.grok/worktrees/<label> must share the main \
             checkout's trust key"
        );
        let nested = wt.join("crates").join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            workspace_key(&nested),
            expected,
            "a nested cwd in a repo-local worktree collapses onto the same root"
        );
    }

    #[test]
    fn workspace_key_does_not_collapse_an_unmanaged_dir_in_a_managed_repo() {
        // Widening the predicate to the repository-local root must not turn.
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let main_repo = root.join("thing");
        std::fs::create_dir_all(&main_repo).unwrap();
        crate::test_git::init(&main_repo);
        let other_source = root.join("other-source");
        std::fs::create_dir_all(&other_source).unwrap();
        crate::test_git::init(&other_source);

        std::fs::create_dir_all(main_repo.join(".grok").join("worktrees").join("label")).unwrap();
        let home = root.join("grok-home");
        let _env = LockedTestEnv::lock().set("GROK_HOME", &home);
        use xai_fast_worktree::{WorktreeDb, WorktreeKind, WorktreeRecord, WorktreeStatus};
        WorktreeDb::open(&home)
            .unwrap()
            .register(&WorktreeRecord {
                id: "repo-local".to_string(),
                path: main_repo.join(".grok").join("worktrees").join("label"),
                source_repo: other_source.clone(),
                repo_name: "thing".to_string(),
                kind: WorktreeKind::Session,
                creation_mode: "standalone".to_string(),
                git_ref: None,
                head_commit: None,
                session_id: None,
                creator_pid: None,
                created_at: 100,
                last_accessed_at: None,
                status: WorktreeStatus::Alive,
                metadata: None,
            })
            .unwrap();

        let unmanaged = main_repo.join(".grok").join("worktrees-other").join("dir");
        std::fs::create_dir_all(&unmanaged).unwrap();
        assert_eq!(
            workspace_key(&unmanaged),
            canonicalize_or_owned(&main_repo),
            "a directory outside <repo>/.grok/worktrees must keep its own key"
        );
    }

    #[test]
    fn workspace_key_ignores_registry_for_cwd_outside_worktrees_dir() {
        // A populated registry must NOT collapse a cwd OUTSIDE `<grok_home>/worktrees` The reader's cwd gate skips the registry there, so the key falls back to
        // git/cwd Non-vacuous: the registry IS populated with a real git source repo that WOULD be returned for a worktree cwd `outside` is its OWN git repo (under
        // grok HOME but not under its `worktrees/`), so the fallback is deterministic (no conditional skip) We assert the key is `outside`'s own root, never the source repo
        let temp = tempfile::TempDir::new().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let source_repo = root.join("source-repo");
        std::fs::create_dir_all(&source_repo).unwrap();
        crate::test_git::init(&source_repo);

        let (_env, _wt) = register_grok_worktree(&temp, "wt", &source_repo, "standalone");

        // Under grok HOME but NOT under `<home>/worktrees`, and its own git repo.
        let outside = root.join("grok-home").join("not-worktrees").join("proj");
        std::fs::create_dir_all(&outside).unwrap();
        crate::test_git::init(&outside);

        let key = workspace_key(&outside);
        assert_eq!(
            key,
            canonicalize_or_owned(&outside),
            "a cwd outside <grok_home>/worktrees keys on its own repo root"
        );
        assert_ne!(
            key,
            canonicalize_or_owned(&source_repo),
            "it must not collapse onto the populated registry's source repo"
        );
    }
}
