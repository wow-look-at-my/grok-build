//! ASAP interjection: a mid-turn user message buffered while a tool is
//! running reaches the model on the *next* request in the same turn — between
//! AI messages, after the tool call — without waiting for stream idle and
//! without being deferred to its own turn.
//!
//! Drives the real turn loop (`handle_prompt`) against a scripted model with
//! a flag-gated blocking tool call, so the interjection is buffered in the
//! window between the tool's request and its completion — the exact path the
//! pager's "queued follow-up delivered mid-turn" feature relies on.

use super::support::*;
use super::*;
use std::sync::Arc;
use std::time::Duration;
use xai_grok_test_support::sse::{
    responses_api_reasoning_then_tool_call_events, responses_api_script_exact,
};
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse};

/// Distinctive text the user "sends" mid-turn; the test asserts it reaches
/// the model on the second request, inside the interjection envelope.
const INTERJECTION_NEEDLE: &str = "ASAP_STEER_PROBE";

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

/// User-message blobs in a Responses-API request body, as JSON strings.
fn user_message_blobs(body: &serde_json::Value) -> Vec<String> {
    body["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].to_string())
        .collect()
}

/// Build an actor wired to the mock server with the `run_terminal_command`
/// (bash) tool registered, so the scripted model can call a flag-gated
/// command that blocks until the test releases it.
async fn actor_with_mock_sampler(
    server: &MockInferenceServer,
    persistence_tx: tokio::sync::mpsc::UnboundedSender<PersistenceMsg>,
    gateway_tx: tokio::sync::mpsc::UnboundedSender<xai_acp_lib::AcpClientMessage>,
) -> Arc<SessionActor> {
    use xai_grok_tools::implementations::grok_build::BashTool;
    use xai_grok_tools::implementations::grok_build::KillTaskTool;
    use xai_grok_tools::implementations::grok_build::TaskOutputTool;
    use xai_grok_tools::registry::types::ToolConfig;

    // The bash tool's background support requires the task-output and kill-task
    // tools to be co-registered so background tasks can be observed/cancelled.
    let bash_cfg = ToolConfig::from(&BashTool)
        .with_name("run_terminal_command")
        .with_param_rename("is_background", "background");
    let task_output_cfg = ToolConfig::for_tool::<TaskOutputTool>()
        .with_name("get_command_or_subagent_output");
    let kill_task_cfg = ToolConfig::for_tool::<KillTaskTool>()
        .with_name("kill_command_or_subagent");
    let tools = vec![bash_cfg, task_output_cfg, kill_task_cfg];

    let sampling_cfg = xai_grok_sampler::SamplerConfig {
        api_key: Some("test-key".to_string()),
        base_url: server.url(),
        model: "test".to_string(),
        api_backend: xai_grok_sampler::ApiBackend::Responses,
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

    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.sampler_handle = sampler_handle;
    *actor.agent.borrow_mut() = test_agent_with_tools(tools).await;

    let mut cfg = actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .expect("test actor has sampling config");
    cfg.base_url = server.url();
    cfg.api_backend = xai_grok_sampling_types::ApiBackend::Responses;
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
    {
        let drainer = actor.clone();
        let mut sampler_event_rx = sampler_event_rx;
        tokio::task::spawn_local(async move {
            while let Some(event) = sampler_event_rx.recv().await {
                drainer.handle_sampling_event(event).await;
            }
        });
    }
    actor
}

/// A mid-turn interjection buffered while a tool call is in flight reaches
/// the model on the turn's *next* request — between AI messages, after the
/// tool result — rather than waiting for the turn to end.
#[tokio::test]
async fn interjection_buffered_during_tool_call_reaches_next_request() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start().await.expect("mock inference server");

            // A flag-gated foreground command: blocks until the test writes the
            // release flag, giving us the mid-turn window to buffer the
            // interjection after the first model request but before the second.
            let tmp = std::env::temp_dir().join(format!(
                "asap-inject-{}",
                uuid::Uuid::now_v7().simple()
            ));
            std::fs::create_dir_all(&tmp).unwrap();
            let release_flag = tmp.join("release.flag");
            let gated_cmd = format!(
                "while [ ! -e {} ]; do /bin/sleep 0.05; done",
                release_flag.display()
            );
            let tool_args = serde_json::json!({
                "command": gated_cmd,
                "description": "flag-gated hold for asap-injection test",
            })
            .to_string();

            // Request 1: model calls the flag-gated command (blocks).
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_reasoning_then_tool_call_events(
                    "thinking",
                    "call-asap-1",
                    "run_terminal_command",
                    &tool_args,
                    "test",
                )),
            );
            // Request 2: model finishes with a text answer.
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_script_exact("done after steer", "test")),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let actor = actor_with_mock_sampler(&server, persistence_tx, gateway_tx).await;

            // Drive the turn on a background local task so we can buffer the
            // interjection while the flag-gated tool is still running.
            let turn_actor = actor.clone();
            let turn_task = tokio::task::spawn_local(async move {
                let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                    "run the gated command".to_string(),
                ))];
                tokio::time::timeout(
                    Duration::from_secs(120),
                    turn_actor.handle_prompt(
                        "asap-inject",
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
                .expect("turn must finish within timeout")
            });

            // Wait until the first model request (the tool call) has been sent,
            // so we know the turn is inside the tool and the interjection
            // buffer will be drained on the *next* loop iteration, not this one.
            let waited = tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if !server.request_bodies().is_empty() {
                        return;
                    }
                    tokio::task::yield_now().await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(
                waited.is_ok(),
                "first model request (tool call) must be sent within timeout"
            );

            // Give the tool a beat to actually start running the gated command.
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                server.request_bodies().len(),
                1,
                "only the first request should be in flight while the tool blocks"
            );

            // Buffer the interjection mid-turn — this is the "asap injection":
            // a user message arriving while a tool is running.
            actor.pending_interjections.push(PendingInterjection {
                text: INTERJECTION_NEEDLE.to_string(),
                attachments: vec![],
            });

            // Release the flag so the tool completes and the turn loop iterates
            // (draining the interjection before the second model request).
            std::fs::write(&release_flag, b"released").unwrap();

            let outcome = turn_task.await.expect("turn task panicked");
            assert!(outcome.is_ok(), "turn must not error: {outcome:?}");

            let bodies = server.request_bodies();
            assert!(
                bodies.len() >= 2,
                "expected at least two model requests (tool call then text); got {}",
                bodies.len()
            );

            // The interjection must reach the model on the SECOND request, as a
            // user message carrying the steering text.
            let second = &bodies[1];
            let user_blobs = user_message_blobs(second);
            assert!(
                user_blobs
                    .iter()
                    .any(|b| b.contains(INTERJECTION_NEEDLE)),
                "second request must carry the interjection as a user message; \
                 user blobs: {user_blobs:?}"
            );

            // And it must NOT have been on the first request (it was buffered
            // after the tool started).
            let first = &bodies[0];
            let first_user = user_message_blobs(first);
            assert!(
                !first_user.iter().any(|b| b.contains(INTERJECTION_NEEDLE)),
                "interjection must not appear on the first request; user blobs: {first_user:?}"
            );

            // The conversation must contain the interjection as a standalone
            // synthetic user message tagged Interjection.
            let conv = actor.chat_state_handle.get_conversation().await;
            let has_interjection = conv.iter().any(|item| {
                matches!(item, ConversationItem::User(u)
                    if u.synthetic_reason == Some(SyntheticReason::Interjection))
                    && item.text_content().contains(INTERJECTION_NEEDLE)
            });
            assert!(
                has_interjection,
                "conversation must contain the interjection as a synthetic user message; conv: {conv:#?}"
            );

            // Cleanup: the temp dir.
            let _ = std::fs::remove_dir_all(&tmp);
        })
        .await;
}

