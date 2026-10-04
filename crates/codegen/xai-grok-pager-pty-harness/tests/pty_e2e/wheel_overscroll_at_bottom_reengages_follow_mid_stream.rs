// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;
#[allow(unused_imports)]
use super::scroll::*;

// A7: wheel overscroll at the bottom re-engages follow mode.

const MARKER_COUNT: usize = 240;

const UP_EVENTS: usize = 8;

const DOWN_EVENTS: usize = 60;

/// Space-separated tail words streamed one per delta after the marker block.
const TAIL_WORDS: usize = 240;

/// Per-SSE-event pacing so deltas keep arriving while the dance runs.
const CHUNK_DELAY: Duration = Duration::from_millis(30);

/// **Regression: overscroll at the bottom must re-engage follow.**
/// Mid-stream: wheel up to exit follow, wheel back down past the bottom, and the viewport must resume following.
/// The stream's final chunk appears with no further input.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn wheel_overscroll_at_bottom_reengages_follow_mid_stream() {
    // The helper spawns under the forced-wheel env (see the header) and returns a gated, paced transcript that is provably mid-turn
    let (mut harness, _content, mut turn, top_start) = spawn_streaming_marker_turn(
        MARKER_COUNT,
        TAIL_WORDS,
        CHUNK_DELAY,
        &[
            ("TERM_PROGRAM", "zed"),
            ("GROK_SCROLL_MODE", "wheel"),
            ("GROK_SCROLL_LINES", "1"),
        ],
    )
    .await;

    // Wheel up: exit follow and park the viewport above the live bottom.
    send_wheel_burst(
        &mut harness,
        SGR_SCROLL_UP,
        UP_EVENTS,
        WHEEL_ROW,
        WHEEL_COL,
        Duration::ZERO,
    );
    harness.update(Duration::from_millis(600));
    let top_parked = topmost_visible_marker(&harness).unwrap_or_else(|| {
        panic!(
            "no marker visible after the up-burst\nscreen:\n{}",
            harness.screen_contents()
        )
    });
    assert!(
        top_parked < top_start,
        "wheel-up did not exit follow / move the viewport: topmost visible \
         marker {} → {}\nscreen:\n{}",
        marker_line(top_start),
        marker_line(top_parked),
        harness.screen_contents()
    );

    // Deltas keep arriving but the parked viewport must not move: follow is off.
    harness.update(Duration::from_millis(500));
    assert_eq!(
        topmost_visible_marker(&harness),
        Some(top_parked),
        "viewport drifted while parked out of follow mode\nscreen:\n{}",
        harness.screen_contents()
    );

    // Wheel down past the bottom: land, then overscroll, which must re-engage follow
    send_wheel_burst(
        &mut harness,
        SGR_SCROLL_DOWN,
        DOWN_EVENTS,
        WHEEL_ROW,
        WHEEL_COL,
        Duration::ZERO,
    );
    harness.update(Duration::from_millis(800));
    let running = harness.is_running().expect("poll pager liveness");
    assert!(
        running && !harness.contains_text("panicked"),
        "pager broke during the wheel dance\nscreen:\n{}",
        harness.screen_contents()
    );
    // The completion gate is still held, so the turn is provably running.
    assert!(
        harness.contains_text("Responding"),
        "turn ended despite the held completion gate\nscreen:\n{}",
        harness.screen_contents()
    );

    // The core assertion: with follow re-engaged, the still-streaming tail pushes the final chunk into view with no further input
    // If the overscroll had not re-engaged follow, new rows would pile up below the viewport and the sentinel would never appear
    harness
        .wait_for_text(STREAM_END_SENTINEL, Duration::from_secs(40))
        .unwrap_or_else(|_| {
            panic!(
                "{STREAM_END_SENTINEL} never became visible — overscroll did not \
                 re-engage follow mode\nscreen:\n{}",
                harness.screen_contents()
            )
        });

    // Release the gate and let the turn complete: the dance didn't wedge it.
    turn.release();
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while harness.contains_text("Responding") {
        assert!(
            std::time::Instant::now() < deadline,
            "turn never completed after releasing its expectation\nscreen:\n{}",
            harness.screen_contents()
        );
        harness.update(Duration::from_millis(200));
    }
    tokio::time::timeout(Duration::from_secs(10), turn.wait_satisfied())
        .await
        .expect("streaming marker expectation satisfied");

    harness.quit().expect("clean quit");
}
