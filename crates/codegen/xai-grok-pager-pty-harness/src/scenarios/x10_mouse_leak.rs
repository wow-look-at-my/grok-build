//! Relay-mangled X10 mouse reports must not type into the composer, and refocus must re-assert mouse capture.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::wait_for_welcome;
use crate::{ContentController, PtyHarness, pager_binary};

const DEFAULT_ROWS: u16 = 50;
const DEFAULT_COLS: u16 = 120;

const LEAKED_RAMP: &str = "PQRSTUVWXYZ";

/// `EnableMouseCapture`'s any-motion DECSET; startup emits it once.
const ANY_MOTION_ENABLE: &[u8] = b"\x1b[?1003h";

/// Drive both X10-leak defenses in one pager session: mangled reports parse as mouse events (nothing typed), and focus-in re-emits the mouse DECSETs.
pub async fn assert_x10_leak_defenses() -> Result<()> {
    let content = ContentController::start()
        .await
        .context("start ContentController")?;
    let binary = pager_binary().context("resolve pager binary")?;
    let mut harness =
        PtyHarness::spawn_with_content(&binary, DEFAULT_ROWS, DEFAULT_COLS, &content, &[])
            .context("spawn pager")?;

    wait_for_welcome(&mut harness).await?;

    // Positive control: the composer is live and echoes typed text, so the negative assertion below is meaningful
    harness.inject_keys(b"abc").context("type control text")?;
    harness
        .wait_for_text("abc", Duration::from_secs(10))
        .context("control text visible in composer")?;

    let mut sweep = Vec::new();
    for row_byte in LEAKED_RAMP.bytes() {
        sweep.extend_from_slice(b"\x1b[MC\xC2\x84");
        sweep.push(row_byte);
    }
    harness
        .inject_keys(&sweep)
        .context("inject mangled X10 sweep")?;

    // Ordering sentinel: input is processed in order Once the trailing "xyz" renders.
    harness.inject_keys(b"xyz").context("type sentinel text")?;
    harness
        .wait_for_text("xyz", Duration::from_secs(10))
        .context("sentinel text visible in composer")?;
    if !harness.contains_text("abcxyz")
        || harness.contains_text(LEAKED_RAMP.get(..3).unwrap_or(LEAKED_RAMP))
    {
        let composer_row = harness
            .screen_contents()
            .lines()
            .find(|l| l.contains("abc"))
            .unwrap_or_default()
            .trim()
            .to_string();
        bail!("the X10 sweep typed into the composer (composer row: {composer_row:?})");
    }

    // Refocus re-assert: focus-out then focus-in must re-emit the mouse DECSETs (only inspect output produced after the focus-in)
    let before = harness.raw_output().len();
    harness
        .inject_keys(b"\x1b[O\x1b[I")
        .context("inject focus-out/focus-in reports")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        harness.update(Duration::from_millis(50));
        let after = &harness.raw_output()[before..];
        if after
            .windows(ANY_MOTION_ENABLE.len())
            .any(|w| w == ANY_MOTION_ENABLE)
        {
            break;
        }
        if std::time::Instant::now() > deadline {
            bail!("focus-in did not re-assert mouse capture (no ?1003h after \\x1b[I)");
        }
    }

    harness.quit().context("quit pager")?;
    Ok(())
}