/// An interjection buffered while the model is *streaming* (no tool call in
/// flight — the turn loop is blocked on `submit_and_collect`) must still reach
/// the model on the next request in the same turn, not wait for a separate
/// prompt turn or get silently dropped.
///
/// This is the "don't wait for stream idle" case: the stream holds open at its
/// terminal event via `expect_response_blocked`, the interjection is buffered
/// mid-stream, then the stream is released. The turn loop iterates, drains the
/// interjection, and the second request carries it.
#[tokio::test]
async fn interjection_buffered_during_stream_reaches_next_request() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            use xai_grok_test_support::InferenceEndpoint;
            use xai_grok_test_support::InferenceRequestMatcher;

            let server = MockInferenceServer::start().await.expect("mock inference server");

            // Request 1: a pure-text stream that holds open at its terminal
            // event so we can buffer the interjection mid-stream.
            let mut first =
                server.expect_response_blocked(
                    "first-stream",
                    InferenceRequestMatcher::foreground(InferenceEndpoint::Responses),
                    ScriptedResponse::sse(responses_api_script_exact(
                        "streaming a long answer here",
                        "test",
                    )),
                );
            // Request 2: the turn's follow-up request after the interjection is
            // drained — a short text completion.
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_script_exact("done after steer", "test")),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let actor = actor_with_mock_sampler(&server, persistence_tx, gateway_tx).await;

            // Drive the turn on a background local task.
            let turn_actor = actor.clone();
            let turn_task = tokio::task::spawn_local(async move {
                let prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                    "give me a long answer".to_string(),
                ))];
                tokio::time::timeout(
                    Duration::from_secs(120),
                    turn_actor.handle_prompt(
                        "asap-stream-inject",
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
                .expect("turn must finish within timeout")
            });

            // Wait until the first stream is actively streaming and parked at
            // its terminal-event barrier — the model is mid-generation.
            first.wait_blocked().await;

            // Buffer the interjection mid-stream — the user "sends" a message
            // while the model is still generating.
            actor.pending_interjections.push(PendingInterjection {
                text: INTERJECTION_NEEDLE.to_string(),
                attachments: vec![],
            });

            // Release the stream so it completes; the turn loop then iterates,
            // drains the interjection, and fires the second request.
            first.release();

            let outcome = turn_task.await.expect("turn task panicked");
            assert!(outcome.is_ok(), "turn must not error: {outcome:?}");

            let bodies = server.request_bodies();
            assert!(
                bodies.len() >= 2,
                "expected at least two model requests (stream then follow-up); got {}",
                bodies.len()
            );

            // The interjection must reach the model on the SECOND request.
            let second = &bodies[1];
            let user_blobs = user_message_blobs(second);
            assert!(
                user_blobs
                    .iter()
                    .any(|b| b.contains(INTERJECTION_NEEDLE)),
                "second request must carry the interjection as a user message; \
                 user blobs: {user_blobs:?}"
            );

            // And it must NOT have been on the first request (it was buffered
            // mid-stream, after the first request was already sent).
            let first_req = &bodies[0];
            let first_user = user_message_blobs(first_req);
            assert!(
                !first_user.iter().any(|b| b.contains(INTERJECTION_NEEDLE)),
                "interjection must not appear on the first request; user blobs: {first_user:?}"
            );

            // The conversation must contain the interjection as a standalone
            // synthetic user message tagged Interjection.
            let conv = actor.chat_state_handle.get_conversation().await;
            let has_interjection = conv.iter().any(|item| {
                matches!(item, ConversationItem::User(u)
                    if u.synthetic_reason == Some(SyntheticReason::Interjection))
                    && item.text_content().contains(INTERJECTION_NEEDLE)
            });
            assert!(
                has_interjection,
                "conversation must contain the interjection as a synthetic user message; conv: {conv:#?}"
            );
        })
        .await;
}

