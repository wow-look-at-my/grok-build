//! The summary call a thinking block triggers, driven end to end.
//!
//! The pager half (`scrollback::blocks::thinking`, and the pager's
//! `acp_handler::tests::thinking_summary`) draws whatever arrives here. What
//! these tests own is the producing half on the real path: a completed response
//! carrying reasoning must put one request on the wire, and the
//! update it broadcasts must carry the key the summary was written for and the
//! cleaned text the model answered with. The switch is exercised in both
//! positions, because a session that resolved it off must spend nothing.

use super::support::*;
use super::*;
use xai_grok_sampling_types::{ConversationItem, ConversationResponse, synthesized_reasoning_item};
use xai_grok_test_support::MockInferenceServer;

/// Reasoning long enough to cover the path a real turn takes.
fn long_reasoning() -> String {
    format!(
        "Read the parser first. {}",
        "Then weigh the lexer boundary against the offset the caller passes. ".repeat(30)
    )
}

fn response_carrying_thinking(thinking: &str) -> ConversationResponse {
    ConversationResponse {
        items: vec![
            ConversationItem::Reasoning(synthesized_reasoning_item(thinking.to_string())),
            ConversationItem::assistant("patched the offset"),
        ],
        stop_reason: None,
        usage: None,
        cost_usd_ticks: None,
        message_chunks_emitted: 1,
        doom_loop_signals: Vec::new(),
        stop_message: None,
        message_id: None,
        raw_stop_reason: None,
        stop_sequence: None,
    }
}

/// Point the actor's sampler at `server`, the same way the turn-summary test
/// does, so the side call is a real HTTP request rather than a stubbed client.
async fn aim_at(actor: &SessionActor, server: &MockInferenceServer) {
    let mut cfg = actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .expect("the test actor has a sampling config");
    cfg.base_url = server.url();
    cfg.api_backend = xai_grok_sampling_types::ApiBackend::Responses;
    actor.chat_state_handle.update_sampling_config(cfg);
}

/// Drain the gateway rail until a `thinking_summary` update arrives, returning
/// its raw payload. The side call runs on the session's LocalSet, so yielding
/// is what lets it progress in a test.
async fn take_thinking_summary(
    grx: &mut tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>,
) -> Option<serde_json::Value> {
    for _ in 0..4000 {
        while let Ok(msg) = grx.try_recv() {
            let xai_acp_lib::AcpClientMessage::ExtNotification(args) = msg else {
                continue;
            };
            if args.request.method.as_ref() != "x.ai/session_notification" {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(args.request.params.get())
            else {
                continue;
            };
            let update = value.get("update")?;
            if update.get("sessionUpdate").and_then(|v| v.as_str()) == Some("thinking_summary") {
                return Some(update.clone());
            }
        }
        tokio::task::yield_now().await;
    }
    None
}

#[tokio::test]
async fn a_long_thinking_block_is_summarized_keyed_to_its_own_call() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, mut prx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled = true;
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response(
                "Summary: \"Weighed the lexer's boundary, then \
                                 changed the caller's offset.\"",
            );
            aim_at(&actor, &server).await;

            let thinking = long_reasoning();
            actor.spawn_thinking_summary(&response_carrying_thinking(&thinking), Some(4_321));

            let update = take_thinking_summary(&mut grx)
                .await
                .expect("the summary must be broadcast");
            assert_eq!(
                update.get("sessionUpdate").and_then(|v| v.as_str()),
                Some("thinking_summary"),
                "the tag the pager's handler matches on"
            );
            assert_eq!(
                update.get("stream_start_ms").and_then(|v| v.as_i64()),
                Some(4_321),
                "the summary names the model call whose reasoning it describes"
            );
            let summary = update
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            assert!(
                summary.contains("boundary") && summary.contains("offset"),
                "the broadcast is the model's answer: {summary:?}"
            );
            assert!(
                !summary.starts_with("Summary:"),
                "the cleaner must strip the model's label: {summary:?}"
            );
            assert!(
                !summary.contains('"'),
                "the cleaner must strip the quotes: {summary:?}"
            );

            // The same update must reach persistence, or a reload loses it while
            // the live session still shows it.
            let mut persisted = false;
            while let Ok(msg) = prx.try_recv() {
                let PersistenceMsg::Update(crate::session::storage::SessionUpdate::Xai(notif)) =
                    msg
                else {
                    continue;
                };
                if matches!(
                    notif.update,
                    crate::extensions::notification::SessionUpdate::ThinkingSummary {
                        stream_start_ms: 4_321,
                        ..
                    }
                ) {
                    persisted = true;
                }
            }
            assert!(persisted, "the summary must be persisted for a reload");

            // The summary is exactly one model call, and that call carried the
            // reasoning it was asked to summarize. The paired off-position test
            // asserts this same counter at zero, so the count measured here is
            // what makes that zero mean something.
            let bodies = server.request_bodies();
            assert_eq!(
                bodies.len(),
                1,
                "one long thinking block is one summary call: {}",
                server.request_log_summary()
            );
            assert!(
                bodies[0].to_string().contains("Read the parser first."),
                "the request must carry the reasoning it is asked to summarize: {}",
                server.request_log_summary()
            );
        })
        .await;
}

