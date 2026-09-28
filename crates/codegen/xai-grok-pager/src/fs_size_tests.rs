use super::*;

fn counted(size: DirSize) -> u64 {
    size.measure.bytes().expect("measured on its own volume")
}

#[cfg(unix)]
#[test]
fn sizing_is_physical_not_logical() {
    let tmp = tempfile::TempDir::new().unwrap();
    const LOGICAL: u64 = 1 << 20;

    let sparse = tmp.path().join("sparse.bin");
    std::fs::File::create(&sparse)
        .unwrap()
        .set_len(LOGICAL)
        .unwrap();
    let sparse_meta = std::fs::symlink_metadata(&sparse).unwrap();
    assert_eq!(sparse_meta.len(), LOGICAL);
    assert!(
        physical_file_size(&sparse_meta) < LOGICAL,
        "a hole must cost fewer blocks than its logical length"
    );

    let linked = tmp.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    let original = linked.join("a.bin");
    std::fs::write(&original, vec![b'x'; 8192]).unwrap();
    std::fs::hard_link(&original, linked.join("b.bin")).unwrap();
    let one_link = physical_file_size(&std::fs::symlink_metadata(&original).unwrap());
    assert_eq!(
        counted(physical_dir_size(&linked, Volume::of(&linked))),
        one_link * 2,
        "each hard link to an inode counts at full size, as clones do"
    );
}

#[test]
fn physical_dir_size_sums_files_without_following_symlinks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir_all(target.join("sub")).unwrap();
    std::fs::write(target.join("a.bin"), vec![b'x'; 8192]).unwrap();
    std::fs::write(target.join("sub/b.bin"), vec![b'y'; 4096]).unwrap();

    let full = counted(physical_dir_size(&target, Volume::of(&target)));
    let expected: u64 = [target.join("a.bin"), target.join("sub/b.bin")]
        .iter()
        .map(|p| physical_file_size(&std::fs::symlink_metadata(p).unwrap()))
        .sum();
    assert_eq!(full, expected);

    #[cfg(unix)]
    {
        let linked = tmp.path().join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&target, linked.join("escape")).unwrap();
        let link_only = counted(physical_dir_size(&linked, Volume::of(&linked)));
        let link_meta = std::fs::symlink_metadata(linked.join("escape")).unwrap();
        assert_eq!(
            link_only,
            physical_file_size(&link_meta),
            "a walk must cost exactly the symlink's own inode, never the target"
        );
    }
}

// The Err arm no permission trick can reach on a root-uid CI runner.
#[test]
fn missing_root_counts_one_unreadable_dir() {
    let missing = Path::new("/nonexistent-grok-du-root");
    let size = physical_dir_size(missing, Volume::of(missing));
    assert_eq!(size.measure.bytes(), Some(0));
    assert_eq!(size.issues.unreadable_dirs, 1);
    assert_eq!(size.issues.skipped(), 1);
}

#[test]
fn a_volume_holds_only_its_own_device() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path();
    assert!(Volume(None).holds(path), "unknown anchor excludes nothing");
    assert!(
        Volume::of(path).holds(Path::new("/nonexistent-grok-du-path")),
        "unknown entry device is not a proven crossing"
    );
    assert!(Volume::of(path).holds(path), "same device");
    #[cfg(unix)]
    {
        let elsewhere = Volume::of(path).other_device_for_test();
        assert!(!elsewhere.holds(path), "a different device is a crossing");
    }
}

#[cfg(unix)]
#[test]
fn a_root_off_the_anchor_is_measured_by_nobody() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("worktrees");
    let worktree = root.join("xai/wt-a");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join(".git"), "gitdir: /repo/.git/worktrees/wt-a\n").unwrap();
    std::fs::write(worktree.join("payload.bin"), vec![b'x'; 65536]).unwrap();
    let elsewhere = Volume::of(tmp.path()).other_device_for_test();

    let home = physical_buckets(&root, Volume::of(&root));
    assert!(home.total.bytes().is_some_and(|bytes| bytes >= 65536));
    assert!(
        home.buckets[&worktree].bytes().is_some_and(|b| b >= 65536),
        "on its own volume the worktree is a counted bucket"
    );

    let foreign = physical_buckets(&root, elsewhere);
    assert_eq!(foreign.total, Measure::Elsewhere, "no total exists for it");
    assert_eq!(foreign.total.bytes(), None);
    assert_eq!(foreign.issues.other_filesystems, 1);
    assert_eq!(
        physical_dir_size(&worktree, elsewhere).measure,
        Measure::Elsewhere,
        "the direct sizing route answers the same way, so no caller can differ"
    );
}