/// A follow-up *queued* mid-turn (the prompt-queue path, not a direct
/// interjection) is harvested into the running turn and reaches the model on
/// the next request — the "claude code style" asap delivery the harvest
/// feature (commit `2c973e5`) provides. The follow-up must NOT wait for the
/// whole turn to end and run as its own turn.
///
/// This drives the REAL turn-promotion path (`maybe_start_running_task`), which
/// sets `running_task` and `queued_at_turn_start` — the state the harvest
/// consults. Direct `handle_prompt` calls bypass the promoter and leave that
/// state empty, so the harvest would no-op; this test goes through the promoter
/// so it exercises the exact path production takes.
#[tokio::test]
async fn queued_followup_harvested_into_running_turn_reaches_next_request() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start().await.expect("mock inference server");

            let tmp = std::env::temp_dir().join(format!(
                "asap-harvest-{}",
                uuid::Uuid::now_v7().simple()
            ));
            std::fs::create_dir_all(&tmp).unwrap();
            let release_flag = tmp.join("release.flag");
            let gated_cmd = format!(
                "while [ ! -e {} ]; do /bin/sleep 0.05; done",
                release_flag.display()
            );
            let tool_args = serde_json::json!({
                "command": gated_cmd,
                "description": "flag-gated hold for harvest test",
            })
            .to_string();

            // Request 1: model calls the flag-gated command (blocks).
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_reasoning_then_tool_call_events(
                    "thinking",
                    "call-harvest-1",
                    "run_terminal_command",
                    &tool_args,
                    "test",
                )),
            );
            // Request 2: model finishes with a text answer.
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_script_exact("done after harvest", "test")),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let actor = actor_with_mock_sampler(&server, persistence_tx, gateway_tx).await;

            const HARVEST_NEEDLE: &str = "HARVEST_STEER_PROBE";

            // Queue the initial prompt — the real path promotes this and spawns
            // the turn (setting running_task + queued_at_turn_start).
            let initial = user_item("asap-harvest", "test-owner");
            {
                let mut state = actor.state.lock().await;
                state.pending_inputs.push_back(initial);
            }

            let (completion_tx, mut completion_rx) =
                tokio::sync::mpsc::unbounded_channel::<(String, PromptTurnResult)>();
            actor.clone().maybe_start_running_task(completion_tx).await;

            // Wait until the first model request (the tool call) has been sent.
            let waited = tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    if !server.request_bodies().is_empty() {
                        return;
                    }
                    tokio::task::yield_now().await;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(waited.is_ok(), "first model request must be sent");

            // Give the tool a beat to start running.
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Queue a follow-up mid-turn — the prompt-queue path. The harvest
            // (called at the top of the next loop iteration, loop_index > 1)
            // must pick this up and deliver it as an interjection on the next
            // model request.
            let mut followup = user_item("harvest-followup", "test-owner");
            if let Some(meta) = followup.queue_meta.as_mut() {
                meta.text = HARVEST_NEEDLE.to_string();
            }
            followup.prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                HARVEST_NEEDLE.to_string(),
            ))];
            {
                let mut state = actor.state.lock().await;
                state.pending_inputs.push_back(followup);
            }

            // Release the flag so the tool completes and the turn loop iterates.
            std::fs::write(&release_flag, b"released").unwrap();

            // Wait for the turn to complete.
            let _completion = tokio::time::timeout(Duration::from_secs(120), async {
                completion_rx.recv().await
            })
            .await
            .expect("turn must complete within timeout");

            let bodies = server.request_bodies();
            assert!(
                bodies.len() >= 2,
                "expected at least two model requests; got {}",
                bodies.len()
            );

            // The harvested follow-up must reach the model on the SECOND
            // request, as a user message carrying the steering text.
            let second = &bodies[1];
            let user_blobs = user_message_blobs(second);
            assert!(
                user_blobs.iter().any(|b| b.contains(HARVEST_NEEDLE)),
                "second request must carry the harvested follow-up as a user message; \
                 user blobs: {user_blobs:?}"
            );

            // And it must NOT have been on the first request.
            let first = &bodies[0];
            let first_user = user_message_blobs(first);
            assert!(
                !first_user.iter().any(|b| b.contains(HARVEST_NEEDLE)),
                "harvested follow-up must not appear on the first request; user blobs: {first_user:?}"
            );

            // The conversation must contain it as a synthetic user message
            // tagged Interjection.
            let conv = actor.chat_state_handle.get_conversation().await;
            let has_interjection = conv.iter().any(|item| {
                matches!(item, ConversationItem::User(u)
                    if u.synthetic_reason == Some(SyntheticReason::Interjection))
                    && item.text_content().contains(HARVEST_NEEDLE)
            });
            assert!(
                has_interjection,
                "conversation must contain the harvested follow-up as a synthetic user message; conv: {conv:#?}"
            );

            let _ = std::fs::remove_dir_all(&tmp);
        })
        .await;
}

