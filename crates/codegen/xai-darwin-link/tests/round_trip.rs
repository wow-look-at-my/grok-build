//! Records a real rustc link, replays it with the shipped script, and runs the
//! binary that comes out.
//!
//! The unit tests read the argument list the recorder writes. They cannot say
//! whether that list still links, which is the only property the macOS job
//! depends on. This drives the same two programs CI drives — the recorder as
//! rustc's linker, then `ci/darwin-relink.sh` — for the host target, because a
//! Linux runner cannot execute a Mach-O binary to check it.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/codegen/xai-darwin-link.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "xai-darwin-link-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_recorded_link_replays_into_a_binary_that_runs() {
    let recorder = PathBuf::from(env!("CARGO_BIN_EXE_xai-darwin-link"));
    let work = temp_dir("round-trip");
    let source = work.join("hello.rs");
    std::fs::write(
        &source,
        "fn main() { println!(\"relinked\"); }\n",
    )
    .unwrap();
    let bundle = work.join("bundle");
    let recorded_output = work.join("hello-recorded");

    let compile = Command::new("rustc")
        .args(["--edition", "2021", "-O"])
        .arg(&source)
        .arg("-C")
        .arg(format!("linker={}", recorder.display()))
        .arg("-o")
        .arg(&recorded_output)
        .env("GROK_DARWIN_LINK_BUNDLE", &bundle)
        .output()
        .expect("rustc must run");
    assert!(
        compile.status.success(),
        "rustc failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(
        bundle.join("args").is_file(),
        "the linker must have recorded its arguments"
    );
    assert_eq!(
        std::fs::metadata(&recorded_output).unwrap().len(),
        0,
        "the recorder writes a placeholder, never a real binary"
    );

    let linked = work.join("hello-linked");
    let relink = Command::new("bash")
        .arg(repo_root().join("ci/darwin-relink.sh"))
        .arg(&bundle)
        .arg(&linked)
        .arg("elf")
        .output()
        .expect("the relink script must run");
    assert!(
        relink.status.success(),
        "relink failed: {}\n{}",
        String::from_utf8_lossy(&relink.stdout),
        String::from_utf8_lossy(&relink.stderr)
    );

    let ran = Command::new(&linked).output().expect("the binary must run");
    assert!(ran.status.success(), "the relinked binary must exit cleanly");
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "relinked");
    let _ = std::fs::remove_dir_all(&work);
}
