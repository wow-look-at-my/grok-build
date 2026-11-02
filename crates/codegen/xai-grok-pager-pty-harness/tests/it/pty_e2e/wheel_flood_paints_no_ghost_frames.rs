// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;
#[allow(unused_imports)]
use super::scroll::*;

// Regression: scroll pacing, one 16ms scroll clock, no ghost frames.

const MARKER_COUNT: usize = 240;

const BURST_EVENTS: usize = 30;

const BURST_INTERVAL: Duration = Duration::from_millis(6);

/// Minimum printable chars for a frame that scrolled the marker viewport.
const MOVEMENT_CHARS_FLOOR: usize = 10;

/// **Wheel-flood pacing regression.**
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn wheel_flood_paints_no_ghost_frames() {
    // The capture window spans the burst plus the post-burst residual and finalize flushes (the update() below outlasts the 80ms stream gap)
    let (mut harness, _content, top_before) =
        spawn_bottom_pinned_marker_scrollback(MARKER_COUNT).await;

    send_wheel_burst(
        &mut harness,
        SGR_SCROLL_UP,
        BURST_EVENTS,
        WHEEL_ROW,
        WHEEL_COL,
        BURST_INTERVAL,
    );
    harness.update(Duration::from_millis(600));

    assert!(
        harness.is_running().expect("poll pager liveness"),
        "pager exited during the wheel flood\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager rendered 'panicked' during the wheel flood\nscreen:\n{}",
        harness.screen_contents()
    );

    // (a) The viewport scrolled: the topmost visible marker index strictly decreased, so a marker that was off the top of the screen is now visible
    let top_after = topmost_visible_marker(&harness).unwrap_or_else(|| {
        panic!(
            "no marker visible after the wheel flood\nscreen:\n{}",
            harness.screen_contents()
        )
    });
    assert!(
        top_after < top_before,
        "wheel-up flood did not scroll the viewport: topmost visible marker \
         {} → {} (expected a decrease)\nscreen:\n{}",
        marker_line(top_before),
        marker_line(top_after),
        harness.screen_contents()
    );

    // (b) Coalescing bounds.
    let frames = harness.frame_count();
    assert!(
        frames >= 2,
        "a flood spanning many cadence slots must repaint more than once, got {frames}"
    );
    assert!(
        frames <= BURST_EVENTS as u64,
        "flood of {BURST_EVENTS} wheel events produced {frames} frames — more than \
         one repaint per event (frame amplification)"
    );

    // (c) No ghost frames: every frame in the burst window painted movement.
    for (i, timing) in harness.frame_timings().iter().enumerate() {
        assert!(
            timing.chars >= MOVEMENT_CHARS_FLOOR,
            "frame {i} of {frames} painted only {} chars (< {MOVEMENT_CHARS_FLOOR}) — \
             a ghost frame that moved no content\nscreen:\n{}",
            timing.chars,
            harness.screen_contents()
        );
    }

    harness.quit().expect("clean quit");
}
