//! Tests for the route dialog's dispatch: open, load, probe, toggle and a failed save.

use super::*;
use crate::views::hops_dialog::ProbeState;
use xai_grok_shell::agent::model_route::{RouteLink, RouteView};

const ID: AgentId = AgentId(0);

fn link(id: &str, kind: &str, optional: bool, enabled: bool, url: &str) -> RouteLink {
    RouteLink {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: kind.to_owned(),
        optional,
        enabled,
        url: url.to_owned(),
    }
}

/// The chain the user guide describes: a relay, a gateway, then the provider.
fn chain() -> RouteView {
    RouteView {
        provider_id: "openrouter".to_owned(),
        links: vec![
            link("nas", "relay", true, true, "http://nas.lan:8317"),
            link(
                "bifrost",
                "gateway",
                true,
                false,
                "http://bifrost.lan:8080/v1",
            ),
            link(
                "openrouter",
                "provider",
                false,
                true,
                "https://openrouter.ai/api/v1",
            ),
        ],
        entry_url: "http://nas.lan:8317/openrouter/api/v1".to_owned(),
        error: None,
    }
}

/// An app whose session model is tagged with the provider `openrouter`.
fn app_on_openrouter() -> AppView {
    let mut app = test_app_with_agent();
    let id = acp::ModelId::new(Arc::from("openrouter/deepseek/deepseek-chat"));
    let meta = serde_json::json!({ "provider": "openrouter" });
    let models = &mut test_agent_mut(&mut app, ID).session.models;
    models.available.insert(
        id.clone(),
        acp::ModelInfo::new(id.clone(), "DeepSeek".to_string()).meta(meta.as_object().cloned()),
    );
    models.current = Some(id);
    app
}

fn loaded(app: &mut AppView, view: Option<RouteView>) -> Vec<Effect> {
    dispatch(
        Action::TaskComplete(TaskResult::RouteLoaded {
            agent_id: ID,
            provider_id: "openrouter".to_owned(),
            result: Ok(view),
        }),
        app,
    )
}

fn probed_links(effects: &[Effect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ProbeRouteLinks { links, .. } => Some(links),
            _ => None,
        })
        .flatten()
        .map(|(id, _)| id.as_str())
        .collect()
}

#[test]
fn opening_the_dialog_reads_the_chain_of_the_session_provider() {
    let mut app = app_on_openrouter();
    let effects = dispatch(Action::OpenHopsDialog, &mut app);

    let dialog = test_agent(&app, ID).hops_dialog.as_ref().expect("open");
    assert!(dialog.loading);
    assert_eq!(dialog.model_label, "DeepSeek");
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::LoadRoute { agent_id: ID, provider_id }] if provider_id == "openrouter"
        ),
        "got {effects:?}"
    );
}

/// A model with no provider tag has no chain to read. The dialog still opens and says so.
#[test]
fn a_model_with_no_provider_opens_the_dialog_and_reads_nothing() {
    let mut app = test_app_with_agent();
    let effects = dispatch(Action::OpenHopsDialog, &mut app);

    let dialog = test_agent(&app, ID).hops_dialog.as_ref().expect("open");
    assert!(!dialog.loading);
    assert!(dialog.view.is_none());
    assert!(effects.is_empty(), "got {effects:?}");
}

/// A link that is off is not on the path, so it is not measured.
#[test]
fn a_loaded_chain_is_shown_and_its_live_links_are_measured() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let effects = loaded(&mut app, Some(chain()));

    assert_eq!(probed_links(&effects), ["nas", "openrouter"]);
    let agent = test_agent(&app, ID);
    assert_eq!(agent.route_view, Some(chain()));
    let dialog = agent.hops_dialog.as_ref().expect("open");
    assert!(!dialog.loading);
    assert_eq!(dialog.probes.get("nas"), Some(&ProbeState::Pending));
    assert_eq!(dialog.probes.get("bifrost"), None);
}

/// The chain is kept for the indicators when no dialog is open.
#[test]
fn a_chain_loaded_with_the_dialog_closed_is_kept_and_not_measured() {
    let mut app = app_on_openrouter();
    let effects = loaded(&mut app, Some(chain()));

    assert!(effects.is_empty(), "got {effects:?}");
    assert_eq!(test_agent(&app, ID).route_view, Some(chain()));
}

#[test]
fn a_chain_of_another_provider_is_dropped() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let effects = dispatch(
        Action::TaskComplete(TaskResult::RouteLoaded {
            agent_id: ID,
            provider_id: "anthropic".to_owned(),
            result: Ok(Some(chain())),
        }),
        &mut app,
    );

    assert!(effects.is_empty(), "got {effects:?}");
    assert!(test_agent(&app, ID).route_view.is_none());
}

#[test]
fn a_failed_read_is_reported_in_the_dialog() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let _ = dispatch(
        Action::TaskComplete(TaskResult::RouteLoaded {
            agent_id: ID,
            provider_id: "openrouter".to_owned(),
            result: Err("config.json: expected value at line 3".to_owned()),
        }),
        &mut app,
    );

    let dialog = test_agent(&app, ID).hops_dialog.as_ref().expect("open");
    assert!(!dialog.loading);
    let notice = dialog.notice.as_deref().unwrap_or_default();
    assert!(
        notice.contains("expected value at line 3"),
        "got {notice:?}"
    );
}

