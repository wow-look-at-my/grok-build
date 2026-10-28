//! The summary call a thinking block triggers, driven end to end.

use super::support::*;
use super::*;
use xai_grok_sampling_types::{ConversationItem, ConversationResponse, synthesized_reasoning_item};
use xai_grok_test_support::sse::responses_api_script_exact;
use xai_grok_test_support::{
    DEFAULT_MODEL, InferenceEndpoint, InferenceExpectation, InferenceRequestMatcher,
    MockInferenceServer, ScriptedResponse,
};

/// Where a summarization call lands once `aim_at` points the actor at the mock.
const SUMMARY_PATH: &str = "/v1/responses";

/// Answer a summarization call with `text`, parked until the handle releases it.
fn hold_next_summary(server: &MockInferenceServer, name: &str, text: &str) -> InferenceExpectation {
    server.expect_response_blocked(
        name,
        InferenceRequestMatcher::auxiliary(InferenceEndpoint::Responses),
        ScriptedResponse::sse(responses_api_script_exact(text, DEFAULT_MODEL)),
    )
}

/// The `thinking_summary` payload on one gateway message, if it carries one.
fn summary_update(msg: &xai_acp_lib::AcpClientMessage) -> Option<serde_json::Value> {
    let xai_acp_lib::AcpClientMessage::ExtNotification(args) = msg else {
        return None;
    };
    if args.request.method.as_ref() != "x.ai/session_notification" {
        return None;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args.request.params.get()) else {
        return None;
    };
    let update = value.get("update")?;
    (update.get("sessionUpdate").and_then(|v| v.as_str()) == Some("thinking_summary"))
        .then(|| update.clone())
}

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
/// does. Do this so the side call is a real HTTP request rather than a stubbed
/// client.
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
    // Timer ticks, not yields: the summary waits on a real HTTP round trip. A loaded runner spends a yield budget before it lands.
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(5));
    for _ in 0..1000 {
        while let Ok(msg) = grx.try_recv() {
            if let Some(update) = summary_update(&msg) {
                return Some(update);
            }
        }
        interval.tick().await;
    }
    None
}

/// Every `thinking_summary` already on the gateway rail, leaving the rest queued.
fn drain_thinking_summaries(
    grx: &mut tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>,
) -> Vec<serde_json::Value> {
    let mut updates = Vec::new();
    while let Ok(msg) = grx.try_recv() {
        if let Some(update) = summary_update(&msg) {
            updates.push(update);
        }
    }
    updates
}

/// `true` once `ready` holds, checking it on timer ticks so the LocalSet runs.
async fn wait_until(ready: impl Fn() -> bool) -> bool {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(5));
    for _ in 0..400 {
        interval.tick().await;
        if ready() {
            return true;
        }
    }
    false
}

/// Run the LocalSet for `ticks` timer ticks, the window anything queued would
/// have needed to reach the mock server.
async fn settle(ticks: u32) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(5));
    for _ in 0..ticks {
        interval.tick().await;
    }
}

