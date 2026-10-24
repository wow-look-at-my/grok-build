//! Every harness model slot, resolved when the session actor is built and
//! again on every `[models]` or `[model_groups]` change.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The count of `[models]` reloads in this process.
static CONFIG_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Mark every session's resolved slots stale. The `[models]` reload calls this.
pub(crate) fn mark_all_stale() {
    CONFIG_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The model each slot resolved to. A slot that inherits the session model is
/// absent from the map.
#[derive(Debug, Clone, Default)]
pub struct ResolvedHarnessModels {
    by_slot: RefCell<HashMap<&'static str, String>>,
    /// The generation this map was resolved at. `None` never goes stale, which keeps a test's pinned map in place.
    resolved_at: Cell<Option<u64>>,
}

impl ResolvedHarnessModels {
    /// Resolve every slot in [`xai_grok_models::HARNESS_MODEL_SLOTS`].
    pub(crate) fn resolve(config: &crate::agent::config::Config) -> Self {
        let resolved_at = CONFIG_GENERATION.load(Ordering::Relaxed);
        Self {
            by_slot: RefCell::new(resolve_map(config)),
            resolved_at: Cell::new(Some(resolved_at)),
        }
    }

    /// True one time after each `[models]` reload. The caller then resolves
    /// the slots again with [`Self::reload`].
    pub(crate) fn take_stale(&self) -> bool {
        let Some(resolved_at) = self.resolved_at.get() else {
            return false;
        };
        let now = CONFIG_GENERATION.load(Ordering::Relaxed);
        if resolved_at == now {
            return false;
        }
        self.resolved_at.set(Some(now));
        true
    }

    /// Resolve every slot again from `config` and replace the map.
    pub(crate) fn reload(&self, config: &crate::agent::config::Config) {
        let next = resolve_map(config);
        let mut current = self.by_slot.borrow_mut();
        if *current != next {
            tracing::info!(
                before = ?*current,
                after = ?next,
                "harness model slots reloaded"
            );
        }
        *current = next;
    }

    /// The model for one slot, or `None` when it inherits the session model.
    pub fn get(&self, slot_id: &str) -> Option<String> {
        self.by_slot.borrow().get(slot_id).cloned()
    }

    /// Build a map directly. Tests use this to pin one slot.
    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&'static str, &str)]) -> Self {
        Self {
            by_slot: RefCell::new(pairs.iter().map(|(k, v)| (*k, (*v).to_string())).collect()),
            resolved_at: Cell::new(None),
        }
    }
}

fn resolve_map(config: &crate::agent::config::Config) -> HashMap<&'static str, String> {
    let mut by_slot = HashMap::new();
    for slot in xai_grok_models::HARNESS_MODEL_SLOTS {
        if let Some(resolved) = config.resolve_harness_model(slot.id) {
            tracing::debug!(
                slot = slot.id,
                model = %resolved.value,
                source = %resolved.source,
                "harness model slot resolved"
            );
            by_slot.insert(slot.id, resolved.value);
        }
    }
    by_slot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_slot_reads_as_inherit() {
        let models = ResolvedHarnessModels::default();
        assert_eq!(models.get("compaction"), None);
    }

    #[test]
    fn a_pinned_slot_reads_back() {
        let models = ResolvedHarnessModels::from_pairs(&[("compaction", "some-model")]);
        assert_eq!(models.get("compaction").as_deref(), Some("some-model"));
        assert_eq!(models.get("recap"), None);
    }

    /// A map goes stale once per `[models]` reload, and a pinned test map
    /// never does.
    #[test]
    fn a_reload_marks_a_resolved_map_stale_once() {
        let resolved = ResolvedHarnessModels::resolve(&crate::agent::config::Config::default());
        let pinned = ResolvedHarnessModels::from_pairs(&[("compaction", "m")]);
        let _ = resolved.take_stale();
        mark_all_stale();
        assert!(resolved.take_stale());
        assert!(!pinned.take_stale());
    }

    /// A running session keeps this map for its whole life. A reload must
    /// change what the next read returns, or a settings change waits for a
    /// restart.
    #[test]
    fn reload_replaces_a_pin_on_the_same_map() {
        let models = ResolvedHarnessModels::from_pairs(&[("compaction", "old-model")]);
        let mut cfg = crate::agent::config::Config::default();
        cfg.models.compaction = Some("new-model".to_owned());
        models.reload(&cfg);
        assert_eq!(models.get("compaction").as_deref(), Some("new-model"));

        models.reload(&crate::agent::config::Config::default());
        assert_eq!(
            models.get("compaction").as_deref(),
            xai_grok_models::slot_by_id("compaction").and_then(|s| s.compiled_default()),
            "a removed pin falls back to the slot's default"
        );
    }

    /// Every slot whose fallback is a compiled default must resolve on a
    /// default config, and every inherit slot must stay absent. This is what
    /// makes "unset means inherit" true rather than assumed.
    #[test]
    fn default_config_resolves_exactly_the_compiled_slots() {
        let resolved = ResolvedHarnessModels::resolve(&crate::agent::config::Config::default());
        for slot in xai_grok_models::HARNESS_MODEL_SLOTS {
            match slot.compiled_default() {
                Some(expected) => assert_eq!(
                    resolved.get(slot.id).as_deref(),
                    Some(expected),
                    "{} must resolve to its compiled default",
                    slot.id
                ),
                None => assert_eq!(
                    resolved.get(slot.id),
                    None,
                    "{} must inherit the session model when nothing sets it",
                    slot.id
                ),
            }
        }
    }
}
