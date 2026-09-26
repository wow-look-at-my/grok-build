//! The summary a long thinking block gains from the shell's side call.
//!
//! Every case here enters through the shipped notification handlers: an ACP
//! thought chunk builds the row, and an `x.ai/session/update` carrying
//! `thinking_summary` is what must decorate it. The block's own setter is
//! covered in `scrollback::blocks::thinking`; what these tests pin is the join
//! between the two halves, which no unit test of either half can see.

#![cfg_attr(rustfmt, rustfmt::skip)]

use super::*;
use crate::appearance::AppearanceConfig;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{BlockContext, DisplayMode};

/// A thought chunk of one model call, stamped with that call's
/// `streamStartMs`, the key a summary later names.
fn thought_chunk(
    session_id: &str,
    text: &str,
    stream_start_ms: i64,
    is_replay: bool,
) -> AcpClientMessage {
    let meta = serde_json::json!({
        "promptId": "pid-1",
        "isReplay": is_replay,
        "streamStartMs": stream_start_ms,
    });
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = acp::SessionNotification::new(
        acp::SessionId::new(session_id),
        acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
            acp::ContentBlock::Text(acp::TextContent::new(text)),
        )),
    )
    .meta(meta.as_object().cloned());
    AcpClientMessage::SessionNotification(xai_acp_lib::AcpArgs {
        request,
        response_tx: tx,
    })
}

/// The shell's `thinking_summary` broadcast, built through the typed payload so
/// the wire shape cannot drift from what the shell emits. `x.ai/session/update`
/// is the method a reload replays persisted updates under.
fn thinking_summary_notif(
    session_id: &str,
    stream_start_ms: i64,
    summary: &str,
    is_replay: bool,
) -> acp::ExtNotification {
    let payload = SessionNotification {
        session_id: acp::SessionId::new(session_id),
        update: XaiSessionUpdate::ThinkingSummary {
            stream_start_ms,
            summary: summary.to_string(),
        },
        meta: Some(serde_json::json!({ "isReplay": is_replay })),
    };
    acp::ExtNotification::new(
        "x.ai/session/update",
        std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
    )
}

fn collapsed_ctx() -> BlockContext {
    BlockContext {
        mode: DisplayMode::Collapsed,
        is_running: false,
        width: 60,
        raw: false,
        max_lines: None,
        appearance: AppearanceConfig::default(),
        is_selected: false,
        cwd: None,
    }
}

/// Every thinking row's rendered collapsed text, in transcript order.
fn collapsed_thinking_rows(agent: &mut AgentView) -> Vec<String> {
    agent
        .scrollback
        .entries_mut()
        .filter(|e| matches!(&e.block, RenderBlock::Thinking(_)))
        .map(|e| {
            let out = e.block.output(&collapsed_ctx());
            out.lines
                .iter()
                .map(|l| crate::scrollback::types::line_plain_text(&l.content))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

/// Measured height of the first thinking row, after a real layout pass.
fn first_thinking_height(agent: &mut AgentView) -> u16 {
    agent.scrollback.prepare_layout(80, 40);
    let idx = (0..agent.scrollback.len())
        .find(|&i| {
            agent
                .scrollback
                .entry(i)
                .is_some_and(|e| e.block.is_thinking())
        })
        .expect("a thinking row was drawn");
    agent
        .scrollback
        .get_cached_entry_height(idx)
        .expect("prepare_layout measures every row")
}

/// Stream one model call's thinking and its answer, which is what finishes the
/// thinking row and collapses it, the state a summary arrives at.
fn stream_call(app: &mut AppView, session_id: &str, stream_start_ms: i64, thinking: &str) {
    let _ = handle(
        thought_chunk(session_id, thinking, stream_start_ms, false),
        app,
    );
    let _ = handle(
        make_agent_chunk_for_response(session_id, "the answer", "pid-1", stream_start_ms),
        app,
    );
}

#[test]
fn a_summary_lands_under_the_collapsed_thinking_row() {
    let mut app = make_app_with_agent("sess-think-sum");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.start_turn(&mut agent.scrollback);
        agent.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(
        &mut app,
        "sess-think-sum",
        1_000,
        "Reads the parser, then fixes the off-by-one in the lexer.",
    );

    let summary = "Reads the parser, then fixes the off-by-one in the lexer.";
    let before = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert!(
        !before[0].contains("off-by-one"),
        "the summary is not in the row before the update: {before:?}"
    );

    let changed = handle_ext_notification(
        &thinking_summary_notif("sess-think-sum", 1_000, summary, false),
        &mut app,
    );
    assert!(changed, "a row that gained a line must report a screen change");

    let after = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert_eq!(after.len(), before.len(), "no row was added or removed");
    assert!(
        after[0].contains(summary),
        "the summary must render under the header: {after:?}"
    );
    assert!(
        after[0].lines().count() > before[0].lines().count(),
        "the summary is its own row under the header: {after:?}"
    );
}

#[test]
fn a_summary_arriving_after_its_row_finished_still_repaints_it() {
    // The summary is written by a side call, so it lands after the thinking row
    // stopped running and after the turn may have ended. A row that only
    // refreshed on frames it drew would keep showing the bare header here.
    let mut app = make_app_with_agent("sess-think-late");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.start_turn(&mut agent.scrollback);
        agent.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(&mut app, "sess-think-late", 1_000, "long reasoning here");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.tracker.finish_turn(&mut agent.scrollback, Some("pid-1"));
        let rows = agent
            .scrollback
            .entries_mut()
            .filter(|e| matches!(&e.block, RenderBlock::Thinking(_)))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].is_running, "the row is finished before the summary");
        assert_eq!(rows[0].display_mode, DisplayMode::Collapsed);
    }

    let height_before =
        first_thinking_height(app.agents.get_mut(&AgentId(0)).unwrap());

    let summary = "It read the lexer's token boundaries first, then changed only the \
                   offset the parser used for a string literal, leaving the grammar alone.";
    let changed = handle_ext_notification(
        &thinking_summary_notif("sess-think-late", 1_000, summary, false),
        &mut app,
    );
    assert!(changed, "a finished row gaining a line is a screen change");

    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    let rows = collapsed_thinking_rows(agent);
    assert!(
        rows[0].contains("It read the lexer's token boundaries"),
        "the finished row must show the summary: {rows:?}"
    );
    let height_after = first_thinking_height(app.agents.get_mut(&AgentId(0)).unwrap());
    assert!(
        height_after > height_before,
        "the summary adds rows, so the measured height must grow: \
         was {height_before}, now {height_after}"
    );
    // The same summary again changes nothing, so no repaint is scheduled for it.
    let again = handle_ext_notification(
        &thinking_summary_notif("sess-think-late", 1_000, summary, false),
        &mut app,
    );
    assert!(!again, "an identical summary is not a change");
}

