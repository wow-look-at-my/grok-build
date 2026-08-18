//! A response that hits the output token cap (`finish_reason: "length"` /
//! `stop_reason: "max_tokens"`) is a turn cut off mid-thought, not a
//! finished one. The turn loop must resubmit immediately instead of
//! reporting `TurnOutcome::Completed` on the truncated text.
//!
//! Drives the real turn loop against a scripted Chat Completions mock so a
//! regression that treats `StopReason::Length` as an ordinary stop is
//! caught here, not just in the model layer's own stop-reason mapping
//! tests.

use super::support::*;
use super::*;
use std::sync::Arc;
use std::time::Duration;
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse, SseEvent};

fn drain_gateway(mut rx: tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>) {
    tokio::task::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let xai_acp_lib::AcpClientMessage::SessionNotification(args) = msg {
                let _ = args.response_tx.send(Ok(()));
            }
        }
    });
}

fn drain_persistence(mut rx: tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>) {
    tokio::task::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let PersistenceMsg::FlushAndAck { respond_to } = msg {
                let _ = respond_to.send(Ok(()));
            }
        }
    });
}

/// A single Chat Completions SSE chunk carrying all of `content` in one
/// delta, terminated with the given `finish_reason` — `"length"` for a
/// truncated response, `"stop"` for a normal one.
fn chat_completion_response(text: &str, finish_reason: &str) -> ScriptedResponse {
    let chunk = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1234567890,
        "model": "test",
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": text },
            "finish_reason": finish_reason
        }]
    });
    let usage = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1234567890,
        "model": "test",
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11 }
    });
    ScriptedResponse::sse(vec![
        SseEvent::data(chunk.to_string()),
        SseEvent::data(usage.to_string()),
        SseEvent::data("[DONE]"),
    ])
}

