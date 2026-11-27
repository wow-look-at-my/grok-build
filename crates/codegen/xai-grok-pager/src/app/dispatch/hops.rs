//! Dispatch for the route dialog: open it, load a provider's chain, measure its links and save a hop toggle.

use xai_grok_shell::agent::model_route::RouteView;

use crate::app::actions::Effect;
use crate::app::agent::AgentId;
use crate::app::app_view::{ActiveView, AppView};
use crate::views::hops_dialog::{HopsDialogState, ProbeState};

/// The links of `view` worth a probe: the ones that are on and have an address.
fn probe_targets(view: &RouteView) -> Vec<(String, String)> {
    view.links
        .iter()
        .filter(|link| link.enabled && !link.url.trim().is_empty())
        .map(|link| (link.id.clone(), link.url.clone()))
        .collect()
}

/// Mark every probe target pending and return the effect that measures them.
fn start_probes(agent_id: AgentId, dialog: &mut HopsDialogState) -> Vec<Effect> {
    let Some(view) = dialog.view.as_ref() else {
        return vec![];
    };
    let links = probe_targets(view);
    dialog.probes.clear();
    for (id, _) in &links {
        dialog.probes.insert(id.clone(), ProbeState::Pending);
    }
    if links.is_empty() {
        return vec![];
    }
    vec![Effect::ProbeRouteLinks { agent_id, links }]
}

/// `/hops`: open the dialog on what is known, then read the chain from config.
pub(super) fn open_hops_dialog(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(agent_id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    let model_label = agent
        .session
        .models
        .current_model_name()
        .unwrap_or_else(|| "unknown".to_owned());
    let provider_id = agent
        .session
        .models
        .current_model_provider()
        .map(str::to_owned);
    let mut dialog = HopsDialogState {
        view: agent.route_view.clone(),
        model_label,
        loading: provider_id.is_some(),
        ..Default::default()
    };
    let mut effects = start_probes(agent_id, &mut dialog);
    agent.hops_dialog = Some(dialog);
    if let Some(provider_id) = provider_id {
        effects.push(Effect::LoadRoute {
            agent_id,
            provider_id,
        });
    }
    effects
}

/// The dialog's `r`: measure the links again.
pub(super) fn probe_route(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(agent_id) = app.active_view else {
        return vec![];
    };
    let Some(dialog) = app
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.hops_dialog.as_mut())
    else {
        return vec![];
    };
    start_probes(agent_id, dialog)
}

/// A hop toggle. The row flips at once and the write follows. A failed write flips it back.
pub(super) fn toggle_hop(
    app: &mut AppView,
    provider_id: String,
    hop_id: String,
    enabled: bool,
) -> Vec<Effect> {
    let ActiveView::Agent(agent_id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    if let Some(dialog) = agent.hops_dialog.as_mut() {
        dialog.notice = None;
        set_link_enabled(dialog.view.as_mut(), &hop_id, enabled);
    }
    set_link_enabled(agent.route_view.as_mut(), &hop_id, enabled);
    vec![Effect::PersistHopEnabled {
        agent_id,
        provider_id,
        hop_id,
        enabled,
    }]
}

fn set_link_enabled(view: Option<&mut RouteView>, hop_id: &str, enabled: bool) {
    if let Some(link) = view.and_then(|view| view.links.iter_mut().find(|link| link.id == hop_id)) {
        link.enabled = enabled;
    }
}

pub(super) fn handle_route_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    provider_id: String,
    result: Result<Option<RouteView>, String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    // The session moved to another provider while the read ran.
    if agent.session.models.current_model_provider() != Some(provider_id.as_str()) {
        return vec![];
    }
    let view = match result {
        Ok(view) => view,
        Err(error) => {
            tracing::error!(%error, provider = %provider_id, "route read failed");
            if let Some(dialog) = agent.hops_dialog.as_mut() {
                dialog.loading = false;
                dialog.notice = Some(format!("Could not read the route from config: {error}"));
            }
            return vec![];
        }
    };
    agent.route_view = view.clone();
    let Some(dialog) = agent.hops_dialog.as_mut() else {
        return vec![];
    };
    dialog.loading = false;
    dialog.set_view(view);
    start_probes(agent_id, dialog)
}

pub(super) fn handle_route_link_probed(
    app: &mut AppView,
    agent_id: AgentId,
    link_id: String,
    result: Result<u64, String>,
) -> Vec<Effect> {
    let Some(dialog) = app
        .agents
        .get_mut(&agent_id)
        .and_then(|agent| agent.hops_dialog.as_mut())
    else {
        return vec![];
    };
    // A probe of a link the chain no longer has, or that is no longer measured, is dropped.
    if dialog.probes.contains_key(&link_id) {
        let state = match result {
            Ok(millis) => ProbeState::Millis(millis),
            Err(error) => ProbeState::Failed(error),
        };
        dialog.probes.insert(link_id, state);
    }
    vec![]
}

pub(super) fn handle_hop_enabled_persisted(
    app: &mut AppView,
    agent_id: AgentId,
    hop_id: String,
    enabled: bool,
    result: Result<(), String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    let provider_id = agent
        .session
        .models
        .current_model_provider()
        .map(str::to_owned);
    if let Err(error) = result {
        tracing::error!(%error, hop = %hop_id, "hop toggle did not save; the row is put back");
        set_link_enabled(agent.route_view.as_mut(), &hop_id, !enabled);
        match agent.hops_dialog.as_mut() {
            Some(dialog) => {
                set_link_enabled(dialog.view.as_mut(), &hop_id, !enabled);
                dialog.notice = Some(format!("Not saved: {error}"));
            }
            None => app.show_toast(&format!("Hop toggle not saved: {error}")),
        }
        return vec![];
    }
    // The saved file is the truth. Read it again for the entry URL the toggle changed.
    match provider_id {
        Some(provider_id) => vec![Effect::LoadRoute {
            agent_id,
            provider_id,
        }],
        None => vec![],
    }
}
