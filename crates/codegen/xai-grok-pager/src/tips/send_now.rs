//! Tip after queuing a follow-up while a turn is running: advertise that bare
//! Enter on an empty prompt interrupts the turn and hands the model everything
//! queued.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::EphemeralTip;
use crate::theme::Theme;

/// Ephemeral-tip dedup key for the queued-follow-up send-now hint.
pub(crate) const SEND_NOW_TIP_KEY: &str = "send_now_tip";

/// Key into the per-session in-memory seen-count map for this tip.
pub(crate) const SEND_NOW_TIP_SEEN_KEY: &str = "send_now_tip_shown_count";

/// Stop showing after this many shows within a single session.
const SEND_NOW_TIP_SEEN_CAP: u32 = 3;

/// Build "Queued · Enter to interrupt & send", seen-gated to
/// [`SEND_NOW_TIP_SEEN_CAP`] shows per session (in-memory).
///
/// The queued follow-up already reaches the model at the running turn's next
/// gap. This advertises the harder gesture: after a mid-turn queue the composer
/// is empty, so a second Enter cuts the model off mid-response and hands it the
/// queue immediately — no special chord to learn.
pub fn send_now_tip() -> EphemeralTip {
    let theme = Theme::current();
    let dim = Style::default().fg(theme.gray);
    let key_style = Style::default()
        .fg(theme.text_secondary)
        .add_modifier(Modifier::BOLD);
    EphemeralTip::new(
        SEND_NOW_TIP_KEY,
        Line::from(vec![
            Span::styled("Queued · ", dim),
            Span::styled("Enter", key_style),
            Span::styled(" to interrupt & send", dim),
        ]),
    )
    .with_session_seen_cap(SEND_NOW_TIP_SEEN_KEY, SEND_NOW_TIP_SEEN_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_now_tip_builder_applies_seen_gating() {
        assert_eq!(
            send_now_tip().session_seen.map(|(key, _cap)| key),
            Some(SEND_NOW_TIP_SEEN_KEY)
        );
        assert_eq!(
            send_now_tip().session_seen.map(|(_, cap)| cap),
            Some(SEND_NOW_TIP_SEEN_CAP)
        );
    }

    #[test]
    fn send_now_tip_advertises_enter() {
        let tip = send_now_tip();
        let text: String = tip.line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("Enter") && text.contains("interrupt & send") && text.contains("Queued"),
            "expected queued/interrupt copy with Enter, got {text:?}"
        );
    }
}
