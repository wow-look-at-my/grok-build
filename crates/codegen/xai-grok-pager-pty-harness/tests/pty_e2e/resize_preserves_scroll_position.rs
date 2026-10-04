// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

// Reproduction: horizontal resize must not lose the scroll position.

/// Unique marker on its own (non-wrapping) line, with WRAPPING content above it.
const MARKER: &str = "SCROLL_ANCHOR_MARKER_ZZZ";

/// First line of the response.
const TOP_SENTINEL: &str = "TOP_OF_RESPONSE_AAA";

/// Last line of the response. When this is NOT on screen we are not pinned to the bottom, not following.
const BOTTOM_SENTINEL: &str = "BOTTOM_OF_RESPONSE_QQQ";

/// Number of long, WRAPPING paragraphs placed above the marker.
const WRAP_LINES_ABOVE: usize = 10;

/// Short, NON-wrapping paragraphs placed immediately above the marker.
const GUARD_LINES: usize = 16;

/// Short, non-wrapping filler below the marker.
const FILLER_BELOW: usize = 30;

/// Spawn width (cols).
const WIDE_COLS: u16 = DEFAULT_COLS;

/// Resize width (cols).
const NARROW_COLS: u16 = 80;

/// Allowed marker drift (viewport rows) across a width-only resize.
const POS_TOLERANCE: i32 = 2;

/// That gives a predictable one-hard-line-per-line layout where only the long paragraphs re-wrap on
/// a width change.
fn scroll_anchor_response() -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    paragraphs.push(TOP_SENTINEL.to_string());

    let wrapping = "W".repeat(220);
    for _ in 0..WRAP_LINES_ABOVE {
        paragraphs.push(wrapping.clone());
    }

    // Short guard paragraphs directly above the marker (do NOT re-wrap on resize).
    for i in 0..GUARD_LINES {
        paragraphs.push(format!("guard-line-{i:02}"));
    }

    paragraphs.push(MARKER.to_string());

    for i in 0..FILLER_BELOW {
        paragraphs.push(format!("below-line-{i:02}"));
    }
    paragraphs.push(BOTTOM_SENTINEL.to_string());

    paragraphs.join("\n\n")
}

/// SGR mouse wheel-up at a position inside the scrollback pane (1-based wire coords).
const WHEEL_UP: &[u8] = b"\x1b[<64;40;12M";
const WHEEL_DOWN: &[u8] = b"\x1b[<65;40;12M";

/// Park the marker mid-viewport, scrolled into the MIDDLE of the transcript: marker visible, TOP
/// and BOTTOM sentinels both scrolled off.
fn park_marker_mid(h: &mut PtyHarness) -> Option<(u16, u16)> {
    // Band (screen rows) we want the marker parked in Aim for the MIDDLE of the viewport so the jump keeps the marker on screen.
    const BAND_LO: u16 = 20;
    const BAND_HI: u16 = 32;

    let deadline = Instant::now() + Duration::from_secs(40);

    while Instant::now() < deadline {
        if locate_screen_text(&h.screen_contents(), MARKER).is_some() {
            break;
        }
        for _ in 0..8 {
            let _ = h.inject_keys(WHEEL_UP);
        }
        h.update(Duration::from_millis(200));
    }

    while Instant::now() < deadline {
        let screen = h.screen_contents();
        let pos = locate_screen_text(&screen, MARKER);
        let top_vis = screen.contains(TOP_SENTINEL);
        let bottom_vis = screen.contains(BOTTOM_SENTINEL);

        match pos {
            Some((row, _)) if !top_vis && !bottom_vis && (BAND_LO..=BAND_HI).contains(&row) => {
                return pos;
            }
            Some((row, _)) if row > BAND_HI || bottom_vis => {
                // Marker too low (or bottom in view): scroll DOWN to raise it.
                for _ in 0..2 {
                    let _ = h.inject_keys(WHEEL_DOWN);
                }
                h.update(Duration::from_millis(160));
            }
            _ => {
                // Marker too high, top in view, or not visible: scroll UP
                for _ in 0..2 {
                    let _ = h.inject_keys(WHEEL_UP);
                }
                h.update(Duration::from_millis(160));
            }
        }
    }
    locate_screen_text(&h.screen_contents(), MARKER)
}

