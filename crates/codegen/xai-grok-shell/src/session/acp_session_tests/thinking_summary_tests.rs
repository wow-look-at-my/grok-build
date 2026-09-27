//! The summary call a long thinking block triggers, driven end to end.
//!
//! The pager half (`scrollback::blocks::thinking`, and the pager's
//! `acp_handler::tests::thinking_summary`) draws whatever arrives here. What
//! these tests own is the producing half on the real path: a completed response
//! carrying over-threshold reasoning must put one request on the wire, and the
//! update it broadcasts must carry the key the summary was written for and the
//! cleaned text the model answered with. The switch is exercised in both
//! positions, because a session that resolved it off must spend nothing.

use super::support::*;
use super::*;
use xai_grok_sampling_types::{ConversationItem, ConversationResponse, synthesized_reasoning_item};
use xai_grok_test_support::MockInferenceServer;

/// Reasoning long enough to clear `THINKING_SUMMARY_MIN_CHARS`, so the
/// threshold itself is not what these tests are silently passing on.
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
async fn short_thinking_is_not_summarized_and_costs_no_request() {
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
            server.set_response("should never be asked for");
            aim_at(&actor, &server).await;

            actor.spawn_thinking_summary(
                &response_carrying_thinking("Read the parser first."),
                Some(5_000),
            );

            // Nothing is expected on the rail; give the LocalSet room to run any
            // task it wrongly spawned, then require that the endpoint stayed cold.
            for _ in 0..200 {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                server.request_count(),
                0,
                "thinking under the threshold must not cost a model call"
            );
            let mut broadcast_a_summary = false;
            while let Ok(msg) = grx.try_recv() {
                let xai_acp_lib::AcpClientMessage::ExtNotification(args) = msg else {
                    continue;
                };
                let is_summary =
                    serde_json::from_str::<serde_json::Value>(args.request.params.get())
                        .ok()
                        .and_then(|v| {
                            v.get("update")?
                                .get("sessionUpdate")?
                                .as_str()
                                .map(str::to_owned)
                        })
                        .is_some_and(|tag| tag == "thinking_summary");
                if is_summary {
                    broadcast_a_summary = true;
                }
            }
            assert!(
                !broadcast_a_summary,
                "short thinking has nothing to summarize; no update may reach the client"
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
