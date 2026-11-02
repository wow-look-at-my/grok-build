// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;
#[allow(unused_imports)]
use super::scroll::*;

// Regression: streaming must not starve wheel input. The symptom: "can't scroll while it's streaming".

const MARKER_COUNT: usize = 240;

const BURST_EVENTS: usize = 30;

const BURST_INTERVAL: Duration = Duration::from_millis(6);

/// Space-separated tail words streamed one-per-delta after the marker block.
const TAIL_WORDS: usize = 160;

/// Per-SSE-event pacing so deltas keep arriving while the wheel burst runs.
const CHUNK_DELAY: Duration = Duration::from_millis(30);

/// Input-fairness regression. While a long response is still streaming (deltas paced, completion
/// gated), a wheel-up burst must scroll the viewport before the turn completes. Scrolling means the
/// topmost visible marker index strictly decreases. The turn must then complete cleanly on release.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn wheel_scrolls_viewport_during_streaming_turn() {
    // Gated, paced, provably mid-turn transcript with setup guards taken; see the helper for the construction and the baseline's meaning
    let (mut harness, _content, mut turn, top_before) =
        spawn_streaming_marker_turn(MARKER_COUNT, TAIL_WORDS, CHUNK_DELAY, &[]).await;

    send_wheel_burst(
        &mut harness,
        SGR_SCROLL_UP,
        BURST_EVENTS,
        WHEEL_ROW,
        WHEEL_COL,
        BURST_INTERVAL,
    );
    // Outlasts the 80ms stream gap so residual/finalize flushes land too.
    harness.update(Duration::from_millis(600));

    assert!(
        harness.is_running().expect("poll pager liveness"),
        "pager exited during the mid-stream wheel burst\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text("panicked"),
        "pager rendered 'panicked'\nscreen:\n{}",
        harness.screen_contents()
    );

    // The core assertion: the viewport moved while the turn was still
    // streaming.
    let top_after = topmost_visible_marker(&harness).unwrap_or_else(|| {
        panic!(
            "no marker visible after the mid-stream burst\nscreen:\n{}",
            harness.screen_contents()
        )
    });
    assert!(
        top_after < top_before,
        "wheel-up during streaming did not move the viewport: topmost visible \
         marker {} → {} (expected a decrease)\nscreen:\n{}",
        marker_line(top_before),
        marker_line(top_after),
        harness.screen_contents()
    );
    // Content-ordering witnesses that this happened mid-turn: the streaming status label is still up and the last chunk is not on screen
    // (The completion gate guarantees the turn itself cannot have ended.)
    assert!(
        harness.contains_text("Responding"),
        "movement observed but the streaming status label is gone — the turn \
         ended despite the held completion gate\nscreen:\n{}",
        harness.screen_contents()
    );
    assert!(
        !harness.contains_text(STREAM_END_SENTINEL),
        "movement observed but {STREAM_END_SENTINEL} is on screen\nscreen:\n{}",
        harness.screen_contents()
    );

    // Release the gate and let the tail finish: the turn must complete (status label clears); scrolling mid-stream didn't wedge it
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