#[tokio::test]
async fn short_thinking_is_summarized_too() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled = true;
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("Read parser first");
            aim_at(&actor, &server).await;

            actor.spawn_thinking_summary(
                &response_carrying_thinking("Read the parser first."),
                Some(5_000),
            );

            let update = take_thinking_summary(&mut grx)
                .await
                .expect("a collapsed short block still needs its summary");
            assert_eq!(
                update.get("stream_start_ms").and_then(|v| v.as_i64()),
                Some(5_000)
            );
            assert_eq!(
                server.request_count(),
                1,
                "one short thinking block is one summary call: {}",
                server.request_log_summary()
            );
        })
        .await;
}

#[tokio::test]
async fn a_session_resolved_with_the_switch_off_asks_nothing() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            // Production sets this once at spawn from `[ui].thinking_summaries`;
            // this is the off position of that same resolved field.
            actor.thinking_summaries_enabled = false;
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("must never be asked for");
            aim_at(&actor, &server).await;

            // Identical input to the on-position test above: only the switch
            // differs, so a skip attributed to anything else is excluded.
            let thinking = long_reasoning();
            actor.spawn_thinking_summary(&response_carrying_thinking(&thinking), Some(6_000));

            for _ in 0..200 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                server.request_count(),
                0,
                "a session with the switch off must issue no summary request"
            );
        })
        .await;
}

/// A second thinking block's summary call carries the first summary as the
/// direction block, and the first call carries none. Driven through the real
/// side call, so the assertion is on the bytes the model was sent.
#[tokio::test]
async fn a_second_summary_carries_the_first_as_its_direction_block() {
    use crate::session::helpers::thinking_summary::ThinkingSummaryHistory;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled = true;
            // Two seconds between calls, so the default window covers both.
            actor.thinking_summary_history = ThinkingSummaryHistory::new(120, 5);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("Offset bug is in the caller");
            aim_at(&actor, &server).await;

            actor.spawn_thinking_summary(
                &response_carrying_thinking("First: read the parser offset."),
                Some(1_000),
            );
            let first = take_thinking_summary(&mut grx)
                .await
                .expect("the first summary must be broadcast");
            assert_eq!(
                first.get("summary").and_then(|v| v.as_str()),
                Some("Offset bug is in the caller")
            );

            actor.spawn_thinking_summary(
                &response_carrying_thinking("Second: weigh the retry budget."),
                Some(2_000),
            );
            let second = take_thinking_summary(&mut grx)
                .await
                .expect("the second summary must be broadcast");
            assert_eq!(
                second.get("summary").and_then(|v| v.as_str()),
                Some("Offset bug is in the caller")
            );

            let bodies = server.request_bodies();
            assert_eq!(bodies.len(), 2, "one summary call per response");
            let first_body = bodies[0].to_string();
            assert!(
                !first_body.contains("<previous_summaries>"),
                "the first summary of a session has no prior block: {first_body}"
            );
            let second_body = bodies[1].to_string();
            assert!(
                second_body.contains("<previous_summaries>"),
                "the second summary must be given the prior block: {second_body}"
            );
            assert!(
                second_body.contains("- Offset bug is in the caller"),
                "the prior block must carry the first summary's text: {second_body}"
            );
            assert!(
                second_body.contains("Second: weigh the retry budget."),
                "the current reasoning must still be the reasoning block: {second_body}"
            );
        })
        .await;
}

