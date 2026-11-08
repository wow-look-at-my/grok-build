//! Config hot-reload, shared by every way of running an agent.
//!
//! `config.json` is the store of settings. `/settings` writes it and so does
//! an editor. This watcher is what carries either write into the running
//! agent.
use crate::agent::MvpAgent;
use crate::agent::config::Config as AgentConfig;
use crate::config::reloader::{ConfigReloader, ConfigUpdate};
use crate::config::watcher::ConfigFileWatcher;
use crate::leader::protocol::InternalMethod;
use crate::util::grok_home;
use std::path::Path;
use std::rc::Rc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Whether the watcher also follows project configs under the working directory.
pub(crate) fn recursive_config_watch_enabled() -> bool {
    let user_cfg = crate::config::load_from_disk().ok();
    let requirements = crate::agent::config::read_requirements_toml();
    crate::util::config::resolve_mcp_recursive_config_watch(
        requirements.as_ref(),
        user_cfg.as_ref(),
        None,
    )
}

/// Start the file watcher and the reloader that turns its events into [`ConfigUpdate`]s.
/// The returned watcher stops when dropped. `None` means hot-reload is off for this process.
pub(crate) fn start_config_reload(
    agent_config: &AgentConfig,
    cwd: &Path,
    recursive_watch: bool,
    cancel: CancellationToken,
) -> (
    Option<ConfigFileWatcher>,
    mpsc::UnboundedReceiver<ConfigUpdate>,
) {
    let (update_tx, update_rx) = mpsc::unbounded_channel();
    let mut watch_paths = crate::config::find_project_configs(cwd);
    watch_paths.extend(crate::util::config::mcp_json_candidate_paths(cwd));
    if let Some(home) = xai_dirs::home_dir() {
        watch_paths.push(home.join(".claude.json"));
    }
    let auth_scope = agent_config.grok_com_config.auth_scope();
    let initial_auth_key_hash = xai_grok_config::user_grok_home()
        .map(|g| g.join("auth.json"))
        .and_then(|auth_path| xai_grok_login::read_auth_json(&auth_path).ok())
        .and_then(|store| {
            xai_grok_login::lookup_auth(&store, &auth_scope)
                .map(|a| crate::config::reloader::hash_auth_key(&a.key))
        })
        .unwrap_or(0);
    let Some((watcher, events_rx)) = ConfigFileWatcher::start(
        &grok_home::grok_home(),
        &watch_paths,
        recursive_watch.then_some(cwd),
        None,
    ) else {
        warn!("Config file watcher failed to start; hot-reload disabled");
        return (None, update_rx);
    };
    let initial_config = crate::config::load_from_disk()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    let reloader = ConfigReloader::new(
        grok_home::grok_home(),
        initial_auth_key_hash,
        initial_config,
        auth_scope,
        None,
        update_tx,
        agent_config.memory_enabled_override,
    );
    xai_spawn::spawn(reloader.run(events_rx, cancel));
    (Some(watcher), update_rx)
}

/// The internal request that applies `update` to the running agent, or `None` when the update
/// is not a reload.
fn reload_request(update: &ConfigUpdate) -> Option<(InternalMethod, serde_json::Value)> {
    Some(match update {
        ConfigUpdate::McpServersChanged => {
            (InternalMethod::ReloadAllMcpServers, serde_json::json!({}))
        }
        ConfigUpdate::ProjectMcpServersChanged { cwd } => (
            InternalMethod::ReloadProjectMcpServers,
            serde_json::json!({ "cwd": cwd.to_string_lossy() }),
        ),
        ConfigUpdate::ModelsChanged => (InternalMethod::ReloadModels, serde_json::json!({})),
        ConfigUpdate::ModelsCacheChanged => {
            (InternalMethod::ReloadModelsCache, serde_json::json!({}))
        }
        ConfigUpdate::OutputRateFloorChanged => {
            (InternalMethod::ReloadOutputRateFloor, serde_json::json!({}))
        }
        ConfigUpdate::ThinkingSummariesChanged => (
            InternalMethod::ReloadThinkingSummaries,
            serde_json::json!({}),
        ),
        _ => return None,
    })
}

/// Run `update`'s reload on `agent`. Returns whether `update` was a reload.
pub(crate) async fn apply_reload(agent: &MvpAgent, update: &ConfigUpdate) -> bool {
    let Some((method, params)) = reload_request(update) else {
        return false;
    };
    info!(method = method.name(), "config change detected, reloading");
    let params = match serde_json::value::to_raw_value(&params) {
        Ok(params) => params,
        Err(error) => {
            warn!(%error, "config reload params did not serialize");
            return true;
        }
    };
    let request = agent_client_protocol::ExtRequest::new(method.name(), params.into());
    if let Err(error) = crate::extensions::session_admin::handle(agent, &request).await {
        warn!(%error, method = method.name(), "config reload failed");
    }
    true
}

/// Watch the config for an agent with no leader around it. Only reloads apply to it:
/// `run_leader` alone hot-swaps auth and relays UI changes.
/// Cancel `cancel` to stop the watcher.
pub fn spawn_config_reload(
    agent: Rc<MvpAgent>,
    agent_config: &AgentConfig,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let (watcher, mut updates) = start_config_reload(
        agent_config,
        &std::env::current_dir().unwrap_or_default(),
        recursive_config_watch_enabled(),
        cancel.clone(),
    );
    xai_spawn::spawn_local(async move {
        let _watcher = watcher;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                update = updates.recv() => match update {
                    Some(update) => {
                        if !apply_reload(&agent, &update).await {
                            debug!("config update is not a reload");
                        }
                    }
                    None => break,
                },
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method_of(update: &ConfigUpdate) -> Option<&'static str> {
        reload_request(update).map(|(method, _)| method.name())
    }

    #[test]
    fn a_model_change_reloads_the_model_tables() {
        assert_eq!(
            method_of(&ConfigUpdate::ModelsChanged),
            Some("example.invalid/internal/reload_models"),
        );
    }

    #[test]
    fn a_ui_change_is_not_a_reload() {
        assert_eq!(
            method_of(&ConfigUpdate::Ui {
                theme: None,
                yolo: false,
            }),
            None,
        );
    }
}
