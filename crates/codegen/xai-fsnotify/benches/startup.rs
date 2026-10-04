//! Watcher startup-latency benchmark.

use std::fs;
use std::path::Path;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use tempfile::TempDir;
use xai_fsnotify::{FsConfig, FsEventSource};

const TOTAL_DIRS: usize = 12_000;

fn make_dirs(base: &Path, count: usize) {
    for i in 0..count {
        let dir = base.join(format!("g{}", i / 100)).join(format!("d{i}"));
        fs::create_dir_all(&dir).unwrap();
    }
}

/// Kept at ~`TOTAL_DIRS` for comparability with the others.
fn build_favorable_tree() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let target = TOTAL_DIRS * 2 / 3;
    let per = (TOTAL_DIRS - target) / 3;
    make_dirs(&root.join("src"), per);
    make_dirs(&root.join("crates"), per);
    make_dirs(&root.join("tests"), per);
    make_dirs(&root.join("target"), target);
    temp
}

/// `width` non-ignored top-level children. When `with_target`, half the dirs
/// live in a gitignored `target/` (skipped by fan-out); otherwise nothing is
/// ignored.
fn build_wide_tree(width: usize, with_target: bool) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    let child_total = if with_target {
        TOTAL_DIRS / 2
    } else {
        TOTAL_DIRS
    };
    let per_child = child_total / width;
    for i in 0..width {
        make_dirs(&root.join(format!("pkg{i}")), per_child);
    }
    if with_target {
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        make_dirs(&root.join("target"), TOTAL_DIRS / 2);
    }
    temp
}

/// JS-monorepo shape: sources are a minority; the rest live in nested `node_modules/`.
/// Fan-out recursive child watches cannot skip those dirs (each still costs an inotify watch).
/// Per-dir mode prunes them.
fn build_nested_ignored_tree() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
    let src = TOTAL_DIRS / 6;
    let ignored = TOTAL_DIRS - 2 * src;
    for i in 0..4 {
        make_dirs(&root.join(format!("packages/pkg{i}/src")), src / 4);
        make_dirs(
            &root.join(format!("packages/pkg{i}/node_modules")),
            ignored / 8,
        );
    }
    make_dirs(&root.join("node_modules"), ignored / 2);
    make_dirs(&root.join("apps/web/src"), src);
    temp
}

fn bench_startup(c: &mut Criterion) {
    // `start` blocks on a std mpsc ready signal.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();

    let mut group = c.benchmark_group("watcher_startup");
    group.sample_size(30);

    let mut run = |name: &str, tree: &TempDir| {
        let root = tree.path().to_path_buf();
        group.bench_function(name, |b| {
            // PerIteration drops each watcher (freeing its inotify watches)
            // before the next iteration and keeps the drop out of timing.
            b.iter_batched(
                || (),
                |()| FsEventSource::start(root.clone(), FsConfig::default()).expect("start"),
                BatchSize::PerIteration,
            );
        });
    };

    let favorable = build_favorable_tree();
    run("favorable", &favorable);

    let moderate = build_wide_tree(48, true);
    run("fanout_w48_with_target", &moderate);

    let worst = build_wide_tree(64, false);
    run("fanout_w64_no_ignored", &worst);

    let wide = build_wide_tree(400, false);
    run("wide_w400", &wide);

    let nested = build_nested_ignored_tree();
    run("nested_ignored_js_shape", &nested);

    group.finish();
}

criterion_group!(benches, bench_startup);
criterion_main!(benches);
