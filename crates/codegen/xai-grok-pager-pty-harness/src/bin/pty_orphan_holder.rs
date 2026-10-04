//! Test fixture for the orphan-reap regression test (`tests/orphan_reap.rs`).

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use xai_grok_pager_pty_harness::{EnvOp, PtyController};
use xai_grok_test_support::TestSandbox;

fn main() -> anyhow::Result<()> {
    let sandbox = TestSandbox::new();
    let ready = sandbox.temp_dir().join("child-ready");
    let ready_path = ready.to_string_lossy().into_owned();
    let controller = PtyController::spawn_in_sandbox(
        Path::new("/bin/sh"),
        PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 0,
            pixel_height: 0,
        },
        &["-c", "trap '' HUP; : > \"$READY\"; while :; do :; done"],
        &sandbox,
        &[EnvOp::set("READY", &ready_path)],
        None,
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        if Instant::now() >= deadline {
            anyhow::bail!("PTY child did not write READY within 30s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = controller
        .child_pid()
        .ok_or_else(|| anyhow::anyhow!("PTY child has no pid"))?;
    println!("CHILD_PID={pid}");
    std::io::stdout().flush()?;

    // Hold the controller (and its child) until the test kills this process.
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
