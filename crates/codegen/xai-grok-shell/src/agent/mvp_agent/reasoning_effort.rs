//! Applies a reasoning-effort hint only when the model supports it; shared by session creation, model switch, and the summary client.

use agent_client_protocol as acp;
use xai_grok_sampler::SamplerConfig;
use xai_grok_sampling_types::ReasoningEffort;

use crate::agent::reasoning_policy::{EffortSupport, ReasoningNeed, resolve_reasoning_effort};
use crate::agent::remote_config::ModelsManager;
use crate::sampling::EffortTarget;

impl ModelsManager {
    /// The effort tiers the catalog publishes for `model_id`, for
    /// [`resolve_reasoning_effort`]. Only a published menu counts.
    pub(crate) fn effort_support(&self, model_id: &str) -> EffortSupport {
        let menu = self.model_reasoning_efforts(model_id);
        if menu.is_empty() {
            return EffortSupport::Unlisted;
        }
        EffortSupport::Menu(menu)
    }

    /// The effort a call for `need` carries on `model_id`.
    pub(crate) fn reasoning_effort_for(
        &self,
        need: ReasoningNeed,
        model_id: &str,
    ) -> ReasoningEffort {
        resolve_reasoning_effort(need, &self.effort_support(model_id))
    }

    /// The effort a config for `model_id` carries when nothing chose one: the
    /// catalog's default for the model, else [`ReasoningNeed::Default`].
    /// `None` only for a model that takes no effort. Reasoning goes off only
    /// through an explicit `none`.
    pub(crate) fn default_effort_for(&self, model_id: &str) -> Option<ReasoningEffort> {
        if !self.model_supports_reasoning_effort(model_id) {
            return None;
        }
        Some(
            self.model_default_reasoning_effort(model_id)
                .unwrap_or_else(|| self.reasoning_effort_for(ReasoningNeed::Default, model_id)),
        )
    }

    /// Apply `effort` to `sampling`. With no `effort`, a config that carries
    /// none takes [`Self::default_effort_for`], so a model that takes an
    /// effort never goes out without one.
    pub(crate) fn apply_supported_effort(
        &self,
        sampling: &mut SamplerConfig,
        effort: Option<ReasoningEffort>,
        session_id: &acp::SessionId,
        target: EffortTarget,
    ) {
        let effort = match effort {
            Some(effort) => effort,
            None if sampling.reasoning_effort.is_some() => return,
            None => match self.default_effort_for(&sampling.model) {
                Some(effort) => effort,
                None => return,
            },
        };
        if !self.model_supports_reasoning_effort(&sampling.model) {
            // SummaryClient stays quiet; the spawn or switch that carried this effort already warned that the model does not support it
            if matches!(target, EffortTarget::NewSession | EffortTarget::ModelSwitch) {
                tracing::warn!(
                    session_id = %session_id.0,
                    model = %sampling.model,
                    effort = %effort,
                    "reasoning_effort: model does not support effort; ignoring it"
                );
            }
            return;
        }
        // Some models are a different model id at each effort, so swap in the id this effort asks for.
        // Do this before the log, or the log records an id we are not sending.
        if let Some(routed) = self.model_for_effort(&sampling.model, effort) {
            sampling.model = routed;
        }
        // Same fields at every target; only the level differs
        // tracing bakes the level into a static callsite, so match a const level per arm
        macro_rules! log_applied {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    session_id = %session_id.0,
                    model = %sampling.model,
                    effort = %effort,
                    target = %target.as_ref(),
                    "reasoning_effort: applied effort"
                )
            };
        }
        match target {
            EffortTarget::NewSession | EffortTarget::ModelSwitch => {
                log_applied!(tracing::Level::INFO)
            }
            EffortTarget::SummaryClient => log_applied!(tracing::Level::DEBUG),
        }
        sampling.reasoning_effort = Some(effort);
    }
}

/// At most one variant carries the hint, so the spawn and switch consumers can never both fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewSessionEffort {
    /// Seed the spawned session's sampling config (default-model path).
    Spawn(ReasoningEffort),
    /// Apply after spawn through the model switch (explicit `modelId` path).
    Switch(ReasoningEffort),
    None,
}

/// Precedence: an explicit `_meta.reasoningEffort` wins over the process-wide last-used or `[models].default_reasoning_effort` value.
/// The catalog default is the last resort and is left on the sampling config when this returns `None`.
pub(crate) fn resolve_new_session_effort_hint(
    meta_hint: Option<ReasoningEffort>,
    current: Option<ReasoningEffort>,
) -> Option<ReasoningEffort> {
    meta_hint.or(current)
}

pub(crate) fn split_new_session_effort(
    resolved_custom_model: Option<&str>,
    hint: Option<ReasoningEffort>,
) -> NewSessionEffort {
    match hint {
        None => NewSessionEffort::None,
        Some(effort) if resolved_custom_model.is_some() => NewSessionEffort::Switch(effort),
        Some(effort) => NewSessionEffort::Spawn(effort),
    }
}
