//! Shared cache-aligned side-call plumbing for recap-style auxiliary model
//! calls (recap, turn summary). `/btw` and `/todo` reuse the request skeleton
//! and the transient-failure retry policy.

use super::*;

use crate::remote::DEFAULT_CONTEXT_WINDOW;

/// Retry policy for a one-shot auxiliary model call (`/btw`, `/todo`): 3
/// attempts total (1 try + 2 retries), 500ms → 1s jittered backoff.
/// Deliberately short — nothing like the sampler actor's budget — so a
/// fleet-wide capacity event can't multiply side-call traffic into a retry
/// storm.
pub(crate) fn aux_retry_policy() -> backon::ExponentialBuilder {
    backon::ExponentialBuilder::default()
        .with_max_times(2)
        .with_min_delay(std::time::Duration::from_millis(500))
        .with_max_delay(std::time::Duration::from_secs(1))
        .with_jitter()
}

/// Retry transient failures per the canonical [`SamplingError::is_retryable`]
/// rule (5xx incl. Cloudflare 52x, stream/connect glitches), minus the shared
/// vetoes (`x-should-retry: false`, context length) and rate limits — a 429
/// needs `Retry-After`-scale waits, not this sub-second budget.
pub(crate) fn should_retry_aux_call(e: &xai_grok_sampling_types::SamplingError) -> bool {
    e.is_retryable() && !e.is_rate_limited() && !e.is_retry_vetoed()
}

/// Clone an auxiliary request and stamp a fresh `req_id`, so retried attempts
/// never collide in logs. Everything else is byte-identical, which is what
/// keeps a retry on the same cached prefix.
pub(crate) fn fresh_req_id(base: &ConversationRequest, label: &str) -> ConversationRequest {
    let mut request = base.clone();
    request.x_grok_req_id = Some(format!("xai-{label}-{}", uuid::Uuid::new_v4()));
    request
}

/// Cache numbers for an auxiliary call. `cache_key_forwarded` separates backends that never send the key from real cache misses.
pub(crate) fn log_prompt_cache_hit(
    call: &str,
    backend: crate::sampling::ApiBackend,
    response: &xai_grok_sampling_types::ConversationResponse,
) {
    let Some(usage) = response.usage.as_ref() else {
        return;
    };
    tracing::info!(
        call,
        cached_prompt_tokens = usage.cached_prompt_tokens,
        prompt_tokens = usage.prompt_tokens,
        cache_key_forwarded = backend.forwards_prompt_cache_key(),
        "auxiliary call prompt cache"
    );
}

/// What differs between the two calls that ride the parent's prompt cache. The shared parts live in [`SessionActor::parent_cached_request`].
pub(crate) struct AuxCall {
    pub(crate) items: Vec<ConversationItem>,
    pub(crate) tools: Vec<ToolSpec>,
    pub(crate) hosted_tools: Vec<xai_grok_sampling_types::HostedTool>,
    pub(crate) model: String,
    /// Must match the main turn's, or the prompt differs before the conversation history even starts.
    pub(crate) reasoning_effort: Option<xai_grok_sampling_types::ReasoningEffort>,
    /// Says whether the cache key gets sent, which is what decides the conv id below.
    pub(crate) backend: crate::sampling::ApiBackend,
    pub(crate) conv_id: String,
    pub(crate) req_id: String,
}

/// Shared setup for a recap-style side-call; see
/// [`SessionActor::prepare_side_call`].
pub(crate) struct SideCallSetup {
    pub(crate) client: xai_grok_sampler::SamplingClient,
    pub(crate) strip_reasoning: bool,
    pub(crate) context_window: u64,
    pub(crate) model: String,
    /// Must match the main turn so the side-call shares the prompt-cache prefix.
    pub(crate) reasoning_effort: Option<xai_grok_sampling_types::ReasoningEffort>,
}

