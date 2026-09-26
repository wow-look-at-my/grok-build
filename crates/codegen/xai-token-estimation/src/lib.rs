//! Pure shared token-estimation primitives.
//!
//! This crate is the single source of truth for the bytes/4 heuristic and the
//! derived-display arithmetic that `/context`, `/session-info`, the auto-compact
//! gates, the preflight overflow check, and every client renderer use to talk
//! about context-window usage.

#![deny(clippy::indexing_slicing)]

/// Bytes per token under the rough character-based heuristic.
pub const BYTES_PER_TOKEN: u64 = 4;

/// Per-image approximate token cost when summing
/// low-resolution image patches.
pub const IMAGE_TOKEN_ESTIMATE: u64 = 765;

/// Bytes/4 estimate of a string's token count.
#[inline]
pub fn estimate_tokens(s: &str) -> u64 {
    (s.len() as u64) / BYTES_PER_TOKEN
}

/// Inverse of [`estimate_tokens`]: convert a token budget into a character
/// budget. Used by skill discovery to size text passages against the model's
/// context window.
#[inline]
pub fn estimate_chars(tokens: u64) -> u64 {
    tokens.saturating_mul(BYTES_PER_TOKEN)
}

/// Token estimate for `image_count` images at [`IMAGE_TOKEN_ESTIMATE`] each.
#[inline]
pub fn estimate_image_tokens(image_count: u64) -> u64 {
    image_count.saturating_mul(IMAGE_TOKEN_ESTIMATE)
}

/// Usage percentage as `f64`, clamped to `100.0`. Returns `0.0` when
/// `total == 0`.
#[inline]
pub fn usage_percentage(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((used as f64) / (total as f64) * 100.0).min(100.0)
    }
}

/// Usage percentage rounded to `u8`, clamped to `100`.
#[inline]
pub fn usage_percentage_u8(used: u64, total: u64) -> u8 {
    usage_percentage(used, total).round() as u8
}

/// Integer-arithmetic (truncating) usage percentage, clamped to `100`. Differs from [`usage_percentage_u8`] in two ways:
/// no `f64` round-trip, and the result is truncated (not rounded). Returns `u8` because the result is bounded to `100`.
/// Saturates on overflow via `saturating_mul`.
#[inline]
pub fn usage_percentage_truncated_u8(used: u64, total: u64) -> u8 {
    used.saturating_mul(100)
        .checked_div(total)
        .map_or(0, |pct| pct.min(100) as u8)
}

/// `total - used`, saturating at zero. The "free" portion of the context
/// window for `/context` rendering.
#[inline]
pub fn free_tokens(total: u64, used: u64) -> u64 {
    total.saturating_sub(used)
}

/// True when `used >= context_window * threshold_percent / 100`. Returns `false` for `context_window == 0` so callers do
/// not have to special-case missing windows. Computed in integer arithmetic to match the existing auto-compact gate
/// semantics.
#[inline]
pub fn exceeds_threshold(used: u64, context_window: u64, threshold_percent: u8) -> bool {
    if context_window == 0 {
        return false;
    }
    used.saturating_mul(100) >= context_window.saturating_mul(threshold_percent as u64)
}

/// True when `used * 100 >= context_window * threshold_percent - headroom * 100`,
/// the scaled form of [`exceeds_threshold`] minus a token headroom.
/// Returns `false` for `context_window == 0`.
#[inline]
pub fn exceeds_threshold_with_headroom(
    used: u64,
    context_window: u64,
    threshold_percent: u8,
    headroom: u64,
) -> bool {
    if context_window == 0 {
        return false;
    }
    used.saturating_mul(100)
        >= context_window
            .saturating_mul(threshold_percent as u64)
            .saturating_sub(headroom.saturating_mul(100))
}

/// Smallest output budget a request may carry.
///
/// A provider rejects `max_tokens: 0`, and an answer shorter than this is
/// not an answer. A prompt that leaves less room than this is over the
/// window: the request goes out at this floor and the provider's own
/// overflow error is the honest report of it.
pub const MIN_OUTPUT_TOKENS: u64 = 1024;

/// Share of the window held back when the prompt size is an estimate.
///
/// Every prompt count in this process is bytes/4, or the provider's own count
/// for the last response plus a bytes/4 delta. Either can read low, and one
/// token low is a rejected request. 1% of the window is slack the output
/// budget will not miss.
pub const PROMPT_ESTIMATE_SLACK_PERCENT: u64 = 1;

/// The window to fit an output budget into when the prompt count is an
/// estimate: [`PROMPT_ESTIMATE_SLACK_PERCENT`] held back.
///
/// Returns 0 for an unknown (`0`) window, which [`fit_output_tokens`] reads as
/// "make no claim".
#[inline]
pub fn window_less_estimate_slack(context_window: u64) -> u64 {
    context_window.saturating_sub(
        context_window
            .saturating_mul(PROMPT_ESTIMATE_SLACK_PERCENT)
            .saturating_div(100),
    )
}

