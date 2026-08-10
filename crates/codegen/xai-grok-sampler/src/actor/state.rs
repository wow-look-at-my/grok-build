//! Actor-internal state.
//!
//! The actor's command-loop serialization gives us a "single-threaded with
//! shared state" discipline matching the hunk-tracker pattern, so fields
//! touched only from the actor task need no synchronization.
//! [`ImageInputRejections`] is the exception: per-request tasks write it, so it
//! carries its own lock.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use xai_grok_sampling_types::{ConversationRequest, ImageStripReason};

use crate::config::{RetryPolicy, SamplerConfig};
use crate::types::RequestId;

/// Models observed to reject image input outright, shared between the actor
/// and its per-request tasks (the tasks are what see the rejection).
///
/// Without this, only the failing request recovers: the images stay in
/// conversation history, so every later turn re-uploads them and eats another
/// rejection before stripping again.
#[derive(Clone, Default)]
pub(crate) struct ImageInputRejections(Arc<Mutex<HashSet<String>>>);

impl ImageInputRejections {
    pub(crate) fn mark(&self, model: &str) {
        self.0
            .lock()
            .expect("image-input rejection set poisoned")
            .insert(model.to_owned());
    }

    pub(crate) fn contains(&self, model: &str) -> bool {
        self.0
            .lock()
            .expect("image-input rejection set poisoned")
            .contains(model)
    }

    /// Strip images up front when `model` is known to reject them. Returns how
    /// many were stripped (0 when the model is fine, or carries no images).
    pub(crate) fn strip_if_rejected(
        &self,
        model: &str,
        request: &mut ConversationRequest,
    ) -> usize {
        if !self.contains(model) {
            return 0;
        }
        let stripped = request.strip_images(ImageStripReason::ModelLacksVision);
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

/// In-flight request bookkeeping.
///
/// `cancel_token` is owned by the actor (cloned into the spawned
/// per-request task). The completion oneshot is moved into the
/// per-request task at spawn time and is therefore not stored here.
pub(crate) struct ActiveRequest {
    pub(crate) cancel_token: CancellationToken,
}

/// Actor-owned state.
pub(crate) struct ActorState {
    pub(crate) active_requests: HashMap<RequestId, ActiveRequest>,
    pub(crate) config: SamplerConfig,
    pub(crate) retry_policy: RetryPolicy,
    pub(crate) image_input_rejections: ImageInputRejections,
}

impl ActorState {
    pub(crate) fn new(config: SamplerConfig, retry_policy: RetryPolicy) -> Self {
        Self {
            active_requests: HashMap::new(),
            config,
            retry_policy,
            image_input_rejections: ImageInputRejections::default(),
        }
    }

    /// Register a newly-spawned request. Returns the previous entry if
    /// the same `request_id` was already in flight (callers should
    /// cancel the previous token before overwriting).
    pub(crate) fn register(
        &mut self,
        request_id: RequestId,
        active: ActiveRequest,
    ) -> Option<ActiveRequest> {
        self.active_requests.insert(request_id, active)
    }

    /// Remove a request from the active set without cancelling its
    /// token. Used by the cleanup signal sent from per-request tasks
    /// when they exit normally.
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

    /// Replace the default config. The next request submitted without
    /// an override will use this.
    pub(crate) fn update_config(&mut self, config: SamplerConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ApiBackend;
    use indexmap::IndexMap;

    /// Minimal config builder for tests in this module.
    fn cfg() -> SamplerConfig {
        SamplerConfig {
            api_key: None,
            base_url: "https://example.test".into(),
            model: "test-model".into(),
            max_completion_tokens: None,
            temperature: None,
            top_p: None,
            api_backend: ApiBackend::ChatCompletions,
            auth_scheme: Default::default(),
            extra_headers: IndexMap::new(),
            query_params: IndexMap::new(),
            env_http_headers: IndexMap::new(),
            context_window: 8192,
            force_http1: false,
            max_retries: None,
            stream_tool_calls: false,
            idle_timeout_secs: None,
            reasoning_effort: None,
            origin_client: None,
            client_identifier: None,
            deployment_id: None,
            user_id: None,
            client_version: None,
            attribution_callback: None,
            bearer_resolver: None,
            supports_backend_search: false,
            compactions_remaining: None,
            compaction_at_tokens: None,
            doom_loop_recovery: None,
            header_injector: None,
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