impl SessionActor {
    /// Request skeleton for an auxiliary call that replays the parent conversation under the parent's `prompt_cache_key`.
    /// Temperature stays unset: cli-chat-proxy may inject a `thinking` config, and the Messages API then requires temperature == 1.
    pub(crate) fn parent_cached_request(&self, call: AuxCall) -> ConversationRequest {
        let session_id = self.session_info.id.to_string();
        // Only the Responses mapping sends the cache key. On the other backends the conv id is what ties a call to its conversation,
        // so it has to stay the parent session id; the `btw-`/`recap-` label still shows up in `x_grok_req_id`.
        let conv_id = if call.backend.forwards_prompt_cache_key() {
            call.conv_id
        } else {
            session_id.clone()
        };
        ConversationRequest {
            items: call.items,
            tools: call.tools,
            hosted_tools: call.hosted_tools,
            model: Some(call.model),
            temperature: None,
            // Effort changes the prompt ahead of the conversation history, so dropping it here would share no prefix with the main turn.
            reasoning_effort: call.reasoning_effort,
            x_grok_conv_id: Some(conv_id),
            x_grok_req_id: Some(call.req_id),
            x_grok_session_id: Some(session_id.clone()),
            x_grok_agent_id: Some(xai_grok_telemetry::id::agent_id()),
            prompt_cache_key: Some(session_id),
            ..Default::default()
        }
    }

    /// Prepare the shared pieces of a recap-style side-call (recap and turn
    /// summary): the sampling client plus the config both need.
    ///
    /// `strip_reasoning` is true ONLY on the Messages API backend (it rejects
    /// thinking blocks without a `thinking` config). Every other backend
    /// keeps reasoning verbatim so the prefix matches the last turn and the
    /// provider's prefix KV cache stays warm. Mirrors compaction's
    /// `summary_strips_reasoning`.
    pub(crate) async fn prepare_side_call(&self) -> Result<SideCallSetup, acp::Error> {
        let client = self.prepare_chat_completion(false).await?;
        let strip_reasoning = client.api_backend().requires_reasoning_strip();
        // One config read serves the window, model, and reasoning effort.
        let sampling_config = self.chat_state_handle.get_sampling_config().await;
        let context_window = sampling_config
            .as_ref()
            .map(|c| c.context_window.get())
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let reasoning_effort = sampling_config.as_ref().and_then(|c| c.reasoning_effort);
        let model = sampling_config.map(|c| c.model).unwrap_or_default();
        Ok(SideCallSetup {
            client,
            strip_reasoning,
            context_window,
            model,
            reasoning_effort,
        })
    }

    /// Build the cache-aligned request for a recap-style side-call via
    /// [`Self::parent_cached_request`]: main-turn tool + hosted-tool specs and
    /// matching reasoning effort so the prompt-cache prefix stays warm.
    ///
    /// Leaves BOTH temperature and max_output_tokens unset: the
    /// cli-chat-proxy layer may inject a `thinking` budget for
    /// thinking-enabled models (which also forces temperature == 1), and a
    /// small max_output_tokens below that budget makes the call error or
    /// return empty. The instructions keep outputs short and the clean
    /// helpers cap length as a safety net, so an explicit token cap isn't
    /// needed.
    pub(crate) async fn side_call_request(
        &self,
        setup: &SideCallSetup,
        items: Vec<ConversationItem>,
        x_grok_conv_id: String,
        x_grok_req_id: String,
    ) -> ConversationRequest {
        let tool_defs = self.prepare_tool_definitions().await;
        let tools = self.turn_base_tool_specs(&tool_defs);
        // Mirror the main turn's hosted tools (overrides folded in) so a
        // side-call can't search past the active cutoff.
        let hosted_tools = self.hosted_tools_for_turn();
        self.parent_cached_request(AuxCall {
            items,
            tools,
            hosted_tools,
            model: setup.model.clone(),
            reasoning_effort: setup.reasoning_effort,
            backend: setup.client.api_backend(),
            conv_id: x_grok_conv_id,
            req_id: x_grok_req_id,
        })
    }