/// Room left in `context_window` for the response once the prompt is in it.
///
/// Returns 0 when the prompt alone fills the window.
#[inline]
pub fn output_room(context_window: u64, prompt_tokens: u64) -> u64 {
    context_window.saturating_sub(prompt_tokens)
}

/// The output budget a request may ask for so that prompt plus output stays
/// inside `context_window`.
///
/// A provider counts the requested output against the same window as the
/// prompt, so `requested` is not a free parameter: a 737_857-token prompt and
/// a 262_144-token output budget is 1_000_001 tokens against a 1_000_000-token
/// window, and the server rejects the request rather than the answer.
///
/// `context_window == 0` means the window is unknown, so the caller's value
/// passes through unchanged. The result never drops below
/// [`MIN_OUTPUT_TOKENS`] — see that constant for why.
#[inline]
pub fn fit_output_tokens(requested: u64, prompt_tokens: u64, context_window: u64) -> u64 {
    if context_window == 0 {
        return requested;
    }
    let room = output_room(context_window, prompt_tokens);
    if requested <= room {
        requested
    } else {
        room.max(MIN_OUTPUT_TOKENS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_that_fits_is_left_alone() {
        assert_eq!(fit_output_tokens(262_144, 100_000, 1_000_000), 262_144);
        // Exactly full is still legal: 737_856 + 262_144 == 1_000_000.
        assert_eq!(fit_output_tokens(262_144, 737_856, 1_000_000), 262_144);
    }

    /// The reported failure, to the token: one over the window.
    #[test]
    fn the_output_budget_shrinks_to_what_the_window_has_left() {
        let fitted = fit_output_tokens(262_144, 737_857, 1_000_000);
        assert_eq!(fitted, 262_143);
        assert_eq!(737_857 + fitted, 1_000_000);
    }

    #[test]
    fn a_prompt_that_fills_the_window_still_asks_for_the_floor() {
        assert_eq!(
            fit_output_tokens(262_144, 1_000_000, 1_000_000),
            MIN_OUTPUT_TOKENS
        );
        assert_eq!(
            fit_output_tokens(262_144, 2_000_000, 1_000_000),
            MIN_OUTPUT_TOKENS
        );
        assert_eq!(
            fit_output_tokens(262_144, 999_999, 1_000_000),
            MIN_OUTPUT_TOKENS
        );
    }

    #[test]
    fn an_unknown_window_changes_nothing() {
        assert_eq!(fit_output_tokens(262_144, 737_857, 0), 262_144);
    }

    #[test]
    fn the_slack_is_one_percent_and_zero_stays_zero() {
        assert_eq!(window_less_estimate_slack(1_000_000), 990_000);
        assert_eq!(window_less_estimate_slack(0), 0);
        // The reported prompt then leaves 252_143 for the answer instead of
        // landing exactly on the wall.
        assert_eq!(
            fit_output_tokens(262_144, 737_857, window_less_estimate_slack(1_000_000)),
            252_143
        );
    }

    #[test]
    fn output_room_saturates_at_zero() {
        assert_eq!(output_room(1_000_000, 737_857), 262_143);
        assert_eq!(output_room(1_000, 2_000), 0);
    }

    #[test]
    fn estimate_tokens_is_bytes_over_four() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens(&"x".repeat(4000)), 1000);
    }

    #[test]
    fn estimate_chars_is_inverse() {
        assert_eq!(estimate_chars(0), 0);
        assert_eq!(estimate_chars(1), 4);
        assert_eq!(estimate_chars(1000), 4000);
    }

    #[test]
    fn estimate_image_tokens_uses_constant() {
        assert_eq!(estimate_image_tokens(0), 0);
        assert_eq!(estimate_image_tokens(1), IMAGE_TOKEN_ESTIMATE);
        assert_eq!(estimate_image_tokens(3), 3 * IMAGE_TOKEN_ESTIMATE);
    }

    #[test]
    fn usage_percentage_clamps_and_handles_zero_total() {
        assert_eq!(usage_percentage(0, 0), 0.0);
        assert_eq!(usage_percentage(50, 100), 50.0);
        assert_eq!(usage_percentage(150, 100), 100.0);
        assert_eq!(usage_percentage(100, 0), 0.0);
    }

    #[test]
    fn usage_percentage_u8_rounds() {
        assert_eq!(usage_percentage_u8(0, 100), 0);
        assert_eq!(usage_percentage_u8(50, 100), 50);
        assert_eq!(usage_percentage_u8(99, 100), 99);
        // 12_700 / 256_000 = 0.04960... -> 5 after rounding
        assert_eq!(usage_percentage_u8(12_700, 256_000), 5);
        assert_eq!(usage_percentage_u8(150, 100), 100);
    }

    /// Half-boundary contract — locks rounding direction. `85 / 200 = 0.425`
    /// becomes `42.5%` which rounds half-up to `43`. The truncating helper
    /// returns `42` for the same input (see `usage_percentage_truncated_u8`).
    #[test]
    fn usage_percentage_u8_rounds_half_up() {
        assert_eq!(usage_percentage_u8(85, 200), 43);
        // 7 / 8 = 0.875, rounds to 88 (truncated would be 87).
        assert_eq!(usage_percentage_u8(7, 8), 88);
    }

    #[test]
    fn usage_percentage_truncated_u8_clamps_and_handles_zero_total() {
        assert_eq!(usage_percentage_truncated_u8(0, 0), 0);
        assert_eq!(usage_percentage_truncated_u8(50, 100), 50);
        assert_eq!(usage_percentage_truncated_u8(150, 100), 100);
        // Large values do not overflow because we use saturating_mul.
        assert_eq!(usage_percentage_truncated_u8(u64::MAX, 1), 100);
    }

    /// Truncation contract — distinguishes this helper from `usage_percentage_u8`, which rounds. Locks in that
    /// `exceeds_threshold(used, cw, p)` and `usage_percentage_truncated_u8(used, cw) >= p` agree.
    #[test]
    fn usage_percentage_truncated_u8_truncates_does_not_round() {
        // 85 / 200 = 0.425, truncated -> 42 (rounded would be 43).
        assert_eq!(usage_percentage_truncated_u8(85, 200), 42);
        // 7 / 8 = 0.875, truncated -> 87 (rounded would be 88).
        assert_eq!(usage_percentage_truncated_u8(7, 8), 87);
    }

    #[test]
    fn free_tokens_saturates() {
        assert_eq!(free_tokens(100, 30), 70);
        assert_eq!(free_tokens(100, 100), 0);
        assert_eq!(free_tokens(100, 200), 0);
    }

    #[test]
    fn exceeds_threshold_matches_integer_pct() {
        assert!(!exceeds_threshold(50, 100, 85));
        assert!(exceeds_threshold(85, 100, 85));
        assert!(exceeds_threshold(99, 100, 85));
        assert!(!exceeds_threshold(50, 0, 85));
    }

    /// Strict-boundary contract — pin the `>=` semantics. At cw=1000, pct=85, `850 * 100 == 1000 * 85` so the gate must fire
    /// at exactly 850 tokens. This is one token earlier than the legacy `>` gate (`total > cw * pct / 100` which fired at
    /// 851).
    #[test]
    fn exceeds_threshold_fires_on_strict_boundary() {
        assert!(exceeds_threshold(850, 1000, 85));
        assert!(!exceeds_threshold(849, 1000, 85));
        // 1000 * 85 / 100 = 850, so 850 is the new strict boundary.
        // Same shape at the other commonly-configured threshold (95%):
        assert!(exceeds_threshold(950, 1000, 95));
        assert!(!exceeds_threshold(949, 1000, 95));
    }

    /// Property: with `headroom == 0` the helper agrees with
    /// [`exceeds_threshold`] across a representative grid of inputs,
    /// including the non-round windows where floor-divide drifts.
    #[test]
    fn exceeds_threshold_with_headroom_zero_headroom_matches_exceeds_threshold() {
        for cw in [0_u64, 1, 50, 100, 101, 1024, 100_000, 128_001, 1_000_001] {
            for pct in [0_u8, 1, 50, 85, 99, 100] {
                for used in [
                    0_u64,
                    1,
                    cw / 2,
                    cw.saturating_sub(1),
                    cw,
                    cw + 1,
                    cw + 1000,
                ] {
                    assert_eq!(
                        exceeds_threshold_with_headroom(used, cw, pct, 0),
                        exceeds_threshold(used, cw, pct),
                        "mismatch at used={used} cw={cw} pct={pct}",
                    );
                }
            }
        }
    }

    #[test]
    fn exceeds_threshold_with_headroom_subtracts_headroom() {
        // 100K window, 85% threshold = 85_000. Headroom 4_000 -> fires at 81_000.
        assert!(!exceeds_threshold_with_headroom(80_999, 100_000, 85, 4_000));
        assert!(exceeds_threshold_with_headroom(81_000, 100_000, 85, 4_000));
    }

    #[test]
    fn exceeds_threshold_with_headroom_zero_window() {
        assert!(!exceeds_threshold_with_headroom(0, 0, 85, 0));
        assert!(!exceeds_threshold_with_headroom(100, 0, 85, 4_000));
    }

    #[test]
    fn exceeds_threshold_with_headroom_headroom_larger_than_threshold_saturates() {
        // 100K * 85% = 85_000 (8_500_000 scaled). Headroom 1M tokens scales to
        // 100_000_000 — saturating sub yields 0, so any used fires.
        assert!(exceeds_threshold_with_headroom(0, 100_000, 85, 1_000_000));
    }
}
