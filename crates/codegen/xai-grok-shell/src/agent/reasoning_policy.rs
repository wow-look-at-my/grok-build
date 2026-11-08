//! Resolving the reasoning effort a call carries, best-effort.

use xai_grok_sampling_types::{ReasoningEffort, ReasoningEffortOption};

/// What a caller wants from the model's reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasoningNeed {
    /// As little reasoning as the model can be asked for.
    Off,
    /// The model's ordinary level, for a session that chose none.
    Default,
}

/// The effort a model that publishes no tiers is asked for by default.
const UNLISTED_DEFAULT_EFFORT: ReasoningEffort = ReasoningEffort::Medium;

/// The effort tiers the catalog publishes for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffortSupport {
    /// The catalog published the tiers this model accepts.
    Menu(Vec<ReasoningEffortOption>),
    /// Nothing was published, so no tier is known to be rejected.
    Unlisted,
}

/// The lowest enabled tier, in ascending order, for a menu that offers no
/// `none`. Falls back to the menu's own order for one that lists no known tier.
const ENABLED_LADDER: [ReasoningEffort; 5] = [
    ReasoningEffort::Minimal,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Xhigh,
];

/// The effort an unlisted model is asked for.
const UNLISTED_MODEL_EFFORT: ReasoningEffort = ReasoningEffort::Low;

/// The lowest enabled tier in `menu`.
fn lowest_enabled(menu: &[ReasoningEffortOption]) -> Option<ReasoningEffort> {
    ENABLED_LADDER
        .iter()
        .copied()
        .find(|tier| menu.iter().any(|option| option.value == *tier))
        .or_else(|| {
            menu.iter()
                .map(|option| option.value)
                .find(|effort| !matches!(effort, ReasoningEffort::None))
        })
}

/// The effort a call for `need` carries on `support`. The menu offers `none`:
/// send `none`. Reasoning is off. The menu offers an enabled tier but no
/// `none`: send the lowest one. No tier is published: send
/// [`UNLISTED_MODEL_EFFORT`]. Every rung names a tier. Omitting the field is
/// not on the ladder: an absent effort is what leaves a provider's own default
/// thinking running.
pub(crate) fn resolve_reasoning_effort(
    need: ReasoningNeed,
    support: &EffortSupport,
) -> ReasoningEffort {
    match (need, support) {
        (ReasoningNeed::Off, EffortSupport::Menu(menu)) => {
            if menu
                .iter()
                .any(|option| option.value == ReasoningEffort::None)
            {
                return ReasoningEffort::None;
            }
            lowest_enabled(menu).unwrap_or(UNLISTED_MODEL_EFFORT)
        }
        (ReasoningNeed::Off, EffortSupport::Unlisted) => UNLISTED_MODEL_EFFORT,
        // The default never turns reasoning off: only an explicit `none` does.
        (ReasoningNeed::Default, EffortSupport::Menu(menu)) => {
            xai_grok_sampling_types::default_reasoning_effort(menu)
                .unwrap_or(UNLISTED_DEFAULT_EFFORT)
        }
        (ReasoningNeed::Default, EffortSupport::Unlisted) => UNLISTED_DEFAULT_EFFORT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(value: ReasoningEffort) -> ReasoningEffortOption {
        ReasoningEffortOption {
            id: value.as_ref().to_string(),
            value,
            label: value.as_ref().to_string(),
            description: None,
            default: false,
        }
    }

    fn menu(values: &[ReasoningEffort]) -> EffortSupport {
        EffortSupport::Menu(values.iter().copied().map(option).collect())
    }

    #[test]
    fn a_menu_offering_none_sends_none() {
        assert_eq!(
            resolve_reasoning_effort(
                ReasoningNeed::Off,
                &menu(&[
                    ReasoningEffort::None,
                    ReasoningEffort::Minimal,
                    ReasoningEffort::High
                ])
            ),
            ReasoningEffort::None
        );
    }

    #[test]
    fn a_menu_without_none_sends_its_lowest_enabled_tier() {
        for (listed, expected) in [
            (vec![ReasoningEffort::Low], ReasoningEffort::Low),
            (
                vec![ReasoningEffort::High, ReasoningEffort::Minimal],
                ReasoningEffort::Minimal,
            ),
            (
                vec![ReasoningEffort::Medium, ReasoningEffort::Low],
                ReasoningEffort::Low,
            ),
            (
                vec![ReasoningEffort::Xhigh, ReasoningEffort::Max],
                ReasoningEffort::Xhigh,
            ),
        ] {
            assert_eq!(
                resolve_reasoning_effort(ReasoningNeed::Off, &menu(&listed)),
                expected,
                "menu {listed:?} must resolve to {expected:?}"
            );
        }
    }

    #[test]
    fn an_unlisted_model_still_gets_an_explicit_tier() {
        // The autodetected-gateway-alias case: the provider publishes
        // nothing, so the entry's support flag reads false without meaning
        // anything.
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Off, &EffortSupport::Unlisted),
            ReasoningEffort::Low
        );
    }

    #[test]
    fn a_menu_listing_only_none_sends_none() {
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Off, &menu(&[ReasoningEffort::None])),
            ReasoningEffort::None
        );
    }

    #[test]
    fn a_menu_of_only_unknown_tiers_still_names_one() {
        // Nothing on the ladder matches, so the menu's own order decides.
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Off, &menu(&[ReasoningEffort::Max])),
            ReasoningEffort::Max
        );
    }

    #[test]
    fn no_rung_resolves_to_an_omitted_effort() {
        for support in [
            EffortSupport::Unlisted,
            menu(&[ReasoningEffort::None]),
            menu(&[ReasoningEffort::Low]),
            menu(&[ReasoningEffort::Max]),
        ] {
            let resolved = resolve_reasoning_effort(ReasoningNeed::Off, &support);
            assert_ne!(
                Option::<ReasoningEffort>::None,
                Some(resolved),
                "{support:?} must name a tier"
            );
        }
    }

    #[test]
    fn the_default_is_the_marked_option_then_medium_then_the_nearest_tier() {
        let mut marked = vec![
            option(ReasoningEffort::None),
            option(ReasoningEffort::Low),
            option(ReasoningEffort::High),
        ];
        marked[2].default = true;
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Default, &EffortSupport::Menu(marked)),
            ReasoningEffort::High
        );
        for (listed, expected) in [
            (
                vec![
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                ],
                ReasoningEffort::Medium,
            ),
            (
                vec![
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                ],
                ReasoningEffort::High,
            ),
            (
                vec![ReasoningEffort::None, ReasoningEffort::Low],
                ReasoningEffort::Low,
            ),
        ] {
            assert_eq!(
                resolve_reasoning_effort(ReasoningNeed::Default, &menu(&listed)),
                expected,
                "menu {listed:?}"
            );
        }
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Default, &EffortSupport::Unlisted),
            ReasoningEffort::Medium
        );
    }

    /// Only an explicit `none` turns reasoning off. A menu that marks `none` as
    /// its default still gets an enabled tier by default.
    #[test]
    fn the_default_never_turns_reasoning_off() {
        let mut marked_none = vec![option(ReasoningEffort::None), option(ReasoningEffort::Low)];
        marked_none[0].default = true;
        assert_eq!(
            resolve_reasoning_effort(ReasoningNeed::Default, &EffortSupport::Menu(marked_none)),
            ReasoningEffort::Low
        );
    }
}
