//! The summary a long thinking block gains from the shell's side call.

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
fn a_summary_lands_on_the_collapsed_thinking_row() {
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
    assert!(changed, "a row that gained a summary must report a screen change");

    let after = collapsed_thinking_rows(app.agents.get_mut(&AgentId(0)).unwrap());
    assert_eq!(after.len(), before.len(), "no row was added or removed");
    let header = before[0].lines().next().expect("the row has a header");
    let first = after[0].lines().next().expect("the row has a header");
    assert!(
        first.starts_with(&format!("{header} - ")),
        "the summary continues the header row: {after:?}"
    );
    // Wrapping may break "off-by-one" at a hyphen, so compare without spaces.
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    assert!(
        squash(&after[0]).contains(&squash(summary)),
        "the whole summary is drawn: {after:?}"
    );
}

#[test]
fn a_subagent_summary_lands_in_the_subagent_view() {
    let mut app = make_app_with_parent_and_child("sess-parent", "sess-child");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        let child = agent.subagent_views.get_mut("sess-child").unwrap();
        child.session.start_turn(&mut child.scrollback);
        child.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(&mut app, "sess-child", 1_000, "child reasoning");

    let changed = handle_ext_notification(
        &thinking_summary_notif("sess-child", 1_000, "CHILD SUMMARY", false),
        &mut app,
    );
    assert!(changed, "the child's row gained a summary");

    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    let child = agent.subagent_views.get_mut("sess-child").unwrap();
    let rows = collapsed_thinking_rows(child);
    assert_eq!(rows.len(), 1, "the child call drew one thinking row: {rows:?}");
    assert!(rows[0].contains("CHILD SUMMARY"), "child row: {rows:?}");
    assert!(
        collapsed_thinking_rows(agent).iter().all(|r| !r.contains("CHILD SUMMARY")),
        "a child's summary must not land on the parent"
    );
}

#[test]
fn a_summary_arriving_after_its_row_finished_still_repaints_it() {
    // The summary is written by a side call.
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
    // The call streamed no thinking, or its row was removed by a rewind.
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
    // `thinking_summary` is persisted, so a reload replays it behind its own call's persisted chunks.
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

fn summary_status_notif(
    session_id: &str,
    stream_start_ms: i64,
    failed: Option<&str>,
) -> acp::ExtNotification {
    let payload = SessionNotification {
        session_id: acp::SessionId::new(session_id),
        update: XaiSessionUpdate::ThinkingSummaryStatus {
            stream_start_ms,
            failed: failed.map(str::to_string),
        },
        meta: None,
    };
    acp::ExtNotification::new(
        "x.ai/session/update",
        std::sync::Arc::from(serde_json::value::to_raw_value(&payload).unwrap()),
    )
}

/// A finished turn with one collapsed thinking row keyed `1_000`, so only the spinner can ask for ticks.
fn finished_turn_with_thought(session_id: &str) -> AppView {
    let mut app = make_app_with_agent(session_id);
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        agent.session.start_turn(&mut agent.scrollback);
        agent.session.current_prompt_id = Some("pid-1".into());
    }
    stream_call(&mut app, session_id, 1_000, "long reasoning here");
    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    agent.session.tracker.finish_turn(&mut agent.scrollback, Some("pid-1"));
    agent.scrollback.prepare_layout(80, 40);
    assert!(!agent.scrollback.needs_animation(), "a finished turn is idle");
    app
}

fn first_thinking_block(agent: &mut AgentView) -> crate::scrollback::blocks::ThinkingBlock {
    agent
        .scrollback
        .entries_mut()
        .find_map(|e| match &e.block {
            RenderBlock::Thinking(b) => Some(b.clone()),
            _ => None,
        })
        .expect("a thinking row was drawn")
}

#[test]
fn a_queued_summary_spins_until_the_summary_lands() {
    let mut app = finished_turn_with_thought("sess-think-spin");

    let changed = handle_ext_notification(
        &summary_status_notif("sess-think-spin", 1_000, None),
        &mut app,
    );
    assert!(changed, "a queued summary starts the spinner");
    {
        let agent = app.agents.get_mut(&AgentId(0)).unwrap();
        assert!(first_thinking_block(agent).summary_pending());
        assert!(
            agent.scrollback.needs_animation(),
            "the spinner must keep ticks flowing after the turn ended"
        );
        assert!(first_thinking_height(agent) > 0, "a verb group must not fold the spinner away");
    }

    let _ = handle_ext_notification(
        &thinking_summary_notif("sess-think-spin", 1_000, "DONE", false),
        &mut app,
    );
    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    assert!(!first_thinking_block(agent).summary_pending(), "the summary stops the spinner");
    assert!(!agent.scrollback.needs_animation(), "no spinner, no ticks");

    // A late "queued" for a call that already has its summary must not restart the spinner.
    let again = handle_ext_notification(
        &summary_status_notif("sess-think-spin", 1_000, None),
        &mut app,
    );
    assert!(!again, "the summary outranks a late queued status");
}

#[test]
fn a_failed_summary_says_why_instead_of_spinning() {
    let mut app = finished_turn_with_thought("sess-think-fail");
    let _ = handle_ext_notification(
        &summary_status_notif("sess-think-fail", 1_000, None),
        &mut app,
    );
    let changed = handle_ext_notification(
        &summary_status_notif("sess-think-fail", 1_000, Some("model call failed: 529 overloaded")),
        &mut app,
    );
    assert!(changed, "a failure replaces the spinner");

    let agent = app.agents.get_mut(&AgentId(0)).unwrap();
    assert!(!first_thinking_block(agent).summary_pending());
    assert!(!agent.scrollback.needs_animation(), "a failed call must not spin forever");
    let rows = collapsed_thinking_rows(agent);
    // The row wraps at the test width, so compare without whitespace.
    let squash = |s: &str| s.split_whitespace().collect::<String>();
    assert!(
        squash(&rows[0]).contains(&squash("no summary: model call failed: 529 overloaded")),
        "the row names the failure: {rows:?}"
    );
}