// statvfs pins the block unit statfs leaves ambiguous: Linux sizes f_blocks
// by f_frsize, and reading f_bsize instead is the bug this crossed off.
#[cfg(unix)]
#[test]
fn volume_bytes_reports_a_real_volume() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (capacity, available) = volume_bytes(tmp.path()).expect("statfs on a tempdir");
    assert!(capacity > 0, "capacity must be positive");
    assert!(
        available <= capacity,
        "available {available} exceeds capacity {capacity}"
    );

    let cpath = std::ffi::CString::new(tmp.path().to_string_lossy().as_bytes()).unwrap();
    // SAFETY: statvfs is zero-initializable POD, cpath is NUL-terminated, and
    // st is a valid out-pointer for the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::statvfs(cpath.as_ptr(), &mut st) }, 0);
    fn widen(v: impl TryInto<u64>) -> Option<u64> {
        v.try_into().ok()
    }
    let block = widen(st.f_frsize)
        .filter(|&size| size != 0)
        .or_else(|| widen(st.f_bsize))
        .unwrap();
    assert_eq!(
        capacity,
        block * widen(st.f_blocks).unwrap(),
        "statfs capacity must match statvfs blocks times the fundamental block size"
    );
}

/// One bucket per checkout, at either depth the old location has been written
/// in, and none for anything else under the root.
#[test]
fn buckets_open_for_a_checkout_at_either_depth_of_the_old_location() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("worktrees");
    let checkout = |dir: &Path, bytes: usize| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(".git"), "gitdir: /repo/.git/worktrees/one\n").unwrap();
        std::fs::write(dir.join("payload.bin"), vec![b'x'; bytes]).unwrap();
    };

    // The shape an unforked grok build leaves: the checkout is the root's child.
    let shallow = root.join("go-toolchain-dats-sandbox");
    checkout(&shallow, 65536);
    std::fs::create_dir_all(shallow.join("src/deep")).unwrap();
    std::fs::write(shallow.join("src/deep/more.bin"), vec![b'y'; 8192]).unwrap();
    // The fork's shape: a bucket per repository, a checkout per label.
    let deep = root.join("repos-buildhost/2026-09-14-reclaim");
    checkout(&deep, 8192);
    // A bucket holding a plain directory -- a go build cache, on the real
    // machine, at 208 MB. It is not a checkout and must not be a row.
    let cache = root.join("repos-buildhost/2026-09-14-gocache");
    std::fs::create_dir_all(cache.join("00")).unwrap();
    std::fs::write(cache.join("00/blob"), vec![b'z'; 32768]).unwrap();

    let sizes = physical_buckets(&root, Volume::of(&root));
    let mut got: Vec<PathBuf> = sizes.buckets.keys().cloned().collect();
    got.sort();
    assert_eq!(
        got,
        vec![shallow.clone(), deep.clone()],
        "one row per checkout, whatever depth it sits at"
    );
    assert_eq!(
        sizes.buckets[&shallow].bytes(),
        physical_dir_size(&shallow, Volume::of(&shallow))
            .measure
            .bytes(),
        "a depth-1 checkout is sized by its whole tree, so its subdirectories \
         are inside that row rather than rows of their own"
    );
    let not_checkouts = [
        root.clone(),
        cache.clone(),
        cache.join("00"),
        shallow.join("src"),
        shallow.join("src/deep"),
        root.join("repos-buildhost"),
    ];
    for not_a_checkout in &not_checkouts {
        assert!(
            !sizes.buckets.contains_key(not_a_checkout),
            "{not_a_checkout:?} is not a checkout and must not be bucketed"
        );
    }
    assert_eq!(
        sizes.total.bytes(),
        physical_dir_size(&root, Volume::of(&root)).measure.bytes(),
        "bucketing must not lose or double-count a byte of the root"
    );
}

/// A nested repository inside a checkout stays inside the row of the checkout
/// that contains it.
#[test]
fn a_repository_nested_in_a_checkout_adds_no_row() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("worktrees");
    let outer = root.join("my-checkout");
    std::fs::create_dir_all(&outer).unwrap();
    std::fs::write(
        outer.join(".git"),
        "gitdir: /repo/.git/worktrees/my-checkout\n",
    )
    .unwrap();
    std::fs::write(outer.join("payload.bin"), vec![b'x'; 8192]).unwrap();
    let nested = outer.join("vendor/lib");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/lib\n",
    )
    .unwrap();
    std::fs::write(nested.join("code.bin"), vec![b'q'; 4096]).unwrap();

    let sizes = physical_buckets(&root, Volume::of(&root));
    assert_eq!(
        sizes.buckets.keys().cloned().collect::<Vec<_>>(),
        vec![outer.clone()],
        "the nested repository is the outer checkout's byte, not a row beside it"
    );
    assert_eq!(
        sizes.buckets[&outer].bytes(),
        physical_dir_size(&outer, Volume::of(&outer))
            .measure
            .bytes()
    );
}