/// The `stream_start_ms` one summary update is keyed to.
fn stream_start_of(update: &serde_json::Value) -> Option<i64> {
    update.get("stream_start_ms").and_then(|v| v.as_i64())
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
            actor.thinking_summaries_enabled.set(true);
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

            // The same update must reach persistence, or a reload loses it while the live session still shows it.
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

            // The summary is exactly one model call, and that call carried the reasoning it was asked to summarize.
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
            actor.thinking_summaries_enabled.set(true);
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
            // Production sets this once at spawn from `[ui].thinking_summaries`.
            actor.thinking_summaries_enabled.set(false);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("must never be asked for");
            aim_at(&actor, &server).await;

            // Identical input to the on-position test above: only the switch differs.
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
            actor.thinking_summaries_enabled.set(true);
            // A couple of seconds between calls, so the default window covers both.
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
/// next call. The same calls under a count floor do carry it, so the window is
/// what excluded it.
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
                actor.thinking_summaries_enabled.set(true);
                actor.thinking_summary_history =
                    ThinkingSummaryHistory::new(window_secs, min_count);
                let actor = std::sync::Arc::new(actor);

                let server = MockInferenceServer::start().await.unwrap();
                server.set_response("Reuse the retry helper");
                aim_at(&actor, &server).await;

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
            actor.thinking_summaries_enabled.set(true);
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

#[tokio::test]
async fn the_second_summary_waits_for_the_first_request_to_finish() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled.set(true);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            aim_at(&actor, &server).await;
            let mut first = hold_next_summary(&server, "first summary", "Weighed the lexer");
            let mut second = hold_next_summary(&server, "second summary", "Patched the offset");

            actor.spawn_thinking_summary(
                &response_carrying_thinking("whether the lexer or the caller is wrong"),
                Some(1_000),
            );
            actor.spawn_thinking_summary(
                &response_carrying_thinking("the caller passes a stale offset"),
                Some(2_000),
            );

            // The first request is parked at its terminal event. Nothing else
            // may reach the server, and nothing may be broadcast yet.
            let arrived =
                tokio::time::timeout(std::time::Duration::from_secs(20), first.wait_blocked())
                    .await;
            assert!(
                arrived.is_ok(),
                "the first summary request must reach the mock: {}",
                server.request_log_summary()
            );
            assert_eq!(
                server.request_count_for(SUMMARY_PATH),
                1,
                "one in-flight summary means one request: {}",
                server.request_log_summary()
            );
            settle(50).await;
            assert_eq!(
                server.request_count_for(SUMMARY_PATH),
                1,
                "the second summary must not open a request while the first is held: {}",
                server.request_log_summary()
            );
            assert!(
                drain_thinking_summaries(&mut grx).is_empty(),
                "no summary exists before its model call has been answered"
            );

            // Answering the first is what releases the second.
            first.release();
            let update = take_thinking_summary(&mut grx)
                .await
                .expect("the first summary is broadcast when its call is answered");
            assert_eq!(
                stream_start_of(&update),
                Some(1_000),
                "the first broadcast belongs to the first model call"
            );
            assert!(
                update
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .contains("lexer"),
                "the broadcast is the first answer: {update}"
            );
            assert!(
                wait_until(|| server.request_count_for(SUMMARY_PATH) >= 2).await,
                "the second request must start once the first is answered: {}",
                server.request_log_summary()
            );
            assert_eq!(
                server.request_count_for(SUMMARY_PATH),
                2,
                "the second summary is one request of its own: {}",
                server.request_log_summary()
            );

            second.release();
            let update = take_thinking_summary(&mut grx)
                .await
                .expect("the queued summary is broadcast after its own call");
            assert_eq!(
                stream_start_of(&update),
                Some(2_000),
                "the second broadcast belongs to the second model call"
            );
            assert!(
                update
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .contains("offset"),
                "the broadcast is the second answer: {update}"
            );
            assert_eq!(
                server.request_count_for(SUMMARY_PATH),
                2,
                "two thinking blocks are two summary calls: {}",
                server.request_log_summary()
            );
        })
        .await;
}

/// A held summarization never blocks the caller: every later spawn returns
/// immediately and stays queued, so the session issues exactly one request and
/// broadcasts nothing.
#[tokio::test]
async fn a_held_summary_leaves_the_caller_free_and_the_queue_closed() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled.set(true);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            aim_at(&actor, &server).await;
            let mut held = hold_next_summary(&server, "held summary", "never answered here");

            actor.spawn_thinking_summary(&response_carrying_thinking("first thought"), Some(3_000));
            let arrived =
                tokio::time::timeout(std::time::Duration::from_secs(20), held.wait_blocked()).await;
            assert!(
                arrived.is_ok(),
                "the held request must reach the mock: {}",
                server.request_log_summary()
            );

            // A second block arrives while the first is still in flight.
            let began = std::time::Instant::now();
            actor
                .spawn_thinking_summary(&response_carrying_thinking("second thought"), Some(4_000));
            let returned_in = began.elapsed();
            assert!(
                returned_in < std::time::Duration::from_millis(250),
                "spawn_thinking_summary must return without waiting for the chain: {returned_in:?}"
            );

            settle(50).await;
            assert_eq!(
                server.request_count_for(SUMMARY_PATH),
                1,
                "the follow-up summary stays queued behind the held request: {}",
                server.request_log_summary()
            );
            assert!(
                drain_thinking_summaries(&mut grx).is_empty(),
                "a parked summarization broadcasts nothing"
            );
        })
        .await;
}

/// The summary call asks for reasoning off, and what reaches the wire names a
/// tier rather than omitting the field. An omitted effort is not neutral: the
/// provider answers it with its own default thinking. This is what a
/// five-word summary must not pay for. The mock's model publishes no effort
/// menu, so this also pins the unlisted rung.
#[tokio::test]
async fn a_summary_call_asks_for_reasoning_off_on_the_wire() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, mut grx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            actor.thinking_summaries_enabled.set(true);
            let actor = std::sync::Arc::new(actor);

            let server = MockInferenceServer::start().await.unwrap();
            server.set_response("Reads the parser first");
            aim_at(&actor, &server).await;

            actor.spawn_thinking_summary(
                &response_carrying_thinking(&long_reasoning()),
                Some(1_000),
            );
            take_thinking_summary(&mut grx)
                .await
                .expect("the summary must be broadcast");

            let bodies = server.request_bodies();
            assert_eq!(
                bodies.len(),
                1,
                "one thinking block is one summary call: {}",
                server.request_log_summary()
            );
            let effort = bodies[0]
                .pointer("/reasoning/effort")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "the summary call must name a reasoning tier; an omitted one \
                         inherits the provider default: {}",
                        bodies[0]
                    )
                });
            assert_ne!(
                effort, "medium",
                "a summary must never land on the default tier: {}",
                bodies[0]
            );
        })
        .await;
}