#[test]
fn each_model_call_keeps_its_own_summary() {
    // A turn's tool loop is several model calls, each with its own thinking row.
    // A summary names the call that produced it, so the later call's summary
    // must not land on the first row, and one arriving out of order still has
    // to find its own.
    let mut app = make_app_with_agent("sess-think-two");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.start_turn(&mut agent.scrollback);
        agent.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(&mut app, "sess-think-two", 1_000, "first call reasoning");
    stream_call(&mut app, "sess-think-two", 2_000, "second call reasoning");

    let _ = handle_ext_notification(
        &thinking_summary_notif("sess-think-two", 2_000, "SECOND", false),
        &mut app,
    );
    let _ = handle_ext_notification(
        &thinking_summary_notif("sess-think-two", 1_000, "FIRST", false),
        &mut app,
    );

    let rows = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert_eq!(rows.len(), 2, "both calls drew a thinking row");
    assert!(rows[0].ends_with("FIRST"), "call 1 row: {rows:?}");
    assert!(rows[1].ends_with("SECOND"), "call 2 row: {rows:?}");
}

#[test]
fn a_summary_naming_no_thinking_row_changes_nothing() {
    // The call streamed no thinking, or its row was removed by a rewind. There
    // is no block this summary describes, so attaching it to the one on screen
    // would put words under the wrong reasoning.
    let mut app = make_app_with_agent("sess-think-none");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.start_turn(&mut agent.scrollback);
        agent.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(&mut app, "sess-think-none", 1_000, "first call reasoning");
    stream_call(&mut app, "sess-think-none", 2_000, "second call reasoning");
    let before = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());

    let changed = handle_ext_notification(
        &thinking_summary_notif("sess-think-none", 9_999, "orphan summary", false),
        &mut app,
    );
    assert!(!changed, "a summary nothing asked for must not repaint");

    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    let after = collapsed_thinking_rows(agent);
    assert_eq!(after, before, "no row may gain or lose a line");
    assert!(
        after.iter().all(|r| !r.contains("orphan summary")),
        "the orphan summary must not be drawn: {after:?}"
    );
}

#[test]
fn a_reloaded_transcript_puts_each_summary_back_under_its_row() {
    // `thinking_summary` is persisted, so a reload replays it behind its own
    // call's persisted chunks. `streamStartMs` rides in those chunks' `_meta`
    // unchanged, which is what lets the replayed join reproduce itself.
    let mut app = make_app_with_agent("sess-think-reload");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.loading_replay = true;
    }
    let _ = handle(
        thought_chunk("sess-think-reload", "reasoning from last run", 1_000, true),
        &mut app,
    );
    let _ = handle(
        make_agent_chunk_meta("sess-think-reload", "answer from last run", "pid-1", None, true),
        &mut app,
    );
    let _ = handle_ext_notification(
        &thinking_summary_notif("sess-think-reload", 1_000, "REPLAYED SUMMARY", true),
        &mut app,
    );

    let rows = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert_eq!(rows.len(), 1, "the replayed call drew one thinking row");
    assert!(
        rows[0].contains("REPLAYED SUMMARY"),
        "the replay must re-attach the summary: {rows:?}"
    );

    // The replay flag is what admits this update: with no load in flight the
    // same payload is dropped rather than appended under the live transcript.
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.loading_replay = false;
    }
    let dropped = handle_ext_notification(
        &thinking_summary_notif("sess-think-reload", 1_000, "LATE REPLAY", true),
        &mut app,
    );
    assert!(!dropped, "a replay with no load in flight is dropped");
    let rows = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert!(
        rows[0].contains("REPLAYED SUMMARY") && !rows[0].contains("LATE REPLAY"),
        "the dropped replay left the row as it was: {rows:?}"
    );
}
