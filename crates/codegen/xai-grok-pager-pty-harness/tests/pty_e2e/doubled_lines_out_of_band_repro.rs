//! Doubled-lines regression guard via a simulated out-of-band screen reflow.

use super::common::*;

/// Unique sentinel that grok would never render on its own.
const STALE_MARKER: &str = "STALE_OUT_OF_BAND_ROW_ZZZ";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore] // opt-in PTY e2e (run with `-- --ignored`)
async fn out_of_band_stale_row_heals_on_focus_gained() {
    let content = ContentController::start()
        .await
        .expect("start mock content");

    // Mock-auth env.
    let overrides: Vec<(String, String)> =
        vec![("NVIM".into(), "/tmp/grok-pty-harness-fake-nvim.sock".into())];
    let env_refs: Vec<(&str, &str)> = overrides
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let binary = pager_binary().expect("resolve pager binary");
    let mut h = PtyHarness::spawn_with_content_env(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &env_refs,
    )
    .expect("spawn pager");

    h.wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome screen");
    // Let the initial draws land before injecting The Welcome logo shimmers.
    h.update(Duration::from_millis(800));
    assert!(
        !h.contains_text(STALE_MARKER),
        "marker must be absent before injection"
    );

    h.feed_screen(format!("\x1b[6;1H{STALE_MARKER}").as_bytes());
    assert!(
        h.contains_text(STALE_MARKER),
        "marker should be on the virtual screen right after injection"
    );

    // grok must not self-heal out-of-band content via ordinary diff redraws It only rewrites cells whose own model changed.
    h.update(Duration::from_millis(300));
    assert!(
        h.contains_text(STALE_MARKER),
        "stale row should survive a normal redraw (grok's diff renderer doesn't own it)\nscreen:\n{}",
        h.screen_contents()
    );

    // A FocusGained (CSI I) forces a full clear and repaint that re-asserts grok's whole screen and removes the out-of-band row
    h.inject_keys(b"\x1b[I").expect("inject FocusGained");
    // Poll for the heal instead of a fixed settle so host load can't flake it.
    wait_for_labels_absent(&mut h, &[STALE_MARKER], Duration::from_secs(5));
    assert!(
        !h.contains_text(STALE_MARKER),
        "FocusGained should force a full repaint and clear the out-of-band row \
         (regression guard for the doubled-line bug)\nscreen:\n{}",
        h.screen_contents()
    );

    h.quit().expect("clean quit");
}