/// Resize preserves scroll position (reproduction). Then resize the WIDTH only. The marker must
/// stay at ~the same viewport row across the reflow. Without the scroll-anchor fix it jumps (stale
/// `scroll_offset`), which is the bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn resize_preserves_scroll_position() {
    let content = ContentController::start().await.expect("start content");
    content.set_response(scroll_anchor_response());

    // Spawn FULLSCREEN (alt-screen).
    let binary = pager_binary().expect("resolve pager binary");
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, WIDE_COLS, &content, &[])
            .expect("spawn pager with content");

    harness
        .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
        .expect("welcome text");

    // Submit a prompt and wait for the whole response to render The bottom
    // sentinel is the last thing streamed.
    harness
        .inject_keys(format!("{PROMPT}\r").as_bytes())
        .expect("submit prompt");
    harness
        .wait_for_text(BOTTOM_SENTINEL, Duration::from_secs(120))
        .unwrap_or_else(|_| {
            panic!(
                "setup: response never finished streaming (no {BOTTOM_SENTINEL:?})\nscreen:\n{}",
                harness.screen_contents()
            )
        });
    harness.update(Duration::from_millis(400));

    // Focus scrollback (Esc) so wheel scroll is unambiguous, then park the marker mid-viewport, scrolled into the middle of the transcript
    harness.inject_keys(keys::ESC).expect("focus scrollback");
    harness.update(Duration::from_millis(250));

    let before = park_marker_mid(&mut harness).unwrap_or_else(|| {
        panic!(
            "setup: could not park the marker mid-viewport\nscreen:\n{}",
            harness.screen_contents()
        )
    });
    let screen_before = harness.screen_contents();

    // Setup guards: we must be scrolled into the MIDDLE (bug regime), i.e.
    // the marker is visible while BOTH sentinels are scrolled off.
    assert!(
        !screen_before.contains(TOP_SENTINEL),
        "setup: TOP sentinel visible → scroll_offset == 0 (at the absolute top, \
         not the bug regime). Marker row {}.\nscreen:\n{screen_before}",
        before.0
    );
    assert!(
        !screen_before.contains(BOTTOM_SENTINEL),
        "setup: BOTTOM sentinel visible → still pinned to the bottom / following \
         (no jump expected). Marker row {}.\nscreen:\n{screen_before}",
        before.0
    );

    // ── Resize the WIDTH only (rows unchanged) and let prepare_layout rebuild the wrapped row map at the new width
    harness
        .resize(DEFAULT_ROWS, NARROW_COLS)
        .expect("resize narrower");
    harness.update(Duration::from_millis(900));
    let screen_after = harness.screen_contents();

    assert!(
        harness.is_running().expect("poll pager liveness"),
        "pager exited during resize\nscreen:\n{screen_after}"
    );
    assert!(
        !screen_after.contains("panicked"),
        "pager rendered 'panicked' during resize\nscreen:\n{screen_after}"
    );

    // ── The reproduction assertion: the marker must still be visible AND at ~the same viewport row after a width-only resize
    let (after_row, _) = locate_screen_text(&screen_after, MARKER).unwrap_or_else(|| {
        panic!(
            "REPRO (marker scrolled off after a width-only resize {WIDE_COLS}→{NARROW_COLS} cols): \
             marker was visible at row {} before, gone after. Stale scroll_offset (absolute \
             wrapped-row count) left unchanged across the reflow.\nscreen before:\n{screen_before}\n\
             screen after:\n{screen_after}",
            before.0
        )
    });

    let delta = (after_row as i32 - before.0 as i32).abs();
    assert!(
        delta <= POS_TOLERANCE,
        "REPRO (marker jumped on a width-only resize {WIDE_COLS}→{NARROW_COLS} cols): \
         marker viewport row {} → {} (|Δ|={} > tolerance {}). Re-wrapping the content above the \
         marker grew its height, but scroll_offset (an absolute wrapped-row count) was left \
         unchanged, so the view lost its scroll position.\nscreen before:\n{screen_before}\n\
         screen after:\n{screen_after}",
        before.0,
        after_row,
        delta,
        POS_TOLERANCE
    );

    harness.quit().expect("clean quit");
}
