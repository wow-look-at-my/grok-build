//! Gesture step tables G1-G11: the timed SGR wheel-report shapes every matrix cell replays.

use crate::scripted::{SGR_SCROLL_DOWN, SGR_SCROLL_UP};

/// Mirror of `mouse.rs` `REDRAW_CADENCE_MS`: minimum flush spacing.
pub const REDRAW_CADENCE_MS: u64 = 16;
/// Mirror of `mouse.rs` `STREAM_GAP_MS`: idle gap that finalizes a stream.
pub const STREAM_GAP_MS: u64 = 80;
/// Mirror of `mouse.rs` `DEFAULT_WHEEL_TICK_DETECT_MAX_MS`.
pub const WHEEL_TICK_DETECT_MAX_MS: u64 = 12;
/// Mirror of `mouse.rs` `ACCEL_MIN_INTERVAL_MS`.
pub const ACCEL_MIN_INTERVAL_MS: f64 = 6.0;
/// Mirror of `mouse.rs` `DEFAULT_TRACKPAD_ACCEL_MAX`: accel clamp ceiling.
pub const TRACKPAD_ACCEL_MAX: f64 = 3.0;
/// Mirror of `mouse.rs` `MIN_LINES_PER_WHEEL_STREAM`.
pub const MIN_LINES_PER_WHEEL_STREAM: i64 = 1;

/// One SGR wheel report: sleep `pre_delay_ms`, then emit `button`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelStep {
    pub pre_delay_ms: u64,
    pub button: u16,
}

/// `count` same-direction reports, `interval_ms` between consecutive ones.
const fn burst<const N: usize>(interval_ms: u64, button: u16) -> [WheelStep; N] {
    let mut steps = [WheelStep {
        pre_delay_ms: 0,
        button,
    }; N];
    let mut i = 1;
    while i < N {
        steps[i].pre_delay_ms = interval_ms;
        i += 1;
    }
    steps
}

/// Notches of `events_per_notch` back-to-back reports, `notch_gap_ms` apart.
const fn notch_train<const N: usize>(
    events_per_notch: usize,
    notch_gap_ms: u64,
    button: u16,
) -> [WheelStep; N] {
    let mut steps = burst::<N>(0, button);
    let mut i = events_per_notch;
    while i < N {
        if i.is_multiple_of(events_per_notch) {
            steps[i].pre_delay_ms = notch_gap_ms;
        }
        i += 1;
    }
    steps
}

pub const G1_NOTCH_EPT3: [WheelStep; 3] = burst::<3>(0, SGR_SCROLL_UP);
/// G1 single notch, ept=1 brands (iTerm2/zed/vscode/mux): one report.
pub const G1_NOTCH_EPT1: [WheelStep; 1] = burst::<1>(0, SGR_SCROLL_UP);
/// G2 notch train: notches 50ms apart (under `STREAM_GAP_MS`, so one stream).
pub const G2_NOTCH_TRAIN_EPT3: [WheelStep; 15] = notch_train::<15>(3, 50, SGR_SCROLL_UP);
pub const G2_NOTCH_TRAIN_EPT1: [WheelStep; 5] = burst::<5>(50, SGR_SCROLL_UP);
pub const G3_FLOOD: [WheelStep; 60] = burst::<60>(0, SGR_SCROLL_UP);
/// 8ms head stays Unknown (a dense burst would promote to wheel and never jerk). 40ms+ tail gaps open coast slots; tighter gaps mask them.
/// Gap finalize used to re-price accel-weighted and burst after input stopped.
pub const G4_JERK: [WheelStep; 66] = {
    let mut steps = burst::<66>(0, SGR_SCROLL_UP);
    steps[1].pre_delay_ms = 8;
    steps[2].pre_delay_ms = 8;
    let tail = [40, 44, 50, 55, 60, 70];
    let mut i = 0;
    while i < tail.len() {
        steps[60 + i].pre_delay_ms = tail[i];
        i += 1;
    }
    steps
};
pub const G5_GHOSTTY_DUP: [WheelStep; 20] = {
    let mut steps = burst::<20>(0, SGR_SCROLL_UP);
    let mut i = 1;
    while i < 20 {
        steps[i].pre_delay_ms = if i % 2 == 1 { 4 } else { 60 };
        i += 1;
    }
    steps
};
pub const G6_FLIP: [WheelStep; 20] = {
    let mut steps = burst::<20>(8, SGR_SCROLL_UP);
    let mut i = 10;
    while i < 20 {
        steps[i].button = SGR_SCROLL_DOWN;
        i += 1;
    }
    steps
};
pub const G7_OVERSCROLL: [WheelStep; 13] = {
    let mut steps = burst::<13>(8, SGR_SCROLL_DOWN);
    let mut i = 10;
    while i < 13 {
        steps[i].button = SGR_SCROLL_UP;
        i += 1;
    }
    steps
};
pub const G9A_MUX_SINGLES: [WheelStep; 8] = burst::<8>(55, SGR_SCROLL_UP);
pub const G9B_MUX_BATCH: [WheelStep; 24] = notch_train::<24>(3, 55, SGR_SCROLL_UP);
/// G10 ambiguous slow roll: reports 40ms apart, inside the vscode-embed 60ms trackpad-detect window but outside the default 30ms one.
pub const G10_AMBIGUOUS_SLOW: [WheelStep; 12] = burst::<12>(40, SGR_SCROLL_UP);
/// G11 carry: one notch, a 120ms wait (past `STREAM_GAP_MS`, so a finalize), then one notch.
pub const G11_CARRY_EPT3: [WheelStep; 6] = notch_train::<6>(3, 120, SGR_SCROLL_UP);
pub const G11_CARRY_EPT1: [WheelStep; 2] = burst::<2>(120, SGR_SCROLL_UP);