/// Past the window, with the count floor at zero, no prior summary reaches the
/// next call. The same two calls under a count floor do carry it, so the window
/// is what excluded it.
#[tokio::test]
async fn a_summary_past_the_window_is_left_out_and_the_count_floor_carries_it() {
    use crate::session::helpers::thinking_summary::ThinkingSummaryHistory;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            for (window_secs, min_count, expect_prior) in [(2u32, 0u32, false), (0, 1, true)] {
                let (gateway_tx, mut grx) =
                    tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
                let (persistence_tx, _prx) =
                    tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
                let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
                actor.thinking_summaries_enabled = true;
                actor.thinking_summary_history =
                    ThinkingSummaryHistory::new(window_secs, min_count);
                let actor = std::sync::Arc::new(actor);

                let server = MockInferenceServer::start().await.unwrap();
                server.set_response("Reuse the retry helper");
                aim_at(&actor, &server).await;

                // Nine seconds apart: outside a 2 s window, inside a 120 s one.
                actor.spawn_thinking_summary(
                    &response_carrying_thinking("First pass at the retry loop."),
                    Some(1_000),
                );
                take_thinking_summary(&mut grx)
                    .await
                    .expect("the first summary must be broadcast");
                actor.spawn_thinking_summary(
                    &response_carrying_thinking("Second pass at the retry loop."),
                    Some(10_000),
                );
                take_thinking_summary(&mut grx)
                    .await
                    .expect("the second summary must be broadcast");

                let bodies = server.request_bodies();
                assert_eq!(bodies.len(), 2, "one summary call per response");
                let second_body = bodies[1].to_string();
                assert_eq!(
                    second_body.contains("<previous_summaries>"),
                    expect_prior,
                    "window={window_secs}s count_floor={min_count}: {second_body}"
                );
            }
        })
        .await;
}

/// Both halves at zero leave the call with no history at all, so a session can
/// turn the block off without turning the summaries off.
#[tokio::test]
async fn zero_window_and_zero_count_leave_the_summary_call_stateless() {
    use crate::session::helpers::thinking_summary::ThinkingSummaryHistory;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled = true;
            actor.thinking_summary_history = ThinkingSummaryHistory::new(0, 0);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("Env var overrides config file");
            aim_at(&actor, &server).await;

            actor.spawn_thinking_summary(
                &response_carrying_thinking("Check precedence of the config value."),
                Some(1_000),
            );
            take_thinking_summary(&mut grx)
                .await
                .expect("the first summary must be broadcast");
            actor.spawn_thinking_summary(
                &response_carrying_thinking("Check precedence again."),
                Some(2_000),
            );
            take_thinking_summary(&mut grx)
                .await
                .expect("the second summary must be broadcast");

            let bodies = server.request_bodies();
            assert_eq!(bodies.len(), 2, "one summary call per response");
            assert!(
                !bodies[1].to_string().contains("<previous_summaries>"),
                "zero window and zero count must not attach a prior block: {bodies:?}"
            );
        })
        .await;
}