    /// Invalidate in-flight recap-style side-calls when a real user prompt is
    /// accepted (queue time / turn start). Bumps the recap epoch so a finishing
    /// recap cannot commit, and aborts an in-flight turn summary. Both would
    /// describe a conversation this prompt is about to extend. Idempotent under
    /// the queue-accept + turn-start double bump. Keep this the single place
    /// that knows which side-calls to cancel on a new prompt.
    pub(crate) fn invalidate_side_calls_for_new_prompt(&self) {
        self.recap_epoch.set(self.recap_epoch.get().wrapping_add(1));
        self.abort_turn_summary();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::SamplingError;

    fn api(status: u16, message: &str, should_retry: Option<bool>) -> SamplingError {
        SamplingError::Api {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            message: message.into(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry,
        }
    }

    #[test]
    fn aux_calls_retry_transient_failures_only() {
        // Transient: overload (stream + proxy-wrapped 500 + 529), generic
        // 5xx, and Cloudflare edge 52x (SEV-576: /btw died on a 522).
        assert!(should_retry_aux_call(&SamplingError::StreamError {
            error_type: "overloaded_error".into(),
            message: "Overloaded".into(),
        }));
        assert!(should_retry_aux_call(&api(
            500,
            "stream error (overloaded_error): Overloaded",
            None
        )));
        for code in [503u16, 522, 529] {
            assert!(
                should_retry_aux_call(&api(code, "transient", None)),
                "{code} must retry"
            );
        }

        // Server veto (`x-should-retry: false`) wins over any retryable status.
        assert!(!should_retry_aux_call(&api(522, "timed out", Some(false))));
        // Deterministic context-length failures never retry, even on 529.
        assert!(!should_retry_aux_call(&api(
            529,
            "invalid_request_error: prompt is too long: 300000 tokens > 200000 maximum",
            None
        )));
        // Rate limits need Retry-After-scale waits, not this sub-second
        // budget; origin TLS and client errors never clear on retry.
        for code in [429u16, 525, 526, 400] {
            assert!(
                !should_retry_aux_call(&api(code, "not transient", None)),
                "{code} must NOT retry"
            );
        }
    }

    /// The wired policy: 3 attempts total, backoff within the configured
    /// bounds (500ms + 1s base, jitter adds up to the current delay), and a
    /// fresh request id stamped per attempt.
    #[tokio::test(start_paused = true)]
    async fn aux_retry_wiring_caps_attempts_and_bounds_backoff() {
        use backon::Retryable as _;

        let calls = std::cell::Cell::new(0u32);
        let start = tokio::time::Instant::now();
        let result: Result<(), SamplingError> = (|| async {
            calls.set(calls.get() + 1);
            Err(SamplingError::StreamError {
                error_type: "overloaded_error".into(),
                message: "Overloaded".into(),
            })
        })
        .retry(aux_retry_policy())
        .when(should_retry_aux_call)
        .await;

        assert!(result.is_err());
        assert_eq!(calls.get(), 3, "1 try + 2 retries");
        // Base delays 500ms + 1s; jitter adds (0, delay) per sleep.
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(1_500),
            "elapsed {elapsed:?} below minimum backoff"
        );
        assert!(
            elapsed <= std::time::Duration::from_millis(3_100),
            "elapsed {elapsed:?} above maximum backoff"
        );
    }

    /// Each attempt is byte-identical but for its request id, and the label
    /// keeps `/btw` and `/todo` attempts apart in logs.
    #[test]
    fn attempts_get_fresh_labelled_request_ids() {
        let base = ConversationRequest {
            x_grok_conv_id: Some("btw-test".into()),
            ..Default::default()
        };
        let a = fresh_req_id(&base, "btw");
        let b = fresh_req_id(&base, "btw");
        let (a_id, b_id) = (a.x_grok_req_id.unwrap(), b.x_grok_req_id.unwrap());
        assert!(a_id.starts_with("xai-btw-"));
        assert_ne!(a_id, b_id, "each attempt must get a fresh req_id");
        // Everything except the request id is byte-identical to the base.
        assert_eq!(a.x_grok_conv_id, base.x_grok_conv_id);
        assert!(
            fresh_req_id(&base, "todo")
                .x_grok_req_id
                .unwrap()
                .starts_with("xai-todo-")
        );
    }
}