/// Gesture identifier a [`super::cells::MatrixCell`] references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GestureId {
    G1Notch,
    G2NotchTrain,
    G3Flood,
    G4Jerk,
    G5GhosttyDup,
    G6Flip,
    G7Overscroll,
    /// G2's step table replayed while the turn is still streaming (`SessionKind::Streaming`); the shape lives in the session, not here.
    G8MidStreamTrain,
    G9aMuxSingles,
    G9bMuxBatch,
    G10AmbiguousSlow,
    G11Carry,
}

impl GestureId {
    /// Every gesture, for exhaustive table sweeps (tests, the matrix runner).
    pub const ALL: [GestureId; 12] = [
        GestureId::G1Notch,
        GestureId::G2NotchTrain,
        GestureId::G3Flood,
        GestureId::G4Jerk,
        GestureId::G5GhosttyDup,
        GestureId::G6Flip,
        GestureId::G7Overscroll,
        GestureId::G8MidStreamTrain,
        GestureId::G9aMuxSingles,
        GestureId::G9bMuxBatch,
        GestureId::G10AmbiguousSlow,
        GestureId::G11Carry,
    ];

    /// Step table for this gesture on a brand with `ept` events per notch.
    /// Only the notch-based gestures (G1/G2/G8/G11) vary by class; the rest are fixed event shapes.
    /// G9b stays 3-per-notch even on the ept=1 mux profile: it simulates the mux passing re-chunked input.
    pub fn steps(self, ept: u16) -> &'static [WheelStep] {
        let ept3 = ept >= 2;
        match self {
            GestureId::G1Notch => {
                if ept3 {
                    &G1_NOTCH_EPT3
                } else {
                    &G1_NOTCH_EPT1
                }
            }
            GestureId::G2NotchTrain | GestureId::G8MidStreamTrain => {
                if ept3 {
                    &G2_NOTCH_TRAIN_EPT3
                } else {
                    &G2_NOTCH_TRAIN_EPT1
                }
            }
            GestureId::G3Flood => &G3_FLOOD,
            GestureId::G4Jerk => &G4_JERK,
            GestureId::G5GhosttyDup => &G5_GHOSTTY_DUP,
            GestureId::G6Flip => &G6_FLIP,
            GestureId::G7Overscroll => &G7_OVERSCROLL,
            GestureId::G9aMuxSingles => &G9A_MUX_SINGLES,
            GestureId::G9bMuxBatch => &G9B_MUX_BATCH,
            GestureId::G10AmbiguousSlow => &G10_AMBIGUOUS_SLOW,
            GestureId::G11Carry => {
                if ept3 {
                    &G11_CARRY_EPT3
                } else {
                    &G11_CARRY_EPT1
                }
            }
        }
    }

    pub fn expected_streams(self) -> usize {
        match self {
            GestureId::G6Flip | GestureId::G7Overscroll | GestureId::G11Carry => 2,
            _ => 1,
        }
    }
}

