use super::*;

/// Result of looking up which view a notification's `session_id` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionMatch {
    /// The session_id matches the root session of this agent.
    Root(AgentId),
    /// The session_id matches a subagent view child of this agent (i.e. an entry in `agent.subagent_views`).
    Child(AgentId),
}

impl SessionMatch {
    /// The owning agent's id: the matched agent for `Root`, the parent that
    /// owns the `subagent_views` entry for `Child`.
    pub(super) fn agent_id(self) -> AgentId {
        match self {
            SessionMatch::Root(id) | SessionMatch::Child(id) => id,
        }
    }
}

/// The session and scrollback that own the task rows for `session_id`.
/// A subagent's `session_id` returns its child view.
pub(crate) fn task_view_by_session_id<'a>(
    app: &'a mut AppView,
    session_id: &str,
) -> Option<(
    &'a mut AgentSession,
    &'a mut crate::scrollback::state::ScrollbackState,
)> {
    let (matched, _, agent) = resolve_notif_agent(app, &acp::SessionId::new(session_id))?;
    resolve_target_view(agent, matched, session_id)
}

/// Resolve the agent that owns a notification's `session_id` and whether the active view is affected.
///
/// Convenience wrapper around `find_session_match`, `is_matched_agent_active`, and `agents.get_mut()`, used by the bg-task notification handlers.
pub(super) fn resolve_notif_agent<'a>(
    app: &'a mut AppView,
    session_id: &acp::SessionId,
) -> Option<(SessionMatch, bool, &'a mut AgentView)> {
    let matched = find_session_match(app, session_id)?;
    let parent_id = matched.agent_id();
    let is_active = is_matched_agent_active(app, parent_id);
    let agent = app.agents.get_mut(&parent_id)?;
    Some((matched, is_active, agent))
}

/// A background session's progress updates and completion signal land on *its* agent instead of whichever agent is foregrounded.
/// Otherwise a background agent's "Connecting MCPs (N/M)…" spinner is never cleared and sticks forever.
/// Only resolves to a `Root` agent: `mcp_init_progress` is a per-root-agent indicator with no per-subagent slot.
pub(super) fn mcp_target_agent<'a>(
    app: &'a mut AppView,
    session_id: Option<&str>,
) -> Option<(bool, &'a mut AgentView)> {
    match session_id {
        Some(sid) => {
            let sid = acp::SessionId::new(sid);
            let (matched, is_active, agent) = resolve_notif_agent(app, &sid)?;
            if matches!(matched, SessionMatch::Child(_)) {
                return None;
            }
            Some((is_active, agent))
        }
        None => {
            let id = match app.active_view {
                ActiveView::Agent(id) => id,
                ActiveView::Welcome => app.home_session_agent?,
                ActiveView::AgentDashboard => return None,
            };
            let agent = app.agents.get_mut(&id)?;
            Some((matches!(app.active_view, ActiveView::Agent(_)), agent))
        }
    }
}

/// The in-flight create a setup-phase notification targets, matched only by `pending_session_id`
/// (not the bound id, so a late phase can't re-stain a live session; no active-view fallback).
pub(super) fn setup_phase_target_agent<'a>(
    app: &'a mut AppView,
    session_id: &str,
) -> Option<&'a mut AgentView> {
    let sid = acp::SessionId::new(session_id);
    app.agents
        .values_mut()
        .find(|agent| agent.pending_session_id.as_ref() == Some(&sid))
}

/// Given a matched session and the owning agent, borrow the correct `(session, scrollback)` pair.
/// That is the child view's pair when the notification targets a subagent, the root agent's otherwise.
pub(super) fn resolve_target_view<'a>(
    agent: &'a mut AgentView,
    matched: SessionMatch,
    child_sid: &str,
) -> Option<(
    &'a mut AgentSession,
    &'a mut crate::scrollback::state::ScrollbackState,
)> {
    if matches!(matched, SessionMatch::Child(_)) {
        // A `TaskBackgrounded` / `TaskCompleted` block for a resumed child always follows the funneled tool_call.
        let child_view = agent.subagent_views.get_mut(child_sid)?;
        Some((&mut child_view.session, &mut child_view.scrollback))
    } else {
        Some((&mut agent.session, &mut agent.scrollback))
    }
}

/// The only agent that could own such a pre-assignment notification is the one the user just created (necessarily active, `session_id == None`).
/// Returns `None` when the notification cannot be associated with any agent.
/// All ACP-notification handlers must route through this function rather than gating on `app.active_view` directly.
pub(super) fn find_session_match(
    app: &AppView,
    session_id: &acp::SessionId,
) -> Option<SessionMatch> {
    // An exact root match returns immediately (root wins when both could match).
    let child_key: &str = session_id.0.as_ref();
    let mut child_match: Option<AgentId> = None;
    for (id, agent) in &app.agents {
        if agent.session.session_id.as_ref() == Some(session_id) {
            return Some(SessionMatch::Root(*id));
        }
        if child_match.is_none() && agent.subagent_views.contains_key(child_key) {
            child_match = Some(*id);
        }
    }
    if let Some(id) = child_match {
        return Some(SessionMatch::Child(id));
    }
    if let ActiveView::Agent(active_id) = app.active_view
        && let Some(agent) = app.agents.get(&active_id)
        && agent.session.session_id.is_none()
    {
        return Some(SessionMatch::Root(active_id));
    }
    if matches!(app.active_view, ActiveView::Welcome)
        && let Some(id) = app.home_session_agent
        && let Some(agent) = app.agents.get(&id)
        && agent.session.session_id.is_none()
    {
        return Some(SessionMatch::Root(id));
    }
    None
}

/// Whether the matched agent is the one currently displayed.
pub(super) fn is_matched_agent_active(app: &AppView, matched_agent: AgentId) -> bool {
    matches!(app.active_view, ActiveView::Agent(id) if id == matched_agent)
}

/// Routes by the request's session id via [`find_session_match`] (exactly like `session/update` notifications), not gated on `app.active_view`. A modal raised by a **background** session thus lands on its own view even when the user is on the
/// dashboard or a different session.
pub(super) fn interaction_target_agent(app: &AppView, session_id: &str) -> Option<AgentId> {
    let sid = acp::SessionId::new(session_id.to_owned());
    match find_session_match(app, &sid) {
        Some(SessionMatch::Root(id) | SessionMatch::Child(id)) => Some(id),
        None => None,
    }
}