#[test]
fn a_probe_answer_lands_on_its_link() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let _ = loaded(&mut app, Some(chain()));
    let probed = |link_id: &str, result| {
        Action::TaskComplete(TaskResult::RouteLinkProbed {
            agent_id: ID,
            link_id: link_id.to_owned(),
            result,
        })
    };
    let _ = dispatch(probed("nas", Ok(4)), &mut app);
    let _ = dispatch(probed("openrouter", Err("timed out".to_owned())), &mut app);
    // This link is off and was not measured. A late answer for it is dropped.
    let _ = dispatch(probed("bifrost", Ok(9)), &mut app);

    let dialog = test_agent(&app, ID).hops_dialog.as_ref().expect("open");
    assert_eq!(dialog.probes.get("nas"), Some(&ProbeState::Millis(4)));
    assert_eq!(
        dialog.probes.get("openrouter"),
        Some(&ProbeState::Failed("timed out".to_owned()))
    );
    assert_eq!(dialog.probes.get("bifrost"), None);
}

#[test]
fn the_probe_key_measures_the_live_links_again() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let _ = loaded(&mut app, Some(chain()));
    let _ = dispatch(
        Action::TaskComplete(TaskResult::RouteLinkProbed {
            agent_id: ID,
            link_id: "nas".to_owned(),
            result: Ok(4),
        }),
        &mut app,
    );

    let effects = dispatch(Action::ProbeRoute, &mut app);

    assert_eq!(probed_links(&effects), ["nas", "openrouter"]);
    let dialog = test_agent(&app, ID).hops_dialog.as_ref().expect("open");
    assert_eq!(dialog.probes.get("nas"), Some(&ProbeState::Pending));
}

fn toggle(app: &mut AppView, hop_id: &str, enabled: bool) -> Vec<Effect> {
    dispatch(
        Action::ToggleHop {
            provider_id: "openrouter".to_owned(),
            hop_id: hop_id.to_owned(),
            enabled,
        },
        app,
    )
}

fn link_enabled(view: Option<&RouteView>, hop_id: &str) -> Option<bool> {
    let link = view?.links.iter().find(|link| link.id == hop_id)?;
    Some(link.enabled)
}

/// The row changes before the write answers. The saved file is then read again.
#[test]
fn a_toggle_flips_the_row_and_saves_then_reads_the_chain_again() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let _ = loaded(&mut app, Some(chain()));

    let effects = toggle(&mut app, "nas", false);

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::PersistHopEnabled { agent_id: ID, provider_id, hop_id, enabled: false }]
                if provider_id == "openrouter" && hop_id == "nas"
        ),
        "got {effects:?}"
    );
    let agent = test_agent(&app, ID);
    assert_eq!(link_enabled(agent.route_view.as_ref(), "nas"), Some(false));
    let dialog = agent.hops_dialog.as_ref().expect("open");
    assert_eq!(link_enabled(dialog.view.as_ref(), "nas"), Some(false));

    let effects = dispatch(
        Action::TaskComplete(TaskResult::HopEnabledPersisted {
            agent_id: ID,
            hop_id: "nas".to_owned(),
            enabled: false,
            result: Ok(()),
        }),
        &mut app,
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::LoadRoute { agent_id: ID, provider_id }] if provider_id == "openrouter"
        ),
        "got {effects:?}"
    );
}

#[test]
fn a_toggle_that_does_not_save_puts_the_row_back_and_says_why() {
    let mut app = app_on_openrouter();
    let _ = dispatch(Action::OpenHopsDialog, &mut app);
    let _ = loaded(&mut app, Some(chain()));
    let _ = toggle(&mut app, "nas", false);

    let effects = dispatch(
        Action::TaskComplete(TaskResult::HopEnabledPersisted {
            agent_id: ID,
            hop_id: "nas".to_owned(),
            enabled: false,
            result: Err("config.json is read-only".to_owned()),
        }),
        &mut app,
    );

    assert!(effects.is_empty(), "got {effects:?}");
    let agent = test_agent(&app, ID);
    assert_eq!(link_enabled(agent.route_view.as_ref(), "nas"), Some(true));
    let dialog = agent.hops_dialog.as_ref().expect("open");
    assert_eq!(link_enabled(dialog.view.as_ref(), "nas"), Some(true));
    assert_eq!(
        dialog.notice.as_deref(),
        Some("Not saved: config.json is read-only")
    );
}

/// With the dialog closed, the failed save still reaches the user.
#[test]
fn a_failed_save_with_the_dialog_closed_is_a_toast() {
    let mut app = app_on_openrouter();
    let _ = loaded(&mut app, Some(chain()));
    let _ = toggle(&mut app, "nas", false);

    let _ = dispatch(
        Action::TaskComplete(TaskResult::HopEnabledPersisted {
            agent_id: ID,
            hop_id: "nas".to_owned(),
            enabled: false,
            result: Err("disk full".to_owned()),
        }),
        &mut app,
    );

    assert_eq!(read_toast(&app), "Hop toggle not saved: disk full");
    let agent = test_agent(&app, ID);
    assert_eq!(link_enabled(agent.route_view.as_ref(), "nas"), Some(true));
}
