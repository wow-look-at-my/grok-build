//! Every harness model slot, resolved once when the session actor is built.
//!
//! A consumer asks for its slot by id and gets the model the user chose, or
//! `None` when the slot inherits the session model. Resolution reads the
//! environment and `config.toml`, so it happens at build time and not on the
//! turn path.
//!
//! See `xai_grok_models::slots` for the slot table itself.

use std::collections::HashMap;

/// The model each slot resolved to. A slot that inherits the session model
/// is absent from the map.
#[derive(Debug, Clone, Default)]
pub struct ResolvedHarnessModels {
    by_slot: HashMap<&'static str, String>,
}

impl ResolvedHarnessModels {
    /// Resolve every slot in [`xai_grok_models::HARNESS_MODEL_SLOTS`].
    pub(crate) fn resolve(config: &crate::agent::config::Config) -> Self {
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
        Self { by_slot }
    }

    /// The model for one slot, or `None` when it inherits the session model.
    pub fn get(&self, slot_id: &str) -> Option<&str> {
        self.by_slot.get(slot_id).map(String::as_str)
    }

    /// Build a map directly. Tests use this to pin one slot.
    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&'static str, &str)]) -> Self {
        Self {
            by_slot: pairs.iter().map(|(k, v)| (*k, (*v).to_string())).collect(),
        }
    }
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
        assert_eq!(models.get("compaction"), Some("some-model"));
        assert_eq!(models.get("recap"), None);
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
                    resolved.get(slot.id),
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
