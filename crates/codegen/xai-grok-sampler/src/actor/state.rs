//! Actor-internal state.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use xai_grok_sampling_types::{ConversationRequest, ImageStripReason, ToolSchemaForm};

use crate::config::{RetryPolicy, SamplerConfig};
use crate::types::RequestId;

/// Models observed to reject image input outright, shared between the actor and its per-request tasks.
#[derive(Clone, Default)]
pub(crate) struct ImageInputRejections(Arc<Mutex<HashSet<String>>>);

impl ImageInputRejections {
    /// The set, taken back from a holder that died holding it.
    #[allow(clippy::disallowed_methods)] // takes the set back as the doc above says
    fn rejections(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn mark(&self, model: &str) {
        self.rejections().insert(model.to_owned());
    }

    pub(crate) fn contains(&self, model: &str) -> bool {
        self.rejections().contains(model)
    }

    /// Strip images up front when `model` is known to reject them.
    pub(crate) fn strip_if_rejected(
        &self,
        model: &str,
        request: &mut ConversationRequest,
    ) -> usize {
        if !self.contains(model) {
            return 0;
        }
        let stripped = request
            .strip_images(ImageStripReason::ModelLacksVision)
            .len();
        if stripped > 0 {
            tracing::warn!(
                model = %model,
                stripped,
                "stripped {stripped} image(s): model rejected image input earlier"
            );
        }
        stripped
    }
}

/// Models observed to reject a top-level `oneOf`/`anyOf`/`allOf` in a tool schema.
#[derive(Clone, Default)]
pub(crate) struct ToolSchemaRejections(Arc<Mutex<HashSet<String>>>);

impl ToolSchemaRejections {
    #[allow(clippy::disallowed_methods)] // as `ImageInputRejections::rejections`
    fn rejections(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn mark(&self, model: &str) {
        self.rejections().insert(model.to_owned());
    }

    /// Start `request` in the fallback form when `model` rejected the native one.
    pub(crate) fn apply(&self, model: &str, request: &mut ConversationRequest) {
        if self.rejections().contains(model) {
            request.tool_schema_form = ToolSchemaForm::NoTopLevelCombinators;
        }
    }
}

/// What each model has rejected, shared by the actor and its request tasks.
#[derive(Clone, Default)]
pub(crate) struct ModelRejections {
    pub(crate) images: ImageInputRejections,
    pub(crate) tool_schemas: ToolSchemaRejections,
}

/// `cancel_token` is owned by the actor (cloned into the spawned per-request task).
/// The completion oneshot is moved into the per-request task at spawn time and is therefore not stored here.
pub(crate) struct ActiveRequest {
    pub(crate) cancel_token: CancellationToken,
}

pub(crate) struct ActorState {
    pub(crate) active_requests: HashMap<RequestId, ActiveRequest>,
    pub(crate) config: SamplerConfig,
    pub(crate) retry_policy: RetryPolicy,
    pub(crate) rejections: ModelRejections,
}

impl ActorState {
    pub(crate) fn new(config: SamplerConfig, retry_policy: RetryPolicy) -> Self {
        Self {
            active_requests: HashMap::new(),
            config,
            retry_policy,
            rejections: ModelRejections::default(),
        }
    }

    /// Returns the previous entry if the same `request_id` was already in flight (callers should cancel the previous token before overwriting).
    pub(crate) fn register(
        &mut self,
        request_id: RequestId,
        active: ActiveRequest,
    ) -> Option<ActiveRequest> {
        self.active_requests.insert(request_id, active)
    }

    /// Remove a request from the active set without cancelling its token.
    /// The actor calls this when a per-request task exits normally.
    pub(crate) fn remove(&mut self, request_id: &RequestId) -> Option<ActiveRequest> {
        self.active_requests.remove(request_id)
    }

    /// Cancel and remove an in-flight request.
    pub(crate) fn cancel(&mut self, request_id: &RequestId) -> bool {
        if let Some(active) = self.active_requests.remove(request_id) {
            active.cancel_token.cancel();
            true
        } else {
            false
        }
    }

    /// Replace the default config.
    /// The next request submitted without an override will use this.
    pub(crate) fn update_config(&mut self, config: SamplerConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SamplerConfig {
        SamplerConfig {
            base_url: "https://example.test".into(),
            model: "test-model".into(),
            context_window: 8192,
            ..Default::default()
        }
    }

    #[test]
    fn cancel_unknown_request_returns_false() {
        let mut state = ActorState::new(cfg(), RetryPolicy::default());
        assert!(!state.cancel(&RequestId::from("unknown")));
    }

    fn request_with_image() -> ConversationRequest {
        use xai_grok_sampling_types::{ContentPart, ConversationItem};
        ConversationRequest {
            items: vec![ConversationItem::user_with_parts(vec![
                ContentPart::Image {
                    url: std::sync::Arc::<str>::from("data:image/png;base64,AAAA"),
                },
            ])],
            model: Some("no-vision".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn images_survive_until_the_model_is_marked() {
        let rejections = ImageInputRejections::default();
        let mut request = request_with_image();
        assert_eq!(rejections.strip_if_rejected("no-vision", &mut request), 0);

        rejections.mark("no-vision");
        assert_eq!(rejections.strip_if_rejected("no-vision", &mut request), 1);
    }

    /// The mark is per model: switching to a vision model must send the images.
    #[test]
    fn marking_one_model_does_not_strip_for_another() {
        let rejections = ImageInputRejections::default();
        rejections.mark("no-vision");
        let mut request = request_with_image();
        assert_eq!(rejections.strip_if_rejected("has-vision", &mut request), 0);
    }

    #[test]
    fn a_marked_model_starts_in_the_fallback_schema_form() {
        let rejections = ToolSchemaRejections::default();
        let mut request = ConversationRequest::default();
        rejections.apply("strict", &mut request);
        assert_eq!(request.tool_schema_form, ToolSchemaForm::Native);

        rejections.mark("strict");
        rejections.apply("other", &mut request);
        assert_eq!(request.tool_schema_form, ToolSchemaForm::Native);
        rejections.apply("strict", &mut request);
        assert_eq!(
            request.tool_schema_form,
            ToolSchemaForm::NoTopLevelCombinators
        );
    }

    #[test]
    fn register_then_cancel_removes() {
        let mut state = ActorState::new(cfg(), RetryPolicy::default());
        let id = RequestId::from("req-1");
        state.register(
            id.clone(),
            ActiveRequest {
                cancel_token: CancellationToken::new(),
            },
        );
        assert_eq!(state.active_requests.len(), 1);
        assert!(state.cancel(&id));
        assert_eq!(state.active_requests.len(), 0);
    }

    #[test]
    fn register_returns_previous_when_same_id() {
        let mut state = ActorState::new(cfg(), RetryPolicy::default());
        let id = RequestId::from("req-1");
        let first = ActiveRequest {
            cancel_token: CancellationToken::new(),
        };
        let second = ActiveRequest {
            cancel_token: CancellationToken::new(),
        };
        assert!(state.register(id.clone(), first).is_none());
        assert!(state.register(id.clone(), second).is_some());
    }
}