/// `(actor, request-count fn)` wired against `server` over Chat Completions,
/// with `max_turns` bounding the resumption loop so a regression that never
/// stops continuing fails on the bound instead of hanging the test.
async fn length_truncation_actor(
    server: &MockInferenceServer,
    max_turns: Option<usize>,
) -> Arc<SessionActor> {
    let sampling_cfg = xai_grok_sampler::SamplerConfig {
        api_key: Some("test-key".to_string()),
        base_url: server.url(),
        model: "test".to_string(),
        api_backend: xai_grok_sampling_types::ApiBackend::ChatCompletions,
        context_window: 256_000,
        max_retries: Some(0),
        idle_timeout_secs: Some(30),
        ..Default::default()
    };
    let (sampler_event_tx, sampler_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_grok_sampler::SamplingEvent>();
    let sampler_handle = xai_grok_sampler::SamplerActor::spawn(
        sampling_cfg,
        xai_grok_sampler::RetryPolicy {
            max_retries: 0,
            rate_limit_retry_threshold: 0,
            ..Default::default()
        },
        sampler_event_tx,
    );

    let (gateway_tx, gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    drain_gateway(gateway_rx);
    let (persistence_tx, persistence_rx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    drain_persistence(persistence_rx);

    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.sampler_handle = sampler_handle;
    actor.max_turns = max_turns;

    let mut cfg = actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .expect("test actor has sampling config");
    cfg.base_url = server.url();
    cfg.api_backend = xai_grok_sampling_types::ApiBackend::ChatCompletions;
    cfg.model = "test".to_string();
    actor.chat_state_handle.update_sampling_config(cfg);
    let mut creds = actor.chat_state_handle.get_credentials().await;
    creds.api_key = Some("test-key".to_string());
    actor.chat_state_handle.update_credentials(creds);

    actor
        .workspace_ops
        .bind_local_session(
            &actor.session_id_string(),
            actor.tool_context.cwd.as_path().to_path_buf(),
            actor.tool_context.hunk_tracker_handle.clone(),
            actor.agent.borrow().tool_bridge().toolset(),
            None,
        )
        .expect("bind_local_session");

    let actor = Arc::new(actor);
    let drainer = actor.clone();
    let mut sampler_event_rx = sampler_event_rx;
    tokio::task::spawn_local(async move {
        while let Some(event) = sampler_event_rx.recv().await {
            drainer.handle_sampling_event(event).await;
        }
    });
    actor
}

fn completions_request_count(server: &MockInferenceServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|e| e.path == "/v1/chat/completions")
        .count()
}

/// Two length-truncated chunks followed by a normal stop: the turn must
/// resubmit twice on its own and converge to `EndTurn`/`Completed`, proving
/// a truncated response is never mistaken for a finished turn.
#[tokio::test(flavor = "current_thread")]
async fn length_truncated_response_resumes_and_completes() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start().await.expect("mock inference server");
            server.enqueue_response(
                "/v1/chat/completions",
                chat_completion_response("Part one of the answer, ", "length"),
            );
            server.enqueue_response(
                "/v1/chat/completions",
                chat_completion_response("part two, ", "length"),
            );
            server.enqueue_response(
                "/v1/chat/completions",
                chat_completion_response("and the finish.", "stop"),
            );

            let actor = length_truncation_actor(&server, Some(10)).await;

            let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                "answer at length".to_string(),
            ))];
            let outcome = tokio::time::timeout(
                Duration::from_secs(30),
                actor.handle_prompt(
                    "length-truncation-resume",
                    prompt_blocks,
                    PromptMode::Agent,
                    None,
                    None,
                    None,
                    None,
                    true,
                    None,
                    None,
                    None,
                ),
            )
            .await
            .expect("turn must finish within timeout");

            let ok = outcome.expect("truncated-then-complete turn must not error");
            assert_eq!(
                ok.stop_reason,
                acp::StopReason::EndTurn,
                "the turn must converge to a normal completion once the model stops truncating"
            );
            assert!(
                matches!(
                    ok.completion_kind,
                    crate::session::commands::PromptCompletionKind::Completed
                ),
                "expected Completed, got {:?}",
                ok.completion_kind
            );
            assert_eq!(
                completions_request_count(&server),
                3,
                "must have resubmitted after each length-truncated chunk: request log:\n{}",
                server.request_log_summary()
            );

            let conv = actor.chat_state_handle.get_conversation().await;
            let assistant_text: String = conv
                .iter()
                .filter(|item| matches!(item, ConversationItem::Assistant(_)))
                .map(|item| item.text_content())
                .collect::<Vec<_>>()
                .join("");
            assert!(
                assistant_text.contains("Part one") && assistant_text.contains("finish"),
                "both truncated chunks must land in history, got: {assistant_text:?}"
            );
        })
        .await;
}

/// A run of `finish_reason: "length"` responses that never stops truncating
/// must not loop forever: it is bounded by the same turn counter a tool
/// round uses, and reports `MaxTurnsReached` once the bound is hit.
#[tokio::test(flavor = "current_thread")]
async fn unbroken_length_truncation_is_bounded_by_max_turns() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start().await.expect("mock inference server");
            for _ in 0..10 {
                server.enqueue_response(
                    "/v1/chat/completions",
                    chat_completion_response("still going, ", "length"),
                );
            }

            let actor = length_truncation_actor(&server, Some(3)).await;

            let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                "answer at length".to_string(),
            ))];
            let outcome = tokio::time::timeout(
                Duration::from_secs(30),
                actor.handle_prompt(
                    "length-truncation-bounded",
                    prompt_blocks,
                    PromptMode::Agent,
                    None,
                    None,
                    None,
                    None,
                    true,
                    None,
                    None,
                    None,
                ),
            )
            .await
            .expect("turn must finish within timeout — an unbounded loop would hang here");

            let ok = outcome.expect("hitting the max-turns bound must not error the turn");
            assert!(
                matches!(
                    ok.completion_kind,
                    crate::session::commands::PromptCompletionKind::MaxTurnsReached { .. }
                ),
                "expected MaxTurnsReached, got {:?}",
                ok.completion_kind
            );
            assert_eq!(
                completions_request_count(&server),
                3,
                "must stop resubmitting at the max_turns bound: request log:\n{}",
                server.request_log_summary()
            );
        })
        .await;
}
