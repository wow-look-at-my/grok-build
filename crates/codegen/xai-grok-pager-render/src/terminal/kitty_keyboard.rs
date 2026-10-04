//! Which Kitty keyboard enhancement flags the pager negotiates at startup.

use std::sync::atomic::{AtomicU8, Ordering};

use crossterm::event::KeyboardEnhancementFlags;

/// Highest packed library version that emits a duplicate legacy release for Backspace/Tab/Enter/Escape.
pub const ALACRITTY_BROKEN_EVENT_TYPES_MAX_PACKED: u32 = 2401;

/// Empty means push nothing. Unknown version never downgrades — DA2 is often skipped, and only a positively identified broken version pays the cost.
/// No brand check: [`super::da2`]'s gate admits Alacritty alone, so widening that gate widens this one.
pub fn negotiated_kitty_flags(
    skip_reason: Option<&str>,
    da2_packed: Option<u32>,
) -> KeyboardEnhancementFlags {
    if skip_reason.is_some() {
        return KeyboardEnhancementFlags::empty();
    }
    let mut flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    let mis_encodes_releases =
        da2_packed.is_some_and(|packed| packed <= ALACRITTY_BROKEN_EVENT_TYPES_MAX_PACKED);
    if !mis_encodes_releases {
        flags |= KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
    }
    flags
}

/// Bits pushed (`0` is empty), not a classification, so the predicates cannot drift.
static PUSHED_KITTY_FLAGS: AtomicU8 = AtomicU8::new(0);

/// The exact flag set `init_terminal` pushed; the suspend/resume path re-pushes this verbatim so both can never drift.
pub fn pushed_kitty_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::from_bits_truncate(PUSHED_KITTY_FLAGS.load(Ordering::Relaxed))
}

pub fn set_pushed_kitty_flags(flags: KeyboardEnhancementFlags) {
    PUSHED_KITTY_FLAGS.store(flags.bits(), Ordering::Relaxed);
}

/// True only if flags were pushed. False means modified keys arrive as legacy
/// bytes.
pub fn kitty_flags_pushed() -> bool {
    !pushed_kitty_flags().is_empty()
}

/// Whether the terminal reports key *release* events.
pub fn kitty_releases_reported() -> bool {
    pushed_kitty_flags().contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
}

/// Whether the version workaround engaged: pushed, but without
/// `REPORT_EVENT_TYPES`.
pub fn kitty_event_types_withheld() -> bool {
    let flags = pushed_kitty_flags();
    !flags.is_empty() && !flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
}

/// Clears the record as it reads, so concurrent teardown paths cannot both pop.
pub fn take_kitty_flags_pushed() -> bool {
    PUSHED_KITTY_FLAGS.swap(0, Ordering::Relaxed) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISAMBIGUATE: KeyboardEnhancementFlags =
        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    const EVENT_TYPES: KeyboardEnhancementFlags = KeyboardEnhancementFlags::REPORT_EVENT_TYPES;

    #[test]
    fn downgrade_boundary_is_the_last_broken_library_version() {
        // Non-empty either side: a downgrade is still a push, teardown owes a pop.
        assert_eq!(negotiated_kitty_flags(None, Some(2401)), DISAMBIGUATE);
        assert_eq!(
            negotiated_kitty_flags(None, Some(2402)),
            DISAMBIGUATE | EVENT_TYPES
        );
    }

    /// DA2 is skipped under multiplexers and off unix, so "no answer" is the common case and must not cost a healthy terminal its release events.
    #[test]
    fn absent_version_does_not_downgrade() {
        assert_eq!(
            negotiated_kitty_flags(None, None),
            DISAMBIGUATE | EVENT_TYPES
        );
    }

    /// A skip reason outranks any version, so teardown owes no pop.
    #[test]
    fn a_skip_reason_pushes_nothing() {
        for packed in [None, Some(2401), Some(2402)] {
            assert_eq!(
                negotiated_kitty_flags(Some("vscode"), packed),
                KeyboardEnhancementFlags::empty(),
                "da2_packed={packed:?}"
            );
        }
    }
}
