//! One watcher startup per process, for `scripts/bench-watcher-startup.sh` to
//! time with hyperfine.

use std::fs;
use std::path::{Path, PathBuf};

use xai_fsnotify::{FsConfig, FsEventSource};

const TOTAL_DIRS: usize = 12_000;
const SCENARIOS: &[&str] = &[
    "favorable",
    "fanout_w48_with_target",
    "fanout_w64_no_ignored",
    "wide_w400",
    "nested_ignored_js_shape",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["fixture", scenario, dir] => fixture(scenario, Path::new(dir)),
        ["start", dir] => start(PathBuf::from(dir)),
        ["scenarios"] => println!("{}", SCENARIOS.join("\n")),
        _ => {
            eprintln!("usage: watcher_startup fixture <scenario> <dir> | start <dir> | scenarios");
            std::process::exit(2);
        }
    }
}

fn start(root: PathBuf) {
    // `start` blocks on its ready signal and only spawns the event loop, so a current-thread runtime is enough.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let _guard = rt.enter();
    let source = FsEventSource::start(root, FsConfig::default()).expect("start watcher");
    drop(source);
}

fn fixture(scenario: &str, root: &Path) {
    fs::create_dir_all(root.join(".git")).expect("create fixture root");
    match scenario {
        "favorable" => {
            fs::write(root.join(".gitignore"), "target/\n").unwrap();
            let target = TOTAL_DIRS * 2 / 3;
            let per = (TOTAL_DIRS - target) / 3;
            make_dirs(&root.join("src"), per);
            make_dirs(&root.join("crates"), per);
            make_dirs(&root.join("tests"), per);
            make_dirs(&root.join("target"), target);
        }
        "fanout_w48_with_target" => wide_tree(root, 48, true),
        "fanout_w64_no_ignored" => wide_tree(root, 64, false),
        "wide_w400" => wide_tree(root, 400, false),
        "nested_ignored_js_shape" => {
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
        }
        other => {
            eprintln!("unknown scenario {other}; known: {}", SCENARIOS.join(", "));
            std::process::exit(2);
        }
    }
}

/// `width` non-ignored top-level children. With `with_target`, half the dirs sit in a gitignored `target/`.
fn wide_tree(root: &Path, width: usize, with_target: bool) {
    let child_total = if with_target {
        TOTAL_DIRS / 2
    } else {
        TOTAL_DIRS
    };
    for i in 0..width {
        make_dirs(&root.join(format!("pkg{i}")), child_total / width);
    }
    if with_target {
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        make_dirs(&root.join("target"), TOTAL_DIRS / 2);
    }
}

fn make_dirs(base: &Path, count: usize) {
    for i in 0..count {
        let dir = base.join(format!("g{}", i / 100)).join(format!("d{i}"));
        fs::create_dir_all(&dir).unwrap();
    }
}