/// `(up, down)` report counts, used by the direction-sum tests.
pub fn direction_counts(steps: &[WheelStep]) -> (usize, usize) {
    let up = steps.iter().filter(|s| s.button == SGR_SCROLL_UP).count();
    (up, steps.len() - up)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Streams split on gaps over STREAM_GAP_MS or on direction flips; this recomputes the count from the table to compare with `expected_streams`.
    fn streams_in(steps: &[WheelStep]) -> usize {
        let mut streams = 1;
        for pair in steps.windows(2) {
            if pair[1].pre_delay_ms > STREAM_GAP_MS || pair[1].button != pair[0].button {
                streams += 1;
            }
        }
        streams
    }

    #[test]
    fn table_counts_and_direction_sums() {
        assert_eq!(G1_NOTCH_EPT3.len(), 3);
        assert_eq!(G1_NOTCH_EPT1.len(), 1);
        assert_eq!(G2_NOTCH_TRAIN_EPT3.len(), 15);
        assert_eq!(G2_NOTCH_TRAIN_EPT1.len(), 5);
        assert_eq!(G3_FLOOD.len(), 60);
        assert_eq!(G4_JERK.len(), 66);
        assert_eq!(G5_GHOSTTY_DUP.len(), 20);
        assert_eq!(G9B_MUX_BATCH.len(), 24);
        assert_eq!(G10_AMBIGUOUS_SLOW.len(), 12);

        assert_eq!(direction_counts(&G3_FLOOD), (60, 0));
        assert_eq!(direction_counts(&G6_FLIP), (10, 10), "flip nets to zero");
        assert_eq!(direction_counts(&G7_OVERSCROLL), (3, 10));
        assert_eq!(direction_counts(&G9A_MUX_SINGLES), (8, 0));
    }

    #[test]
    fn notch_structure_and_gaps() {
        for (i, step) in G2_NOTCH_TRAIN_EPT3.iter().enumerate() {
            let expected = if i > 0 && i % 3 == 0 { 50 } else { 0 };
            assert_eq!(step.pre_delay_ms, expected, "G2 ept3 step {i}");
        }
        // G9b: same shape at 55ms (under the 80ms gap, one stream)
        let notch_gaps = G9B_MUX_BATCH
            .iter()
            .filter(|s| s.pre_delay_ms == 55)
            .count();
        assert_eq!(notch_gaps, 7, "8 notches → 7 inter-notch gaps");
        // G5: dup 4ms after each notch head, notch heads 60ms apart.
        for (i, step) in G5_GHOSTTY_DUP.iter().enumerate() {
            let expected = if i == 0 {
                0
            } else if i % 2 == 1 {
                4
            } else {
                60
            };
            assert_eq!(step.pre_delay_ms, expected, "G5 step {i}");
        }
    }

    #[test]
    fn jerk_head_blocks_promotion_and_tail_decays_monotonically() {
        // Anti-promotion head: even with zero jitter (sleeps only stretch).
        let head_span: u64 = G4_JERK[..3].iter().map(|s| s.pre_delay_ms).sum();
        assert!(head_span > WHEEL_TICK_DETECT_MAX_MS);

        let tail: Vec<u64> = G4_JERK[60..].iter().map(|s| s.pre_delay_ms).collect();
        assert_eq!(tail, vec![40, 44, 50, 55, 60, 70]);
        assert!(
            tail.windows(2).all(|w| w[0] < w[1]),
            "strictly decelerating"
        );
        // Coast window: every tail gap opens at least empty 16ms cadence slots so the dense backlog drains as events_since_flush == 0 flushes
        assert!(tail.iter().all(|&gap| gap >= 2 * REDRAW_CADENCE_MS));
        assert!(G4_JERK[3..60].iter().all(|s| s.pre_delay_ms == 0));
    }

    #[test]
    fn delays_agree_with_stream_gap_thresholds() {
        // Single-stream gestures never pause past the 80ms finalize gap and never flip; multi-stream ones split exactly as declared
        for gesture in GestureId::ALL {
            for ept in [1u16, 3] {
                let steps = gesture.steps(ept);
                assert!(!steps.is_empty());
                assert_eq!(steps[0].pre_delay_ms, 0, "{gesture:?} first step");
                assert_eq!(
                    streams_in(steps),
                    gesture.expected_streams(),
                    "{gesture:?} ept={ept}: table shape vs declared stream count"
                );
            }
        }
        // The G11 pause is what splits it: strictly past the finalize gap.
        assert!(G11_CARRY_EPT3[3].pre_delay_ms > STREAM_GAP_MS);
        assert!(G11_CARRY_EPT1[1].pre_delay_ms > STREAM_GAP_MS);
        // G1/ept3 is a first tick inside the wheel-promotion window.
        let g1_span: u64 = G1_NOTCH_EPT3.iter().map(|s| s.pre_delay_ms).sum();
        assert!(g1_span <= WHEEL_TICK_DETECT_MAX_MS);
        // G5's dup spacing must sit under the interval-window floor.
        assert!((G5_GHOSTTY_DUP[1].pre_delay_ms as f64) < ACCEL_MIN_INTERVAL_MS);
    }
}
