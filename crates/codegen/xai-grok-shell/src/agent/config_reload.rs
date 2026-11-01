//! Config hot-reload, shared by the leader and the stdio agent.
//!
//! `config.toml` is the store of settings. `/settings` writes it and so does
//! an editor. This watcher is what carries either write into the running
//! agent.
use crate::agent::config::Config as AgentConfig;
use crate::config::reloader::{ConfigReloader, ConfigUpdate};
use crate::config::watcher::ConfigFileWatcher;
use crate::leader::protocol::{InternalMethod, internal_request_line};
use crate::util::grok_home;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{Mutex as TokioMutex, mpsc};
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
    tokio::spawn(reloader.run(events_rx, cancel));
    (Some(watcher), update_rx)
}

/// The internal request that applies `update` to the running agent, or `None` when the update
/// is not applied through the ACP stream.
pub(crate) fn reload_request_line(update: &ConfigUpdate) -> Option<String> {
    let (id, method, params) = match update {
        ConfigUpdate::McpServersChanged => (
            "config-reload-mcp",
            InternalMethod::ReloadAllMcpServers,
            serde_json::json!({}),
        ),
        ConfigUpdate::ProjectMcpServersChanged { cwd } => (
            "config-reload-project-mcp",
            InternalMethod::ReloadProjectMcpServers,
            serde_json::json!({ "cwd": cwd.to_string_lossy() }),
        ),
        ConfigUpdate::ModelsChanged => (
            "config-reload-models",
            InternalMethod::ReloadModels,
            serde_json::json!({}),
        ),
        ConfigUpdate::ModelsCacheChanged => (
            "config-reload-models-cache",
            InternalMethod::ReloadModelsCache,
            serde_json::json!({}),
        ),
        ConfigUpdate::OutputRateFloorChanged => (
            "config-reload-output-rate-floor",
            InternalMethod::ReloadOutputRateFloor,
            serde_json::json!({}),
        ),
        ConfigUpdate::ThinkingSummariesChanged => (
            "config-reload-thinking-summaries",
            InternalMethod::ReloadThinkingSummaries,
            serde_json::json!({}),
        ),
        _ => return None,
    };
    Some(internal_request_line(id, method, params))
}

/// Write `update`'s reload request into the agent's ACP stream.
/// Returns whether `update` was a reload.
pub(crate) async fn inject_reload<W>(acp_tx: &Arc<TokioMutex<W>>, update: &ConfigUpdate) -> bool
where
    W: AsyncWrite + Unpin,
{
    let Some(line) = reload_request_line(update) else {
        return false;
    };
    info!(line = line.trim_end(), "config change detected, reloading");
    let mut tx = acp_tx.lock().await;
    if let Err(error) = tx.write_all(line.as_bytes()).await {
        warn!(%error, "failed to inject config reload into ACP stream");
    }
    true
}

/// Apply config updates for an agent with no leader process around it, the TUI's in-process
/// agent included. Only reloads reach it: `run_leader` alone hot-swaps auth and relays UI changes.
pub(crate) fn spawn_reload_forwarder<W>(
    mut updates: mpsc::UnboundedReceiver<ConfigUpdate>,
    acp_tx: Arc<TokioMutex<W>>,
) where
    W: AsyncWrite + Unpin + 'static,
{
    tokio::task::spawn_local(async move {
        while let Some(update) = updates.recv().await {
            if !inject_reload(&acp_tx, &update).await {
                debug!("config update has no reload in a stdio agent");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method_of(update: &ConfigUpdate) -> Option<String> {
        let line = reload_request_line(update)?;
        let msg: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        Some(msg["method"].as_str().unwrap().to_string())
    }

    #[test]
    fn a_model_change_reloads_the_model_tables() {
        assert_eq!(
            method_of(&ConfigUpdate::ModelsChanged).as_deref(),
            Some("_x.ai/internal/reload_models"),
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

    #[tokio::test]
    async fn inject_writes_one_line_and_reports_a_reload() {
        let (mut read, write) = tokio::io::simplex(4096);
        let tx = Arc::new(TokioMutex::new(write));
        assert!(inject_reload(&tx, &ConfigUpdate::ModelsChanged).await);
        drop(tx);
        let mut text = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut read, &mut text)
            .await
            .unwrap();
        assert_eq!(text.matches('\n').count(), 1, "one request line: {text}");
        assert!(text.contains("reload_models"), "{text}");
    }
}
