//! One skills-watcher startup per process, for
//! `scripts/bench-watcher-startup.sh` to time with hyperfine.
//!

use std::fs;
use std::path::Path;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use xai_grok_shell::config::watcher::SkillsFileWatcher;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["fixture", dirs, root] => {
            let dirs = dirs.parse().expect("worktree-dirs must be a number");
            fixture(Path::new(root), dirs);
        }
        ["scoped", root] => scoped(Path::new(root)),
        ["recursive-control", root] => recursive_control(Path::new(root)),
        _ => {
            eprintln!(
                "usage: skills_watcher_startup fixture <worktree-dirs> <root> | scoped <root> | recursive-control <root>"
            );
            std::process::exit(2);
        }
    }
}

fn fixture(root: &Path, worktree_dirs: usize) {
    let claude = root.join("project/.claude");
    let skill = claude.join("skills/alpha");
    fs::create_dir_all(&skill).expect("create skill dir");
    fs::write(skill.join("SKILL.md"), "# alpha\n").expect("write skill");
    let worktrees = claude.join("worktrees/wt1");
    for i in 0..worktree_dirs {
        fs::create_dir_all(
            worktrees
                .join(format!("g{}", i / 100))
                .join(format!("d{i}")),
        )
        .expect("create worktree dir");
    }
    fs::create_dir_all(root.join("grok-home")).expect("create grok home");
}

fn scoped(root: &Path) {
    let project = root.join("project");
    let (watcher, _rx) = SkillsFileWatcher::start_with_dirs(
        &[project.join(".claude")],
        &root.join("grok-home"),
        Some(project.as_path()),
    )
    .expect("scoped skills watcher should start");
    drop(watcher);
}

/// The control arm: one recursive watch on the whole `.claude`.
fn recursive_control(root: &Path) {
    let mut debouncer = new_debouncer(Duration::from_secs(2), |_| {}).expect("debouncer");
    debouncer
        .watcher()
        .watch(&root.join("project/.claude"), RecursiveMode::Recursive)
        .expect("recursive watch");
    drop(debouncer);
}
