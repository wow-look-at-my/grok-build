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

/// Run a one-shot auxiliary call under [`aux_retry_policy`]. The first retry
/// moves to HTTP/1.1, as the main turn's sampler does
/// (`RetryDecision::RetryWithClientRebuild`). Without that, every retry goes
/// out on the same bad HTTP/2 connection, and the side call fails while the
/// main turn recovers.
pub(crate) async fn collect_aux_call(
    client: &xai_grok_sampler::SamplingClient,
    base: &ConversationRequest,
    label: &str,
    mut on_retry: impl FnMut(&SamplingError, std::time::Duration),
) -> Result<xai_grok_sampling_types::ConversationResponse, SamplingError> {
    use backon::BackoffBuilder as _;
    let mut client = client.clone();
    let mut backoff = aux_retry_policy().build();
    let mut on_http1 = false;
    loop {
        let err = match client.conversation_collect(fresh_req_id(base, label)).await {
            Ok(response) => return Ok(response),
            Err(err) => err,
        };
        if !should_retry_aux_call(&err) {
            return Err(err);
        }
        let Some(delay) = backoff.next() else {
            return Err(err);
        };
        on_retry(&err, delay);
        tokio::time::sleep(delay).await;
        if on_http1 {
            continue;
        }
        on_http1 = true;
        match client.with_http1() {
            Ok(http1) => client = http1,
            Err(e) => tracing::warn!(
                error = %e,
                call = label,
                "side call: no HTTP/1.1 client for the retry; the retry stays on HTTP/2"
            ),
        }
    }
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
    /// `slot` is the harness model slot this call belongs to. A slot the user
    /// set brings its OWN sampler, not just its model id: the backend, the
    /// context window and the credentials belong to the model the slot names,
    /// and writing that id onto the session's client sends one model's id to
    /// another model's endpoint. That costs the shared prompt-cache prefix,
    /// which is the point of the alignment here — a user who pins the slot has
    /// asked for the other model and pays for the cache miss.
    ///
    /// An unset slot, or one the session cannot reach, keeps the session's own
    /// client, so nothing changes until a slot is pinned.
    pub(crate) async fn prepare_side_call(&self, slot: &str) -> Result<SideCallSetup, acp::Error> {
        // One config read serves the window, model, and reasoning effort.
        let sampling_config = self.chat_state_handle.get_sampling_config().await;
        let reasoning_effort = sampling_config.as_ref().and_then(|c| c.reasoning_effort);
        if let Some((client, cfg)) = self.resolve_slot_sampler(slot).await {
            return Ok(SideCallSetup {
                strip_reasoning: client.api_backend().requires_reasoning_strip(),
                context_window: cfg.context_window,
                model: cfg.model.clone(),
                client,
                reasoning_effort,
            });
        }
        let client = self.prepare_chat_completion(false).await?;
        let strip_reasoning = client.api_backend().requires_reasoning_strip();
        let context_window = sampling_config
            .as_ref()
            .map(|c| c.context_window.get())
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
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

    /// Serve HTTP/1.1 keep-alive. Every request on the FIRST connection gets a
    /// 503, like a pooled connection that has gone bad. Any other connection
    /// gets a one-chunk completion. Returns the base URL and the number of
    /// requests the first connection took.
    async fn spawn_bad_first_connection_server()
    -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let on_bad = Arc::new(AtomicUsize::new(0));
        let on_bad_server = Arc::clone(&on_bad);
        tokio::spawn(async move {
            let mut accepted = 0usize;
            while let Ok((mut sock, _)) = listener.accept().await {
                accepted += 1;
                let bad = accepted == 1;
                let on_bad = Arc::clone(&on_bad_server);
                tokio::spawn(async move {
                    let mut buf: Vec<u8> = Vec::new();
                    loop {
                        let head_end = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break i + 4;
                            }
                            let mut chunk = [0u8; 4096];
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                        let body_len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        while buf.len() < head_end + body_len {
                            let mut chunk = [0u8; 4096];
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        buf.drain(..head_end + body_len);
                        let response = if bad {
                            on_bad.fetch_add(1, Ordering::SeqCst);
                            "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}".to_string()
                        } else {
                            let chunk = serde_json::json!({
                                "id": "chatcmpl-test",
                                "object": "chat.completion.chunk",
                                "created": 0,
                                "model": "test-model",
                                "choices": [{
                                    "index": 0,
                                    "delta": { "role": "assistant", "content": "ok" },
                                    "finish_reason": "stop"
                                }]
                            });
                            let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                            format!(
                                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
                                body.len()
                            )
                        };
                        if sock.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (base_url, on_bad)
    }

    /// The main turn leaves a bad pooled connection by moving to HTTP/1.1 on
    /// its first retry. A side call must do the same. If it retries on the
    /// pooled client, every attempt lands on the bad connection and `/todo`
    /// fails while the main turn works.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_aux_call_leaves_a_bad_pooled_connection_on_its_first_retry() {
        use std::sync::atomic::Ordering;

        let (base_url, on_bad) = spawn_bad_first_connection_server().await;
        let client = xai_grok_sampler::SamplingClient::new(xai_grok_sampler::SamplerConfig {
            api_key: Some("test-key".into()),
            base_url,
            model: "test-model".into(),
            ..Default::default()
        })
        .unwrap();
        let base = ConversationRequest::from_items(vec![ConversationItem::user("hi")]);

        let mut retries = 0usize;
        let response = collect_aux_call(&client, &base, "test", |_, _| retries += 1)
            .await
            .expect("the retry must reach a working connection");

        assert_eq!(response.assistant_text(), "ok");
        assert_eq!(retries, 1, "one failure, then one retry that succeeds");
        assert_eq!(
            on_bad.load(Ordering::SeqCst),
            1,
            "the retry must not go back to the bad connection"
        );
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