/// ASAP injection during a model stream: when an interjection arrives while
/// the model is actively streaming (the turn loop is blocked on
/// `submit_and_collect`), the in-flight stream is CANCELLED so the turn loop
/// iterates immediately, drains the interjection, and resubmits — instead of
/// waiting for the (potentially many-minutes-long) stream to finish. The
/// partial text the model had already streamed is preserved as an assistant
/// message so the resubmitted request sees `partial + interjection`.
///
/// This is the "don't wait for stream idle" path: it mirrors what the
/// `SessionCommand::Interject` handler does (buffer + cancel the in-flight
/// request), driven here directly so the test does not need the full
/// `run_session` actor loop.
#[tokio::test]
async fn interjection_during_stream_cancels_and_resubmits_with_partial_preserved() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            use xai_grok_test_support::InferenceEndpoint;
            use xai_grok_test_support::InferenceRequestMatcher;

            let server = MockInferenceServer::start().await.expect("mock inference server");

            // Request 1: a stream that emits some text then holds open at its
            // terminal event — the model is mid-generation with partial text.
            let mut first =
                server.expect_response_blocked(
                    "first-stream",
                    InferenceRequestMatcher::foreground(InferenceEndpoint::Responses),
                    ScriptedResponse::sse(responses_api_script_exact(
                        "partial answer so far",
                        "test",
                    )),
                );
            // Request 2: the resubmitted request after the cancel+drain.
            server.enqueue_response(
                "/v1/responses",
                ScriptedResponse::sse(responses_api_script_exact("final answer", "test")),
            );

            let (gateway_tx, gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            drain_gateway(gateway_rx);
            let (persistence_tx, persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            drain_persistence(persistence_rx);

            let actor = actor_with_mock_sampler(&server, persistence_tx, gateway_tx).await;

            // Queue the initial prompt and promote it (the real path that sets
            // running_task + in_flight_sampler_request_id).
            let initial = user_item("asap-cancel", "test-owner");
            {
                let mut state = actor.state.lock().await;
                state.pending_inputs.push_back(initial);
            }
            let (completion_tx, mut completion_rx) =
                tokio::sync::mpsc::unbounded_channel::<(String, PromptTurnResult)>();
            actor.clone().maybe_start_running_task(completion_tx).await;

            // Wait until the first stream is parked at its terminal-event
            // barrier — the model is mid-stream with partial text streamed.
            first.wait_blocked().await;

            // Give the sampler-event drainer task a beat to process the text
            // deltas that arrived before the terminal barrier (they update
            // `streaming_turn_capture`, which the cancel path preserves).
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Simulate the `SessionCommand::Interject` handler: buffer the
            // interjection, set the cancel flag, and cancel the in-flight
            // request id. (In production this is one `SessionCommand::Interject`
            // dispatch; here we reproduce its effect directly.)
            actor.pending_interjections.push(PendingInterjection {
                text: INTERJECTION_NEEDLE.to_string(),
                attachments: vec![],
            });
            let in_flight = actor
                .in_flight_sampler_request_id
                .lock()
                .clone()
                .expect("in-flight request id must be set while streaming");
            actor
                .interjection_cancel_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            actor.sampler_handle.cancel(in_flight);

            // Wait for the turn to complete (cancel → drain → resubmit → done).
            let _completion = tokio::time::timeout(Duration::from_secs(120), async {
                completion_rx.recv().await
            })
            .await
            .expect("turn must complete within timeout");

            let bodies = server.request_bodies();
            assert!(
                bodies.len() >= 2,
                "expected at least two model requests (cancelled stream then resubmit); got {}",
                bodies.len()
            );

            // The interjection must reach the model on the SECOND (resubmit)
            // request, as a user message.
            let second = &bodies[1];
            let user_blobs = user_message_blobs(second);
            assert!(
                user_blobs.iter().any(|b| b.contains(INTERJECTION_NEEDLE)),
                "resubmitted request must carry the interjection as a user message; \
                 user blobs: {user_blobs:?}"
            );

            // The partial assistant text must be preserved in the conversation
            // as an assistant message (committed before the resubmit), so the
            // model sees `partial + interjection` and the streamed text is not
            // silently lost.
            let conv = actor.chat_state_handle.get_conversation().await;
            let has_partial = conv.iter().any(|item| {
                matches!(item, ConversationItem::Assistant(a) if a.content.as_ref() == "partial answer so far")
            });
            assert!(
                has_partial,
                "conversation must preserve the partial assistant text; conv: {conv:#?}"
            );

            // And the interjection must be present as a synthetic user message.
            let has_interjection = conv.iter().any(|item| {
                matches!(item, ConversationItem::User(u)
                    if u.synthetic_reason == Some(SyntheticReason::Interjection))
                    && item.text_content().contains(INTERJECTION_NEEDLE)
            });
            assert!(
                has_interjection,
                "conversation must contain the interjection as a synthetic user message; conv: {conv:#?}"
            );

            // The in-flight id must be cleared after the turn (no leak).
            assert!(
                actor.in_flight_sampler_request_id.lock().is_none(),
                "in-flight request id must be cleared after the turn"
            );
        })
        .await;
}
