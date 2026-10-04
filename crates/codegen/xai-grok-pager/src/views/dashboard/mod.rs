//! The agent dashboard lists every top-level agent, grouped by state, with peek, attach, and dispatch actions.

mod actions_focus;
pub(crate) mod animation;
mod chrome;
pub mod layout;
pub mod peek;
pub mod peek_tail;
mod preview;
pub mod render;
pub mod row;
mod row_activity;
mod row_title;
mod search;
pub mod state;
#[cfg(test)]
mod test_support;
mod usage_modal;

pub use chrome::HeaderUpgradeCta;
pub(crate) use render::render_dashboard;
pub use render::{popup_rect, render_popup_overlay};
pub use row::{
    DashboardRow, RowBadge, build_rows_with_roster, classify_top_level, roster_activity_to_state,
    sort_rows,
};
pub(crate) use row::{WorkspaceRowInputs, build_rows_with_workspace};
pub(crate) use state::DashboardStopAction;
pub use state::{
    DashboardDispatchMode, DashboardRowId, DashboardState, Filter, FilterValue, Focusable,
    Grouping, LocationCandidate, LocationPickerState, PendingDispatchModel, PersistedDashboard,
    PersistedRowId, RowState, SectionKey, SessionIdResolver, ShortcutsModalState, load_persisted,
    parse_filter, parse_row_state_token,
};

/// Top-level agents visible in the dashboard's row list, in the exact order [`render_dashboard`]
/// paints them. "Previous" / "next" then follow what the user actually sees instead of the agent
/// map's insertion order.
pub fn overlay_cycle_order(
    state: &DashboardState,
    agents: &indexmap::IndexMap<crate::app::agent::AgentId, crate::app::agent_view::AgentView>,
) -> Vec<crate::app::agent::AgentId> {
    let home = render::cached_home();
    let rows = build_rows_with_roster(
        agents,
        &state.pinned,
        &state.reorder,
        state.grouping,
        &state.filter,
        home,
        &[],
    );
    rows.iter()
        .filter_map(|r| match &r.id {
            DashboardRowId::TopLevel(id) => Some(*id),
            _ => None,
        })
        .collect()
}

/// The env override wins (`GROK_AGENT_DASHBOARD=0` turns the dashboard off),
/// else the persisted `[dashboard].enabled` flag (default `true`).
pub fn dashboard_enabled() -> bool {
    if std::env::var_os("GROK_AGENT_DASHBOARD")
        .as_deref()
        .is_some_and(|v| v == std::ffi::OsStr::new("0"))
    {
        return false;
    }
    state::load_persisted_enabled().unwrap_or(true)
}

/// Command to name in the "use /X to switch between sessions" session banners
/// (the `/new` session-created banner and the fork marker).
pub(crate) fn session_switch_hint_command() -> Option<&'static str> {
    dashboard_enabled().then_some("/dashboard")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hint mirrors the dashboard flag: `None` when the env override
    /// disables it (the tip would name a refused command), otherwise whatever
    /// `dashboard_enabled()` says — asserted as consistency, not a fixed
    /// value, so the test doesn't depend on the machine's persisted
    /// `[dashboard].enabled`.
    #[serial_test::serial(GROK_AGENT_DASHBOARD)]
    #[test]
    fn switch_hint_follows_dashboard_flag() {
        // SAFETY: the test temporarily mutates a process-wide env var.
        unsafe { std::env::set_var("GROK_AGENT_DASHBOARD", "0") };
        assert_eq!(session_switch_hint_command(), None);
        unsafe { std::env::remove_var("GROK_AGENT_DASHBOARD") };
        assert_eq!(
            session_switch_hint_command(),
            dashboard_enabled().then_some("/dashboard")
        );
    }
}
