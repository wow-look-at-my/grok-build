use agent_client_protocol as acp;
use anyhow::Result;
use indexmap::IndexMap;
use std::path::PathBuf;
use toml::Value as TomlValue;
use toml::map::Map as TomlMap;
use xai_grok_agent::prompt::skills::SkillsConfig;
use xai_grok_tools::types::compat::{CompatConfig, CompatConfigToml};

use xai_grok_config::ClaudeImport;
pub(crate) use xai_grok_config::mcp_servers::{
    MCP_SCOPE_PROJECT, McpEnabledFilter, get_mcp_server_config, load_mcp_json_file,
    load_mcp_preferences, materialize_mcp_config, parse_mcp_servers_from_toml,
};
use xai_grok_config::mcp_servers::{
    MCP_SCOPE_USER, McpPreferencesLoad, McpServerSources, load_cursor_mcp_servers_as_configs,
    load_mcp_preferences_from, mcp_preferences_path,
};
pub use xai_grok_config_types::{McpConfig, RelaySyncConfig};
pub use xai_grok_config_types::{
    McpJsonOAuthBlock, McpPreferenceSource, McpPreferencesFile, McpServerConfig,
    McpServerConfigProblem, McpServerPreferences, McpServerProblemSeverity,
    McpServerTransportConfig, McpSetupConfig, McpSetupDerivedValue, McpSetupField,
    McpSetupFieldType, McpSetupOption, McpSetupResolution,
};
pub use xai_grok_config_types::{
    PatternMode, PermissionConfig, PermissionRule, RuleAction, ToolFilter,
};
pub use xai_grok_mcp::oauth_config::{McpOAuthConfig, McpOAuthConfigMap};
pub use xai_grok_workspace::project_config::{
    MCP_JSON_FILENAME, find_mcp_json_files, mcp_json_candidate_paths,
};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub cli: crate::agent::config::CliConfig,
    pub models: crate::agent::config::ModelsConfig,
    pub ui: crate::agent::config::UiConfig,
    pub harness: crate::agent::config::HarnessConfig,
    pub skills: SkillsConfig,
    /// Round-tripped so the pager preserves per-vendor toggles when persisting other settings.
    pub compat: CompatConfigToml,
    /// From `[endpoints]`.
    pub management_api_key: Option<String>,
    pub permission: Option<PermissionConfig>,
    pub diagnostics: crate::agent::config::DiagnosticsConfig,
    /// Round-tripped through `merge_section` so pager setters can persist session fields.
    pub session: crate::agent::config::SessionConfig,
    /// `[toolset.ask_user_question]` sub-table, the only `[toolset]` piece the settings modal writes.
    /// The rest of `[toolset]` never round-trips (it carries runtime-only structs whose defaults must not hit disk).
    pub ask_user_question: crate::tools::config::AskUserQuestionToolConfig,
    pub privacy: PrivacyConfig,
    pub consent: super::consent::ConsentConfig,
    pub telemetry: TelemetryPersistConfig,
    pub features: FeaturesPersistConfig,
}

/// Unmodeled keys under `[telemetry]` are preserved by the deep merge in `save_config_locked`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TelemetryPersistConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_upload: Option<bool>,
}

/// Unmodeled keys under `[features]` are preserved by the deep merge in `save_config_locked`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FeaturesPersistConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback_trace_card: Option<bool>,
}

#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq, schemars::JsonSchema,
)]
pub struct PrivacyConfig {
    /// RFC 3339 UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_banner_acked: Option<String>,
}

pub(crate) fn get_mcp_server_config_with_project(
    name: &str,
    cwd: &std::path::Path,
) -> Option<McpServerConfig> {
    xai_grok_config::mcp_servers::get_mcp_server_config_with_project(
        name,
        &crate::config::find_project_configs(cwd),
    )
}

pub(crate) fn mcp_server_scope(name: &str, cwd: &std::path::Path) -> &'static str {
    xai_grok_config::mcp_servers::mcp_server_scope(name, &crate::config::find_project_configs(cwd))
}

<<<<<<< fork
/// Load MCP servers and their OAuth configurations from config.json.
||||||| base
/// Load MCP servers and their OAuth configurations from config.toml.
=======
>>>>>>> upstream
pub(crate) fn load_mcp_servers_with_oauth(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> (Vec<acp::McpServer>, McpOAuthConfigMap) {
    xai_grok_config::mcp_servers::load_mcp_servers_with_oauth(&mcp_server_sources(
        cwd,
        compat,
        "load_mcp_servers_with_oauth",
    ))
}

<<<<<<< fork
/// Load MCP servers with project-scoped overrides from `.grok/config.json`. Merge strategy: Load MCP servers from global `~/.grok/config.json`
/// Walk from git repo root down to `cwd`, loading `.grok/config.json` at each level (matching skills and AGENTS.md discovery)
/// Each level's entries replace entries with the same name entirely (no deep merge; omitted fields fall back to defaults) Closer directories (cwd) take priority over further ones (repo root)
||||||| base
/// Load MCP servers with project-scoped overrides from `.grok/config.toml`. Merge strategy: Load MCP servers from global `~/.grok/config.toml`
/// Walk from git repo root down to `cwd`, loading `.grok/config.toml` at each level (matching skills and AGENTS.md discovery)
/// Each level's entries replace entries with the same name entirely (no deep merge; omitted fields fall back to defaults) Closer directories (cwd) take priority over further ones (repo root)
=======
>>>>>>> upstream
pub fn load_mcp_servers(cwd: &std::path::Path, compat: &CompatConfig) -> Vec<acp::McpServer> {
<<<<<<< fork
    let global_config = crate::config::load_effective_config()
        .unwrap_or_else(|_| TomlValue::Table(toml::map::Map::new()));
    reload_mcp_servers_merged(&global_config, cwd, compat)
}

/// Load MCP servers from config.json only (global and project-scoped).
/// Used by [`crate::session::managed_mcp::merge_managed_mcp_servers_sourced`]; it handles those sources separately with `ConfigSource` tracking.
/// Using [`load_mcp_servers`] there would cause all entries to be tagged as `ConfigSource::ConfigToml`, hiding the true origin.
pub(crate) fn load_mcp_servers_toml_only(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    let preferences = load_mcp_preferences().file();
    let sub = &crate::config::expand_env_vars_in_string;
    load_all_mcp_configs(cwd)
        .into_iter()
        .filter_map(|(name, config)| {
            materialize_mcp_config(&name, config, &preferences, sub, McpEnabledFilter::Respect)
        })
        .collect()
}

/// Whether materialization respects the config `enabled` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpEnabledFilter {
    /// Drop when `enabled = false` (live merge / load).
    Respect,
    /// Materialize even when `enabled = false` (list stubs / reenable probe).
    Ignore,
}

/// Resolve setup, expand strings, and convert to ACP.
pub(crate) fn materialize_mcp_config(
    name: &str,
    mut config: McpServerConfig,
    preferences: &McpPreferencesFile,
    sub: &dyn Fn(&str) -> String,
    enabled_filter: McpEnabledFilter,
) -> Option<acp::McpServer> {
    if matches!(enabled_filter, McpEnabledFilter::Ignore) {
        config.enabled = true;
    }
    let mut config = match config.resolve_setup(preferences.servers.get(name)) {
        McpSetupResolution::Resolved(config) => config,
        McpSetupResolution::Required(_) => return None,
        McpSetupResolution::Invalid(reason) => {
            tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
            return None;
        }
    };
    config.expand_strings(sub);
    config.to_acp_mcp_server(name)
||||||| base
    let global_config = crate::config::load_effective_config()
        .unwrap_or_else(|_| TomlValue::Table(toml::map::Map::new()));
    reload_mcp_servers_merged(&global_config, cwd, compat)
}

/// Load MCP servers from config.toml only (global and project-scoped). Skips the `~/.claude.json`, `~/.cursor/mcp.json`, and `.mcp.json` sources.
/// Used by [`crate::session::managed_mcp::merge_managed_mcp_servers_sourced`]; it handles those sources separately with `ConfigSource` tracking.
/// Using [`load_mcp_servers`] there would cause all entries to be tagged as `ConfigSource::ConfigToml`, hiding the true origin.
pub(crate) fn load_mcp_servers_toml_only(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    let preferences = load_mcp_preferences().file();
    let sub = &crate::config::expand_env_vars_in_string;
    load_all_mcp_configs(cwd)
        .into_iter()
        .filter_map(|(name, config)| {
            materialize_mcp_config(&name, config, &preferences, sub, McpEnabledFilter::Respect)
        })
        .collect()
}

/// Whether materialization respects the config `enabled` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpEnabledFilter {
    /// Drop when `enabled = false` (live merge / load).
    Respect,
    /// Materialize even when `enabled = false` (list stubs / reenable probe).
    Ignore,
}

/// Resolve setup, expand strings, and convert to ACP.
pub(crate) fn materialize_mcp_config(
    name: &str,
    mut config: McpServerConfig,
    preferences: &McpPreferencesFile,
    sub: &dyn Fn(&str) -> String,
    enabled_filter: McpEnabledFilter,
) -> Option<acp::McpServer> {
    if matches!(enabled_filter, McpEnabledFilter::Ignore) {
        config.enabled = true;
    }
    let mut config = match config.resolve_setup(preferences.servers.get(name)) {
        McpSetupResolution::Resolved(config) => config,
        McpSetupResolution::Required(_) => return None,
        McpSetupResolution::Invalid(reason) => {
            tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
            return None;
        }
    };
    config.expand_strings(sub);
    config.to_acp_mcp_server(name)
=======
    xai_grok_config::mcp_servers::load_mcp_servers(&mcp_server_sources(
        cwd,
        compat,
        "load_mcp_servers",
    ))
>>>>>>> upstream
}

pub(crate) fn mcp_server_sources(
    cwd: &std::path::Path,
    compat: &CompatConfig,
<<<<<<< fork
) -> Vec<acp::McpServer> {
    // The one shared TOML walk (see `toml_mcp_server_configs_from`): the
    // merge classifies from the same set, so discovery and policy can't split.
    let mut servers: IndexMap<String, McpServerConfig> =
        toml_mcp_server_configs_from(global_config, cwd)
            .into_iter()
            .map(|(name, (config, _scope))| (name, config))
            .collect();

    // Also load from ~/.claude.json (lower priority than TOML)
    let claude_servers = load_claude_json_mcp_servers_as_configs(cwd, compat);
    tracing::info!(
        count = claude_servers.len(),
        "Loaded MCP servers from ~/.claude.json"
    );
    for (name, config) in claude_servers {
        servers.entry(name).or_insert(config);
    }

    // Also load from ~/.cursor/mcp.json (lower priority than TOML and ~/.claude.json)
    let cursor_servers = load_cursor_mcp_servers_as_configs(cwd, compat);
    tracing::info!(
        count = cursor_servers.len(),
        "Loaded Cursor MCP servers from ~/.cursor/mcp.json"
    );
    for (name, config) in cursor_servers {
        servers.entry(name).or_insert(config);
    }

    // Also load from .mcp.json files (lower priority than TOML)
    let mcp_json_servers = load_mcp_json_servers_as_configs(cwd);
    tracing::info!(
        count = mcp_json_servers.len(),
        "Loaded .mcp.json MCP servers"
    );
    for (name, config) in mcp_json_servers {
        servers.entry(name).or_insert(config);
    }

    let preferences = load_mcp_preferences().file();
    let sub = &crate::config::expand_env_vars_in_string;
    servers
        .into_iter()
        .filter_map(|(name, config)| {
            let mut config = match config.resolve_setup(preferences.servers.get(&name)) {
                McpSetupResolution::Resolved(config) => config,
                McpSetupResolution::Required(_) => return None,
                McpSetupResolution::Invalid(reason) => {
                    tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
                    return None;
                }
            };
            config.expand_strings(sub);
            config.to_acp_mcp_server(name)
        })
        .collect()
}

/// Load `.mcp.json` servers from repo root to `cwd` (closest wins on name conflict).
pub(crate) fn load_mcp_json_servers(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    // Phase 2 cutoff: if the user has imported, skip reading .mcp.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_mcp_json_servers") {
        return vec![];
    }

    let mcp_json_files = find_mcp_json_files(cwd);
    if mcp_json_files.is_empty() {
        return vec![];
    }

    let mut result = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Reverse so cwd entries win on name conflict.
    for mcp_path in mcp_json_files.iter().rev() {
        let json_servers = load_mcp_json_file(mcp_path);
        for server in json_servers {
            let name = match &server {
                acp::McpServer::Http(acp::McpServerHttp { name, .. })
                | acp::McpServer::Sse(acp::McpServerSse { name, .. })
                | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
                // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
                _ => continue,
            };
            if seen_names.insert(name) {
                result.push(server);
            }
        }
    }

    result
}

/// All server names from config.json (including `enabled = false`).
pub(crate) fn all_toml_mcp_server_names(
    cwd: &std::path::Path,
) -> std::collections::HashSet<String> {
    load_all_mcp_configs(cwd).keys().cloned().collect()
}

pub(crate) fn mcp_preferences_path() -> PathBuf {
    xai_grok_config::grok_home().join("mcp_preferences.json")
}

/// Corrupt files are readable as empty for resolution but must not be overwritten (would clobber other servers).
#[derive(Debug, Clone)]
pub(crate) enum McpPreferencesLoad {
    Ok(McpPreferencesFile),
    Missing,
    Corrupt,
}

impl McpPreferencesLoad {
    pub(crate) fn file(&self) -> McpPreferencesFile {
        match self {
            Self::Ok(f) => f.clone(),
            Self::Missing | Self::Corrupt => McpPreferencesFile::default(),
        }
    }

    pub(crate) fn is_writable(&self) -> bool {
        !matches!(self, Self::Corrupt)
||||||| base
) -> Vec<acp::McpServer> {
    // The one shared TOML walk (see `toml_mcp_server_configs_from`): the
    // merge classifies from the same set, so discovery and policy can't split.
    let mut servers: IndexMap<String, McpServerConfig> =
        toml_mcp_server_configs_from(global_config, cwd)
            .into_iter()
            .map(|(name, (config, _scope))| (name, config))
            .collect();

    // Also load from ~/.claude.json (lower priority than TOML)
    let claude_servers = load_claude_json_mcp_servers_as_configs(cwd, compat);
    tracing::info!(
        count = claude_servers.len(),
        "Loaded MCP servers from ~/.claude.json"
    );
    for (name, config) in claude_servers {
        servers.entry(name).or_insert(config);
    }

    // Also load from ~/.cursor/mcp.json (lower priority than TOML and ~/.claude.json)
    let cursor_servers = load_cursor_mcp_servers_as_configs(cwd, compat);
    tracing::info!(
        count = cursor_servers.len(),
        "Loaded Cursor MCP servers from ~/.cursor/mcp.json"
    );
    for (name, config) in cursor_servers {
        servers.entry(name).or_insert(config);
    }

    // Also load from .mcp.json files (lower priority than TOML)
    let mcp_json_servers = load_mcp_json_servers_as_configs(cwd);
    tracing::info!(
        count = mcp_json_servers.len(),
        "Loaded .mcp.json MCP servers"
    );
    for (name, config) in mcp_json_servers {
        servers.entry(name).or_insert(config);
    }

    let preferences = load_mcp_preferences().file();
    let sub = &crate::config::expand_env_vars_in_string;
    servers
        .into_iter()
        .filter_map(|(name, config)| {
            let mut config = match config.resolve_setup(preferences.servers.get(&name)) {
                McpSetupResolution::Resolved(config) => config,
                McpSetupResolution::Required(_) => return None,
                McpSetupResolution::Invalid(reason) => {
                    tracing::warn!(server = %name, error = %reason, "MCP setup config is invalid");
                    return None;
                }
            };
            config.expand_strings(sub);
            config.to_acp_mcp_server(name)
        })
        .collect()
}

/// Load `.mcp.json` servers from repo root to `cwd` (closest wins on name conflict).
pub(crate) fn load_mcp_json_servers(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    // Phase 2 cutoff: if the user has imported, skip reading .mcp.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_mcp_json_servers") {
        return vec![];
    }

    let mcp_json_files = find_mcp_json_files(cwd);
    if mcp_json_files.is_empty() {
        return vec![];
    }

    let mut result = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Reverse so cwd entries win on name conflict.
    for mcp_path in mcp_json_files.iter().rev() {
        let json_servers = load_mcp_json_file(mcp_path);
        for server in json_servers {
            let name = match &server {
                acp::McpServer::Http(acp::McpServerHttp { name, .. })
                | acp::McpServer::Sse(acp::McpServerSse { name, .. })
                | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
                // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
                _ => continue,
            };
            if seen_names.insert(name) {
                result.push(server);
            }
        }
    }

    result
}

/// All server names from config.toml (including `enabled = false`).
pub(crate) fn all_toml_mcp_server_names(
    cwd: &std::path::Path,
) -> std::collections::HashSet<String> {
    load_all_mcp_configs(cwd).keys().cloned().collect()
}

pub(crate) fn mcp_preferences_path() -> PathBuf {
    xai_grok_config::grok_home().join("mcp_preferences.json")
}

/// Corrupt files are readable as empty for resolution but must not be overwritten (would clobber other servers).
#[derive(Debug, Clone)]
pub(crate) enum McpPreferencesLoad {
    Ok(McpPreferencesFile),
    Missing,
    Corrupt,
}

impl McpPreferencesLoad {
    pub(crate) fn file(&self) -> McpPreferencesFile {
        match self {
            Self::Ok(f) => f.clone(),
            Self::Missing | Self::Corrupt => McpPreferencesFile::default(),
        }
    }

    pub(crate) fn is_writable(&self) -> bool {
        !matches!(self, Self::Corrupt)
=======
    caller: &'static str,
) -> McpServerSources {
    McpServerSources {
        cwd: cwd.to_path_buf(),
        project_configs: crate::config::find_project_configs(cwd),
        mcp_json_files: find_mcp_json_files(cwd),
        compat: *compat,
        claude_import: claude_import(caller),
>>>>>>> upstream
    }
}

pub(crate) fn load_mcp_servers_toml_only(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    xai_grok_config::mcp_servers::load_mcp_servers_toml_only(&crate::config::find_project_configs(
        cwd,
    ))
}

pub(crate) fn load_mcp_json_servers(cwd: &std::path::Path) -> Vec<acp::McpServer> {
    xai_grok_config::mcp_servers::load_mcp_json_servers(
        &find_mcp_json_files(cwd),
        claude_import("load_mcp_json_servers"),
    )
}

pub(crate) async fn save_mcp_preferences(prefs: &McpPreferencesFile) -> Result<()> {
    save_mcp_preferences_to(&mcp_preferences_path(), prefs).await
}

pub(crate) async fn save_mcp_preferences_to(
    path: &std::path::Path,
    prefs: &McpPreferencesFile,
) -> Result<()> {
    if matches!(load_mcp_preferences_from(path), McpPreferencesLoad::Corrupt) {
        anyhow::bail!(
            "refusing to overwrite unreadable MCP preferences at {}",
            path.display()
        );
    }
    let json = serde_json::to_string_pretty(prefs)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Setup values may hold secrets: owner-only from creation.
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        xai_grok_config::fs_atomic::write_user_file_atomically(&path, &json, Some(0o600))
    })
    .await?
    .map_err(|e| anyhow::anyhow!("failed to write mcp preferences: {e}"))?;
    Ok(())
}

pub(crate) async fn restore_mcp_preference_server(
    server_name: &str,
    previous: Option<McpServerPreferences>,
) -> Result<()> {
    let load = load_mcp_preferences();
    if !load.is_writable() {
        // Error, not Ok: the caller's "no state left behind" contract failed
        // and it must at least log that the saved values were kept.
        anyhow::bail!("MCP preferences file is unwritable; saved setup values were not restored");
    }
    let mut prefs = load.file();
    match previous {
        Some(entry) => {
            prefs.servers.insert(server_name.to_string(), entry);
        }
        None => {
            prefs.servers.remove(server_name);
        }
    }
    save_mcp_preferences(&prefs).await
}

#[derive(Debug, Clone)]
pub struct McpSetupServerEntry {
    pub name: String,
    pub config: McpServerConfig,
    pub source: McpPreferenceSource,
}

<<<<<<< fork
/// Collect MCP configs that declare a `setup` schema from config and plugins. Used to show setup-required rows and drive `example.invalid/mcp/setup`.
/// User/project **TOML** includes `enabled = false` so Space-disabled setup servers stay visible. (`handle_list` derives `session.enabled` from `disabled_mcp_servers`.)
/// Other sources still skip native `enabled = false` because Space cannot unstick those flags.
||||||| base
/// Collect MCP configs that declare a `setup` schema from config and plugins. Used to show setup-required rows and drive `x.ai/mcp/setup`.
/// User/project **TOML** includes `enabled = false` so Space-disabled setup servers stay visible. (`handle_list` derives `session.enabled` from `disabled_mcp_servers`.)
/// Other sources still skip native `enabled = false` because Space cannot unstick those flags.
=======
pub(crate) fn plugin_mcp_servers(
    plugin: &xai_grok_agent::plugins::LoadedPlugin,
) -> xai_grok_config::mcp_servers::PluginMcpServers {
    xai_grok_config::mcp_servers::PluginMcpServers {
        plugin_name: plugin.name.clone(),
        root: plugin.root.clone(),
        data_dir: plugin.data_dir(),
        scope: xai_grok_config::mcp_servers::McpServerScope::from(plugin.scope),
        mcp_config_path: plugin.mcp_config_path.clone(),
        inline_mcp_servers: plugin.inline_mcp_servers.clone(),
    }
}

/// User and project TOML include `enabled = false` so Space-disabled setup servers stay visible.
/// Other sources skip `enabled = false` because Space cannot unstick those flags.
>>>>>>> upstream
pub(crate) fn collect_mcp_setup_configs(
    cwd: &std::path::Path,
    plugin_registry: Option<&xai_grok_agent::plugins::PluginRegistry>,
    compat: &CompatConfig,
) -> IndexMap<String, McpSetupServerEntry> {
    let mut result = IndexMap::new();
    for (name, (config, scope)) in load_mcp_server_configs_with_project(cwd) {
        if config.setup.is_none() {
            continue;
        }
        result.insert(
            name.clone(),
            McpSetupServerEntry {
                name,
                config,
                source: McpPreferenceSource {
                    kind: "config".to_string(),
                    plugin: None,
                    scope: Some(scope.to_string()),
                },
            },
        );
    }

    let claude_import = claude_import("collect_mcp_setup_configs");
    if claude_import == ClaudeImport::NotImported {
        insert_compat_setup_configs(
            &mut result,
            xai_grok_config::mcp_servers::load_claude_json_mcp_servers_as_configs(
                cwd,
                compat,
                claude_import,
            ),
            MCP_SCOPE_USER,
        );
        insert_compat_setup_configs(
            &mut result,
            load_cursor_mcp_servers_as_configs(cwd, compat),
            MCP_SCOPE_USER,
        );
        insert_compat_setup_configs(
            &mut result,
            xai_grok_config::mcp_servers::load_mcp_json_servers_as_configs(
                &find_mcp_json_files(cwd),
                claude_import,
            ),
            MCP_SCOPE_PROJECT,
        );
    }

    if let Some(registry) = plugin_registry {
        let toml_claimed_names = all_toml_mcp_server_names(cwd);
        for plugin in registry.active_plugins() {
            let plugin_configs = xai_grok_config::mcp_servers::plugin_setup_server_configs(
                &plugin_mcp_servers(plugin),
            );
            for (name, config) in plugin_configs {
                if toml_claimed_names.contains(&name) || !config.enabled || config.setup.is_none() {
                    continue;
                }
                result.entry(name.clone()).or_insert(McpSetupServerEntry {
                    name,
                    config,
                    source: McpPreferenceSource {
                        kind: "plugin".to_string(),
                        plugin: Some(plugin.name.clone()),
                        scope: None,
                    },
                });
            }
        }
    }
    result
}

fn insert_compat_setup_configs(
    result: &mut IndexMap<String, McpSetupServerEntry>,
    configs: IndexMap<String, McpServerConfig>,
    scope: &'static str,
) {
    for (name, config) in configs {
        if !config.enabled || config.setup.is_none() {
            continue;
        }
        result.entry(name.clone()).or_insert(McpSetupServerEntry {
            name,
            config,
            source: McpPreferenceSource {
                kind: "config".to_string(),
                plugin: None,
                scope: Some(scope.to_string()),
            },
        });
    }
}

fn claude_import(caller: &'static str) -> ClaudeImport {
    ClaudeImport::from_marker(crate::claude_import::is_claude_import_marked_with_log(
        caller,
    ))
}

pub const MANAGED_GATEWAY_DISABLED_CONNECTORS_KEY: &str = "__managed_gateway_connectors";

<<<<<<< fork
/// Persist `disabled_tools` for a server under `mcp.disabled_tools` in config.json.
///
||||||| base
/// Persist `disabled_tools` for a server under `[disabled_mcp_tools]` in config.toml.
///
=======
>>>>>>> upstream
/// Uses a dedicated top-level section (not `[mcp_servers]`) to avoid creating incomplete server entries that fail to deserialize for managed servers.
pub(crate) async fn save_mcp_disabled_tools(
    server_name: &str,
    disabled_tools: &[String],
) -> Result<()> {
    let name = server_name.to_string();
    let disabled_tools = disabled_tools.to_vec();
    rmw_mcp_toml(
        &config_path(),
        McpMissing::EmptyTable,
        false,
        move |table| {
            let section = table
                .entry("disabled_mcp_tools")
                .or_insert_with(|| TomlValue::Table(TomlMap::new()))
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("`mcp.disabled_tools` is not an object"))?;

            if disabled_tools.is_empty() {
                section.remove(&name);
                if section.is_empty() {
                    table.remove("disabled_mcp_tools");
                }
            } else {
                let arr = disabled_tools
                    .iter()
                    .map(|s| TomlValue::String(s.clone()))
                    .collect();
                section.insert(name, TomlValue::Array(arr));
            }
            Ok(true)
        },
    )
    .await?;
    Ok(())
}

/// On enable, if the project unstick fails after a successful user-tier write, this rolls back whatever was already written and returns the error.
pub async fn save_mcp_server_enabled_in(
    server_name: &str,
    enabled: bool,
    cwd: &std::path::Path,
) -> Result<Vec<PathBuf>> {
    let mut modified = Vec::new();

    let user_path = config_path();
    let name = server_name.to_string();
    if write_toml_table_if_changed(&user_path, move |table| {
        apply_mcp_server_enabled(table, &name, enabled);
    })
    .await?
    {
        modified.push(user_path);
    }

<<<<<<< fork
    // Enable-only: unstick the nearest (winning) project def with `enabled = false`
||||||| base
    // Enable-only: unstick the nearest (winning) project def with `enabled = false` via toml_edit (preserves comments/layout)
=======
>>>>>>> upstream
    // Disable is personal (user `disabled_mcp_servers`) and must not dirty shared files
    if enabled && let Some(path) = nearest_project_mcp_definition(cwd, server_name) {
        match clear_sticky_project_disabled_at(&path, server_name).await {
            Ok(true) => modified.push(path),
            Ok(false) => {}
            Err(e) => {
                if let Err(re) =
                    restore_mcp_server_enabled_after_enable(server_name, &modified).await
                {
                    tracing::warn!(
                        server = server_name,
                        error = %re,
                        "failed to roll back partial enable after project unstick error"
                    );
                }
                return Err(e);
            }
        }
    }

    Ok(modified)
}

pub(crate) async fn save_user_mcp_server_enabled(server_name: &str, enabled: bool) -> Result<()> {
    let name = server_name.to_string();
    write_toml_table_if_changed(&config_path(), move |table| {
        apply_mcp_server_enabled(table, &name, enabled);
    })
    .await
    .map(|_| ())
}

<<<<<<< fork
/// Undo a prior [`save_mcp_server_enabled_in`]`(…, true, …)` using the paths that call returned. Restores only the tiers that were written. Restores an **equivalent** disabled state, not necessarily the original encoding.
/// User-tier restore always goes through [`save_user_mcp_server_enabled`]`(…, false)` (personal `disabled_mcp_servers`). Project-tier restore re-sticks `enabled = false`.
/// A server disabled only via a sticky project field may therefore pick up a personal list entry if the user tier was also written during enable.
||||||| base
/// Undo a prior [`save_mcp_server_enabled_in`]`(…, true, …)` using the paths that call returned. Restores only the tiers that were written. Restores an **equivalent** disabled state, not necessarily the original encoding.
/// User-tier restore always goes through [`save_user_mcp_server_enabled`]`(…, false)` (personal `disabled_mcp_servers`). Project-tier restore re-sticks `enabled = false` via toml_edit.
/// A server disabled only via a sticky project field may therefore pick up a personal list entry if the user tier was also written during enable.
=======
/// Restores an equivalent disabled state, not necessarily the original encoding: a server disabled only
/// by a sticky project field may pick up a personal `disabled_mcp_servers` entry.
>>>>>>> upstream
pub(crate) async fn restore_mcp_server_enabled_after_enable(
    server_name: &str,
    modified_paths: &[PathBuf],
) -> Result<()> {
    let user_path = config_path();
    for path in modified_paths {
        if path == user_path.as_path() {
            save_user_mcp_server_enabled(server_name, false).await?;
        } else {
            set_sticky_project_disabled_at(path, server_name).await?;
        }
    }
    Ok(())
}

/// `find_project_configs` lists the cwd last, and the nearest definition wins.
fn nearest_project_mcp_definition(cwd: &std::path::Path, server_name: &str) -> Option<PathBuf> {
    crate::config::find_project_configs(cwd)
        .into_iter()
        .rev()
        .find(|path| mcp_server_defined_at(path, server_name))
}

<<<<<<< fork
/// User `$GROK_HOME/config.json`. [`config_path`] is live (`grok_home()` is OnceLock).
||||||| base
/// User `$GROK_HOME/config.toml`. [`config_path`] is live (`grok_home()` is OnceLock).
=======
>>>>>>> upstream
fn is_user_config_path(path: &std::path::Path) -> bool {
    path == config_path().as_path()
}

#[derive(Clone, Copy)]
enum McpMissing {
    EmptyTable,
    Skip,
}

fn parse_mcp_root(path: &std::path::Path, original: &str) -> Result<TomlValue> {
    super::persist::parse_existing_config(path, original).map_err(|parse_err| {
        anyhow::anyhow!(
            "refusing to overwrite unparseable {}: {}; fix the syntax before retrying",
            path.display(),
            parse_err
        )
    })
}

fn apply_mcp_rmw(
    path: &std::path::Path,
    original: &str,
    f: impl FnOnce(&mut TomlMap<String, TomlValue>) -> Result<bool>,
    skip_if_unchanged: bool,
    publish: impl FnOnce(&str) -> std::io::Result<()>,
) -> Result<bool> {
    let mut root = parse_mcp_root(path, original)?;
    let before = root.clone();
    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root is not an object"))?;
    if !f(table)? {
        return Ok(false);
    }
    if skip_if_unchanged && before == root {
        return Ok(false);
    }
    let text = super::persist::render_existing_config(path, &root)?;
    publish(&text).map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(true)
}

/// The JSON file to write for a config at `path`. A legacy project `config.toml` converts to `config.json` first.
fn writable_config_path(path: &std::path::Path) -> Result<PathBuf> {
    if xai_grok_config::format::ConfigFormat::of(path)
        == xai_grok_config::format::ConfigFormat::Json
    {
        return Ok(path.to_path_buf());
    }
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) else {
        return Err(anyhow::anyhow!("cannot convert {} to JSON", path.display()));
    };
    let converted = xai_grok_config::format::migrate_legacy_config(dir, stem, None)
        .map_err(|e| anyhow::anyhow!("convert {} to JSON before the edit: {e}", path.display()))?;
    if xai_grok_config::format::ConfigFormat::of(&converted)
        != xai_grok_config::format::ConfigFormat::Json
    {
        return Err(anyhow::anyhow!(
            "could not write {}; convert it to JSON first",
            path.with_extension("json").display()
        ));
    }
    Ok(converted)
}

fn rmw_mcp_toml_user(
    path: &std::path::Path,
    f: impl FnOnce(&mut TomlMap<String, TomlValue>) -> Result<bool>,
    skip_if_unchanged: bool,
) -> Result<bool> {
    let (dest, original) = super::persist::read_follow_bound(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    apply_mcp_rmw(path, &original, f, skip_if_unchanged, |s| {
        super::persist::atomic_write_follow_bound(path, &dest, s)
    })
}

fn rmw_mcp_toml_project(
    path: &std::path::Path,
    missing: McpMissing,
    f: impl FnOnce(&mut TomlMap<String, TomlValue>) -> Result<bool>,
    skip_if_unchanged: bool,
) -> Result<bool> {
    if !path.exists() && matches!(missing, McpMissing::Skip) {
        return Ok(false);
    }
    let path = &writable_config_path(path)?;
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match missing {
            McpMissing::Skip => return Ok(false),
            McpMissing::EmptyTable => String::new(),
        },
        Err(e) => {
            return Err(anyhow::anyhow!("failed to read {}: {e}", path.display()));
        }
    };
    apply_mcp_rmw(path, &original, f, skip_if_unchanged, |s| {
        super::persist::atomic_replace_string(path, s)
    })
}

async fn rmw_mcp_toml(
    path: &std::path::Path,
    missing: McpMissing,
    skip_if_unchanged: bool,
    f: impl FnOnce(&mut TomlMap<String, TomlValue>) -> Result<bool> + Send + 'static,
) -> Result<bool> {
    let is_user = is_user_config_path(path);
    let path = path.to_path_buf();
    if is_user {
        let guard = super::persist::lock_config_writes().await?;
        guard
            .run_blocking(move || rmw_mcp_toml_user(&path, f, skip_if_unchanged))
            .await
            .map_err(|e| anyhow::anyhow!("config write task failed: {e}"))?
    } else {
        xai_spawn::spawn_blocking(move || {
            rmw_mcp_toml_project(&path, missing, f, skip_if_unchanged)
        })
        .await
        .map_err(|e| anyhow::anyhow!("config write task failed: {e}"))?
    }
}

/// Production MCP saves go through [`rmw_mcp_toml`]; this remains for the
/// follow-symlink regression that pins the one-shot path.
#[cfg(test)]
async fn persist_mcp_toml(path: &std::path::Path, toml_str: &str) -> Result<()> {
    let is_user = is_user_config_path(path);
    let path = path.to_path_buf();
    let toml_str = toml_str.to_string();
    if is_user {
        let guard = super::persist::lock_config_writes().await?;
        guard
            .run_blocking(move || {
                super::persist::atomic_write_string(&path, &toml_str)
                    .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))
            })
            .await
            .map_err(|e| anyhow::anyhow!("config write task failed: {e}"))?
    } else {
        xai_spawn::spawn_blocking(move || {
            super::persist::atomic_replace_string(&path, &toml_str)
                .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))
        })
        .await
        .map_err(|e| anyhow::anyhow!("config write task failed: {e}"))?
    }
}

async fn write_toml_table_if_changed(
    path: &std::path::Path,
    f: impl FnOnce(&mut TomlMap<String, TomlValue>) + Send + 'static,
) -> Result<bool> {
    let missing = if is_user_config_path(path) {
        McpMissing::EmptyTable
    } else {
        McpMissing::Skip
    };
    rmw_mcp_toml(path, missing, true, move |table| {
        f(table);
        Ok(true)
    })
    .await
}

<<<<<<< fork
/// Flip sticky project `enabled = false` to true.
||||||| base
/// Flip sticky project `enabled = false` to true with toml_edit (comments kept).
=======
/// Uses `toml_edit` so the shared project file keeps its comments and layout.
>>>>>>> upstream
async fn clear_sticky_project_disabled_at(
    path: &std::path::Path,
    server_name: &str,
) -> Result<bool> {
    flip_sticky_project_enabled_at(path, server_name, false, true).await
}

<<<<<<< fork
/// Flip sticky project `enabled = true` to false.
/// Inverse of [`clear_sticky_project_disabled_at`].
||||||| base
/// Flip sticky project `enabled = true` to false with toml_edit (comments kept).
/// Inverse of [`clear_sticky_project_disabled_at`].
=======
>>>>>>> upstream
async fn set_sticky_project_disabled_at(path: &std::path::Path, server_name: &str) -> Result<bool> {
    flip_sticky_project_enabled_at(path, server_name, true, false).await
}

/// Set `mcp_servers.<name>.enabled` to `to` in a project config when it is `from` now. Returns whether the file changed.
async fn flip_sticky_project_enabled_at(
    path: &std::path::Path,
    server_name: &str,
    from: bool,
    to: bool,
) -> Result<bool> {
    let name = server_name.to_string();
    rmw_mcp_toml(path, McpMissing::Skip, true, move |table| {
        let Some(server) = table
            .get_mut("mcp_servers")
            .and_then(TomlValue::as_table_mut)
            .and_then(|servers| servers.get_mut(&name))
            .and_then(TomlValue::as_table_mut)
        else {
            return Ok(false);
        };
        if server.get("enabled").and_then(TomlValue::as_bool) != Some(from) {
            return Ok(false);
        }
        server.insert("enabled".to_string(), TomlValue::Boolean(to));
        Ok(true)
    })
    .await
}

fn apply_mcp_server_enabled(
    table: &mut TomlMap<String, TomlValue>,
    server_name: &str,
    enabled: bool,
) {
    let mut disabled_list: Vec<String> = table
        .get("disabled_mcp_servers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if enabled {
        disabled_list.retain(|n| n != server_name);
    } else if !disabled_list.contains(&server_name.to_string()) {
        disabled_list.push(server_name.to_string());
    }

    if disabled_list.is_empty() {
        table.remove("disabled_mcp_servers");
    } else {
        let arr = disabled_list
            .iter()
            .map(|s| TomlValue::String(s.clone()))
            .collect();
        table.insert("disabled_mcp_servers".to_string(), TomlValue::Array(arr));
    }

    set_mcp_server_enabled_field(table, server_name, enabled);
}

fn set_mcp_server_enabled_field(
    table: &mut TomlMap<String, TomlValue>,
    server_name: &str,
    enabled: bool,
) {
    if let Some(servers) = table.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
        && let Some(entry) = servers.get_mut(server_name)
        && let Some(server_table) = entry.as_table_mut()
    {
        server_table.insert("enabled".to_string(), TomlValue::Boolean(enabled));
    }
}

<<<<<<< fork
/// Upsert an MCP server entry in `~/.grok/config.json`.
/// Also removes the server from `disabled_mcp_servers` if present (a newly defined server should start enabled).
||||||| base
/// Upsert an MCP server entry in `~/.grok/config.toml`.
/// Also removes the server from `disabled_mcp_servers` if present (a newly defined server should start enabled).
=======
/// Also removes the server from `disabled_mcp_servers`, so a newly defined server starts enabled.
>>>>>>> upstream
pub(crate) async fn save_mcp_server_config(
    server_name: &str,
    config: &McpServerConfig,
) -> Result<()> {
    save_mcp_server_config_at(&config_path(), server_name, config).await
}

<<<<<<< fork
/// Upsert an MCP server entry in the config file at `path`.
///
/// project-scoped `.grok/config.json`.
||||||| base
/// Upsert an MCP server entry in the config file at `path`.
///
/// Same behavior as [`save_mcp_server_config`] but targets an explicit config file, e.g. a project-scoped `.grok/config.toml`.
=======
>>>>>>> upstream
pub async fn save_mcp_server_config_at(
    path: &std::path::Path,
    server_name: &str,
    config: &McpServerConfig,
) -> Result<()> {
    let name = server_name.to_string();
    let serialized = toml::Value::try_from(config)
        .map_err(|e| anyhow::anyhow!("failed to serialize MCP server config: {e}"))?;
    rmw_mcp_toml(path, McpMissing::EmptyTable, false, move |table| {
        let servers = table
            .entry("mcp_servers")
            .or_insert_with(|| TomlValue::Table(TomlMap::new()))
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("`mcp.servers` is not an object"))?;
        servers.insert(name.clone(), serialized);

        if let Some(arr) = table
            .get_mut("disabled_mcp_servers")
            .and_then(|v| v.as_array_mut())
        {
            arr.retain(|v| v.as_str() != Some(name.as_str()));
            if arr.is_empty() {
                table.remove("disabled_mcp_servers");
            }
        }
        Ok(true)
    })
    .await?;
    Ok(())
}

<<<<<<< fork
/// Delete an MCP server entry from `~/.grok/config.json`.
/// Removes `[mcp_servers.<name>]`, cleans up `disabled_mcp_servers` and `[disabled_mcp_tools.<name>]` entries.
/// Returns `true` if the entry existed.
||||||| base
/// Delete an MCP server entry from `~/.grok/config.toml`.
/// Removes `[mcp_servers.<name>]`, cleans up `disabled_mcp_servers` and `[disabled_mcp_tools.<name>]` entries.
/// Returns `true` if the entry existed.
=======
>>>>>>> upstream
pub(crate) async fn delete_mcp_server_config(server_name: &str) -> Result<bool> {
    delete_mcp_server_config_at(&config_path(), server_name).await
}

<<<<<<< fork
/// Delete an MCP server entry from the config file at `path`. Same behavior as [`delete_mcp_server_config`] but targets an explicit config file, e.g. a project-scoped `.grok/config.json`.
/// OAuth credential cleanup is keyed by server name against the global credential store. It therefore also drops credentials a same-named server in another config file uses.
||||||| base
/// Delete an MCP server entry from the config file at `path`. Same behavior as [`delete_mcp_server_config`] but targets an explicit config file, e.g. a project-scoped `.grok/config.toml`.
/// OAuth credential cleanup is keyed by server name against the global credential store. It therefore also drops credentials a same-named server in another config file uses.
=======
/// OAuth credential cleanup is keyed by server name against the global credential store.
/// It therefore also drops credentials a same-named server in another config file uses.
>>>>>>> upstream
pub async fn delete_mcp_server_config_at(
    path: &std::path::Path,
    server_name: &str,
) -> Result<bool> {
    let name = server_name.to_string();
    let wrote = rmw_mcp_toml(path, McpMissing::Skip, false, move |table| {
        let existed = table
            .get_mut("mcp_servers")
            .and_then(|v| v.as_table_mut())
            .and_then(|servers| servers.remove(&name))
            .is_some();

        if !existed {
            return Ok(false);
        }

        if table
            .get("mcp_servers")
            .and_then(|v| v.as_table())
            .is_some_and(|t| t.is_empty())
        {
            table.remove("mcp_servers");
        }

        if let Some(arr) = table
            .get_mut("disabled_mcp_servers")
            .and_then(|v| v.as_array_mut())
        {
            arr.retain(|v| v.as_str() != Some(name.as_str()));
            if arr.is_empty() {
                table.remove("disabled_mcp_servers");
            }
        }

        if let Some(section) = table
            .get_mut("disabled_mcp_tools")
            .and_then(|v| v.as_table_mut())
        {
            section.remove(&name);
            if section.is_empty() {
                table.remove("disabled_mcp_tools");
            }
        }
        Ok(true)
    })
    .await?;

    if wrote
        && let Ok(mut cred_store) = xai_grok_mcp::credentials::McpCredentialStore::load_default()
    {
        let removed = cred_store.remove_by_server_name(server_name);
        if removed > 0 {
            let _ = cred_store.save_default();
        }
    }

    Ok(wrote)
}

<<<<<<< fork
/// Load disabled_tools for all MCP servers from `mcp.disabled_tools` in config.json.
||||||| base
/// Load disabled_tools for all MCP servers from `[disabled_mcp_tools]` in config.toml.
=======
>>>>>>> upstream
pub(crate) fn get_all_mcp_disabled_tools(
    _cwd: &std::path::Path,
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    xai_grok_config::mcp_servers::get_all_mcp_disabled_tools()
}

<<<<<<< fork
pub(crate) struct ParsedMcpServers {
    pub servers: IndexMap<String, McpServerConfig>,
    pub problems: Vec<McpServerConfigProblem>,
}

/// Parse `[mcp_servers.*]` without ever failing the whole config: valid servers load, invalid entries are reported.
pub(crate) fn parse_mcp_servers_with_problems(root: &TomlValue) -> ParsedMcpServers {
    let mut servers = IndexMap::new();
    let mut problems = Vec::new();

    let entries = match root {
        TomlValue::Table(table) => match table.get("mcp_servers") {
            Some(TomlValue::Table(mcp_servers)) => mcp_servers,
            _ => return ParsedMcpServers { servers, problems },
        },
        _ => return ParsedMcpServers { servers, problems },
    };

    for (name, value) in entries {
        match deserialize_mcp_server_config(value) {
            Ok((config, unknown_fields)) => {
                for field in unknown_fields {
                    problems.push(McpServerConfigProblem {
                        server: name.clone(),
                        field: Some(field.clone()),
                        severity: McpServerProblemSeverity::Warning,
                        message: format!(
                            "`mcp_servers.{name}` has an unrecognized field `{field}`; it is \
                             ignored. See ~/.grok/docs/user-guide/07-mcp-servers.md"
                        ),
                    });
                }
                if config.enabled
                    && let Some(field) = config.blank_transport_field()
                {
                    problems.push(McpServerConfigProblem {
                        server: name.clone(),
                        field: Some(field.to_string()),
                        severity: McpServerProblemSeverity::Error,
                        message: format!(
                            "`mcp_servers.{name}` is enabled but its `{field}` is blank. \
                             Set a value, or add \"{name}\" to `disabled_mcp_servers` to turn it \
                             off. See ~/.grok/docs/user-guide/07-mcp-servers.md"
                        ),
                    });
                    continue;
                }
                servers.insert(name.clone(), config);
            }
            Err(error) => problems.push(diagnose_invalid_entry(name, value, &error)),
        }
    }
    ParsedMcpServers { servers, problems }
}

/// Wrapper that logs problems and returns only the valid servers.
pub(crate) fn parse_mcp_servers_from_toml(root: &TomlValue) -> IndexMap<String, McpServerConfig> {
    let ParsedMcpServers { servers, problems } = parse_mcp_servers_with_problems(root);
    for problem in &problems {
        tracing::warn!(server = %problem.server, "{}", problem.message);
    }
    servers
}

// ── .mcp.json support ────────────────────────────────────────────────

// `.mcp.json` discovery moved to `xai-grok-workspace` (client-side, shared with the folder-trust gate)
// The re-export keeps `crate::util::config::*` paths working
pub use xai_grok_workspace::project_config::{
    MCP_JSON_FILENAME, find_mcp_json_files, mcp_json_candidate_paths,
};

pub(crate) fn load_mcp_json_file(path: &std::path::Path) -> Vec<acp::McpServer> {
    if !path.is_file() {
        return vec![];
    }
    let Some(value) = read_mcp_json(path) else {
        return vec![];
    };
    let label = path.display().to_string();
    parse_mcp_config(&value, &label, &crate::config::expand_env_vars_in_string)
}
/// Load .mcp.json servers as McpServerConfig map (for merging into load_mcp_servers).
pub(crate) fn load_mcp_json_servers_as_configs(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    // Phase 2 cutoff: if the user has imported, skip reading .mcp.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_mcp_json_servers_as_configs") {
        return IndexMap::new();
    }
    load_mcp_json_servers_as_configs_unfiltered(cwd)
}

/// Like [`load_mcp_json_servers_as_configs`] but bypasses the import-marker gate.
/// Used by the `/import-claude` scanner so users can re-import items they previously skipped, even after the runtime cutoff is active.
pub(crate) fn load_mcp_json_servers_as_configs_unfiltered(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let mcp_json_files = find_mcp_json_files(cwd);
    if mcp_json_files.is_empty() {
        return IndexMap::new();
    }

    let mut result = IndexMap::new();

    // Reverse so cwd entries win on name conflict.
    for mcp_path in mcp_json_files.iter().rev() {
        if let Some(config) = read_mcp_json(mcp_path) {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }

    result
}

pub(crate) fn parse_mcp_config(
    config: &McpConfig,
    source_label: &str,
    sub: &dyn Fn(&str) -> String,
) -> Vec<acp::McpServer> {
    parse_mcp_config_with_oauth(config, source_label, sub).0
}

pub(crate) fn parse_mcp_config_with_oauth(
    config: &McpConfig,
    source_label: &str,
    sub: &dyn Fn(&str) -> String,
) -> (Vec<acp::McpServer>, McpOAuthConfigMap) {
    let preferences = load_mcp_preferences().file();
    let mut servers = Vec::new();
    let mut oauth_configs = McpOAuthConfigMap::new();
    for (name, server_config) in &config.mcp_servers {
        let mut server_config = match server_config.resolve_setup(preferences.servers.get(name)) {
            McpSetupResolution::Resolved(config) => config,
            McpSetupResolution::Required(_) => continue,
            McpSetupResolution::Invalid(reason) => {
                tracing::warn!(
                    source = source_label,
                    server = %name,
                    error = %reason,
                    "MCP setup config is invalid"
                );
                continue;
            }
        };
        server_config.expand_strings(sub);
        if let Some(oauth) = server_config.oauth_config() {
            oauth_configs.insert(name.clone(), oauth);
        }
        if let Some(server) = server_config.to_acp_mcp_server(name.clone()) {
            servers.push(server);
        } else {
            tracing::warn!(
                source = source_label,
                server = name,
                "MCP server has no 'command' (stdio) or 'url' (http/sse); skipping"
            );
        }
    }

    if !servers.is_empty() {
        tracing::info!(
            source = source_label,
            count = servers.len(),
            "loaded MCP servers"
        );
    }

    (servers, oauth_configs)
}

/// Load MCP servers from `~/.claude.json`.
/// User-level MCP servers live at the top-level `mcpServers` key, and per-project (local-scope) MCP servers under `projects.<cwd>.mcpServers`.
/// Returns servers from both locations (project-specific first, then user-level).
pub(crate) fn load_claude_json_mcp_servers(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> Vec<acp::McpServer> {
    if !compat.claude.mcps {
        return vec![];
    }
    // Phase 2 cutoff: if the user has imported, skip reading ~/.claude.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_claude_json_mcp_servers") {
        return vec![];
    }

    let Some(home) = xai_dirs::home_dir() else {
        return vec![];
    };
    let claude_json_path = home.join(".claude.json");
    load_claude_json_mcp_servers_from(&claude_json_path, cwd)
}

/// On-disk `~/.claude.json` MCP servers for kill-switch attribution only.
/// Bypasses `compat.claude.mcps` and the import-marker cutoff.
/// A client reseed then cannot re-admit Claude-sourced servers while the kill switch is off merely because the normal runtime loader is gated empty.
pub(crate) fn load_claude_json_mcp_servers_for_attribution(
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    let Some(home) = xai_dirs::home_dir() else {
        return vec![];
    };
    load_claude_json_mcp_servers_from(&home.join(".claude.json"), cwd)
}

/// Load ~/.claude.json MCP servers as McpServerConfig map (for merging into load_mcp_servers).
pub(crate) fn load_claude_json_mcp_servers_as_configs(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> IndexMap<String, McpServerConfig> {
    if !compat.claude.mcps {
        return IndexMap::new();
    }
    // Phase 2 cutoff: if the user has imported, skip reading ~/.claude.json.
    if crate::claude_import::is_claude_import_marked_with_log(
        "load_claude_json_mcp_servers_as_configs",
    ) {
        return IndexMap::new();
    }
    load_claude_json_mcp_servers_as_configs_unfiltered(cwd)
}

/// Like [`load_claude_json_mcp_servers_as_configs`] but bypasses the import-marker gate.
/// Used by the `/import-claude` scanner so users can re-import items they previously skipped, even after the runtime cutoff is active.
pub(crate) fn load_claude_json_mcp_servers_as_configs_unfiltered(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let Some(home) = xai_dirs::home_dir() else {
        return IndexMap::new();
    };
    let claude_json_path = home.join(".claude.json");
    load_claude_json_mcp_servers_from_as_configs(&claude_json_path, cwd)
}

fn load_claude_json_mcp_servers_from_as_configs(
    claude_json_path: &std::path::Path,
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let content = match std::fs::read_to_string(claude_json_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to read ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let config = claude_json_mcp_from_value(&value);

    let mut result = IndexMap::new();

    // Per-project MCP servers (local scope, higher priority)
    let cwd_key = cwd.to_string_lossy();
    if let Some(project) = config.projects.get(cwd_key.as_ref()) {
        for (name, cfg) in &project.mcp_servers {
            result.insert(name.clone(), cfg.clone());
        }
    }

    // User-level MCP servers (lower priority)
    for (name, cfg) in &config.user_mcp.mcp_servers {
        result.entry(name.clone()).or_insert(cfg.clone());
    }
    tracing::info!(
        project_count = config
            .projects
            .get(cwd_key.as_ref())
            .map(|p| p.mcp_servers.len())
            .unwrap_or(0),
        user_level_count = config.user_mcp.mcp_servers.len(),
        total_count = result.len(),
        "MCP servers loaded from ~/.claude.json"
    );

    result
}

/// Load MCP servers from editor MCP config files. Scans project-level `<cwd>/.cursor/mcp.json` first (higher priority), then global `~/.cursor/mcp.json`. Both use the `{"mcpServers": {...}}` format identical to `.mcp.json`.
/// Gated by `compat.cursor.mcps`.
pub(crate) fn load_cursor_mcp_servers(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> Vec<acp::McpServer> {
    if !compat.cursor.mcps {
        return vec![];
    }
    let mut result = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Project-level (higher priority)
    let project_path = cwd.join(".cursor").join("mcp.json");
    for server in load_mcp_json_file(&project_path) {
        let name = match &server {
            acp::McpServer::Http(acp::McpServerHttp { name, .. })
            | acp::McpServer::Sse(acp::McpServerSse { name, .. })
            | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
            // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
            _ => continue,
        };
        if seen_names.insert(name) {
            result.push(server);
        }
    }

    // Global (lower priority)
    if let Some(home) = xai_dirs::home_dir() {
        let global_path = home.join(".cursor").join("mcp.json");
        for server in load_mcp_json_file(&global_path) {
            let name = match &server {
                acp::McpServer::Http(acp::McpServerHttp { name, .. })
                | acp::McpServer::Sse(acp::McpServerSse { name, .. })
                | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
                // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
                _ => continue,
            };
            if seen_names.insert(name) {
                result.push(server);
            }
        }
    }

    result
}

/// Load Cursor MCP servers as McpServerConfig map (for merging into load_mcp_servers).
///
/// Scans project-level `<cwd>/.cursor/mcp.json` first, then global.
pub(crate) fn load_cursor_mcp_servers_as_configs(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> IndexMap<String, McpServerConfig> {
    if !compat.cursor.mcps {
        return IndexMap::new();
    }
    let mut result = IndexMap::new();

    // Project-level (higher priority)
    let project_path = cwd.join(".cursor").join("mcp.json");
    if project_path.is_file()
        && let Some(config) = read_mcp_json(&project_path)
    {
        for (name, cfg) in config.mcp_servers {
            result.insert(name, cfg);
        }
    }

    // Global (lower priority; or_insert so project wins)
    if let Some(home) = xai_dirs::home_dir() {
        let global_path = home.join(".cursor").join("mcp.json");
        if global_path.is_file()
            && let Some(config) = read_mcp_json(&global_path)
        {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }

    result
}

/// Inner implementation that accepts the file path, making it testable.
fn load_claude_json_mcp_servers_from(
    claude_json_path: &std::path::Path,
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    let content = match std::fs::read_to_string(claude_json_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse claude.json"
            );
            return vec![];
        }
    };
    let config = claude_json_mcp_from_value(&value);

    let sub = &crate::config::expand_env_vars_in_string;
    let mut servers = Vec::new();

    // Per-project MCP servers (local scope, higher priority)
    let cwd_key = cwd.to_string_lossy();
    if let Some(project) = config.projects.get(cwd_key.as_ref()) {
        let label = format!("~/.claude.json projects[{}]", cwd_key);
        servers.extend(parse_mcp_config(project, &label, sub));
    }

    // User-level MCP servers (lower priority)
    if !config.user_mcp.mcp_servers.is_empty() {
        servers.extend(parse_mcp_config(&config.user_mcp, "~/.claude.json", sub));
    }

    servers
}

/// Build an `McpConfig` from a JSON value, skipping any `mcpServers` entry that fails to deserialize instead of dropping the whole file.
/// Mirrors the per-entry tolerance of [`parse_mcp_servers_with_problems`] for TOML.
/// One bad entry in a `.mcp.json` or `~/.claude.json` cannot take out its siblings.
fn mcp_config_from_json_value(value: &serde_json::Value) -> McpConfig {
    let mut mcp_servers = IndexMap::new();
    if let Some(entries) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in entries {
            match serde_json::from_value::<McpServerConfig>(entry.clone()) {
                Ok(config) => {
                    mcp_servers.insert(name.clone(), config);
                }
                Err(error) => tracing::warn!(
                    server = %name,
                    error = %error,
                    "skipping invalid MCP server entry in JSON config"
                ),
            }
        }
    }
    McpConfig { mcp_servers }
}

/// Parsed `~/.claude.json` MCP view: top-level user servers plus per-project maps.
struct ClaudeJsonMcp {
    user_mcp: McpConfig,
    projects: HashMap<String, McpConfig>,
}

/// Build the `~/.claude.json` MCP view from a JSON value, tolerating bad entries per server (see [`mcp_config_from_json_value`]).
fn claude_json_mcp_from_value(value: &serde_json::Value) -> ClaudeJsonMcp {
    let user_mcp = mcp_config_from_json_value(value);
    let mut projects = HashMap::new();
    if let Some(entries) = value.get("projects").and_then(|v| v.as_object()) {
        for (path, project) in entries {
            projects.insert(path.clone(), mcp_config_from_json_value(project));
        }
    }
    ClaudeJsonMcp { user_mcp, projects }
}

/// Returns `None` on I/O or top-level parse errors (logged); individual bad `mcpServers` entries are skipped, not fatal.
pub(crate) fn read_mcp_json(path: &std::path::Path) -> Option<McpConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to read MCP JSON");
        })
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to parse MCP JSON");
        })
        .ok()?;
    Some(mcp_config_from_json_value(&value))
}

/// Like `load_mcp_servers_with_project` but returns raw configs without filtering by `enabled`.
fn load_all_mcp_configs(cwd: &std::path::Path) -> IndexMap<String, McpServerConfig> {
    load_mcp_server_configs_with_project(cwd)
        .into_iter()
        .map(|(name, (config, _))| (name, config))
        .collect()
}

/// Load all configured MCP servers with the scope each definition came from (`"user"` or `"project"`).
/// Overlays project-scoped `.grok/config.json` files from `cwd` up to the repo root onto the user-tier config, nearest definition winning.
/// Overrides work the same way as in [`get_mcp_server_config_with_project`].
||||||| base
pub(crate) struct ParsedMcpServers {
    pub servers: IndexMap<String, McpServerConfig>,
    pub problems: Vec<McpServerConfigProblem>,
}

/// Parse `[mcp_servers.*]` without ever failing the whole config: valid servers load, invalid entries are reported.
pub(crate) fn parse_mcp_servers_with_problems(root: &TomlValue) -> ParsedMcpServers {
    let mut servers = IndexMap::new();
    let mut problems = Vec::new();

    let entries = match root {
        TomlValue::Table(table) => match table.get("mcp_servers") {
            Some(TomlValue::Table(mcp_servers)) => mcp_servers,
            _ => return ParsedMcpServers { servers, problems },
        },
        _ => return ParsedMcpServers { servers, problems },
    };

    for (name, value) in entries {
        match deserialize_mcp_server_config(value) {
            Ok((config, unknown_fields)) => {
                for field in unknown_fields {
                    problems.push(McpServerConfigProblem {
                        server: name.clone(),
                        field: Some(field.clone()),
                        severity: McpServerProblemSeverity::Warning,
                        message: format!(
                            "`mcp_servers.{name}` has an unrecognized field `{field}`; it is \
                             ignored. See ~/.grok/docs/user-guide/07-mcp-servers.md"
                        ),
                    });
                }
                if config.enabled
                    && let Some(field) = config.blank_transport_field()
                {
                    problems.push(McpServerConfigProblem {
                        server: name.clone(),
                        field: Some(field.to_string()),
                        severity: McpServerProblemSeverity::Error,
                        message: format!(
                            "`mcp_servers.{name}` is enabled but its `{field}` is blank. \
                             Set a value, or add \"{name}\" to `disabled_mcp_servers` to turn it \
                             off. See ~/.grok/docs/user-guide/07-mcp-servers.md"
                        ),
                    });
                    continue;
                }
                servers.insert(name.clone(), config);
            }
            Err(error) => problems.push(diagnose_invalid_entry(name, value, &error)),
        }
    }
    ParsedMcpServers { servers, problems }
}

/// Wrapper that logs problems and returns only the valid servers.
pub(crate) fn parse_mcp_servers_from_toml(root: &TomlValue) -> IndexMap<String, McpServerConfig> {
    let ParsedMcpServers { servers, problems } = parse_mcp_servers_with_problems(root);
    for problem in &problems {
        tracing::warn!(server = %problem.server, "{}", problem.message);
    }
    servers
}

// ── .mcp.json support ────────────────────────────────────────────────

// `.mcp.json` discovery moved to `xai-grok-workspace` (client-side, shared with the folder-trust gate)
// The re-export keeps `crate::util::config::*` paths working
pub use xai_grok_workspace::project_config::{
    MCP_JSON_FILENAME, find_mcp_json_files, mcp_json_candidate_paths,
};

pub(crate) fn load_mcp_json_file(path: &std::path::Path) -> Vec<acp::McpServer> {
    if !path.is_file() {
        return vec![];
    }
    let Some(value) = read_mcp_json(path) else {
        return vec![];
    };
    let label = path.display().to_string();
    parse_mcp_config(&value, &label, &crate::config::expand_env_vars_in_string)
}
/// Load .mcp.json servers as McpServerConfig map (for merging into load_mcp_servers).
pub(crate) fn load_mcp_json_servers_as_configs(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    // Phase 2 cutoff: if the user has imported, skip reading .mcp.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_mcp_json_servers_as_configs") {
        return IndexMap::new();
    }
    load_mcp_json_servers_as_configs_unfiltered(cwd)
}

/// Like [`load_mcp_json_servers_as_configs`] but bypasses the import-marker gate.
/// Used by the `/import-claude` scanner so users can re-import items they previously skipped, even after the runtime cutoff is active.
pub(crate) fn load_mcp_json_servers_as_configs_unfiltered(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let mcp_json_files = find_mcp_json_files(cwd);
    if mcp_json_files.is_empty() {
        return IndexMap::new();
    }

    let mut result = IndexMap::new();

    // Reverse so cwd entries win on name conflict.
    for mcp_path in mcp_json_files.iter().rev() {
        if let Some(config) = read_mcp_json(mcp_path) {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }

    result
}

pub(crate) fn parse_mcp_config(
    config: &McpConfig,
    source_label: &str,
    sub: &dyn Fn(&str) -> String,
) -> Vec<acp::McpServer> {
    parse_mcp_config_with_oauth(config, source_label, sub).0
}

pub(crate) fn parse_mcp_config_with_oauth(
    config: &McpConfig,
    source_label: &str,
    sub: &dyn Fn(&str) -> String,
) -> (Vec<acp::McpServer>, McpOAuthConfigMap) {
    let preferences = load_mcp_preferences().file();
    let mut servers = Vec::new();
    let mut oauth_configs = McpOAuthConfigMap::new();
    for (name, server_config) in &config.mcp_servers {
        let mut server_config = match server_config.resolve_setup(preferences.servers.get(name)) {
            McpSetupResolution::Resolved(config) => config,
            McpSetupResolution::Required(_) => continue,
            McpSetupResolution::Invalid(reason) => {
                tracing::warn!(
                    source = source_label,
                    server = %name,
                    error = %reason,
                    "MCP setup config is invalid"
                );
                continue;
            }
        };
        server_config.expand_strings(sub);
        if let Some(oauth) = server_config.oauth_config() {
            oauth_configs.insert(name.clone(), oauth);
        }
        if let Some(server) = server_config.to_acp_mcp_server(name.clone()) {
            servers.push(server);
        } else {
            tracing::warn!(
                source = source_label,
                server = name,
                "MCP server has no 'command' (stdio) or 'url' (http/sse); skipping"
            );
        }
    }

    if !servers.is_empty() {
        tracing::info!(
            source = source_label,
            count = servers.len(),
            "loaded MCP servers"
        );
    }

    (servers, oauth_configs)
}

/// Load MCP servers from `~/.claude.json`.
/// User-level MCP servers live at the top-level `mcpServers` key, and per-project (local-scope) MCP servers under `projects.<cwd>.mcpServers`.
/// Returns servers from both locations (project-specific first, then user-level).
pub(crate) fn load_claude_json_mcp_servers(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> Vec<acp::McpServer> {
    if !compat.claude.mcps {
        return vec![];
    }
    // Phase 2 cutoff: if the user has imported, skip reading ~/.claude.json.
    if crate::claude_import::is_claude_import_marked_with_log("load_claude_json_mcp_servers") {
        return vec![];
    }

    let Some(home) = xai_dirs::home_dir() else {
        return vec![];
    };
    let claude_json_path = home.join(".claude.json");
    load_claude_json_mcp_servers_from(&claude_json_path, cwd)
}

/// On-disk `~/.claude.json` MCP servers for kill-switch attribution only.
/// Bypasses `compat.claude.mcps` and the import-marker cutoff.
/// A client reseed then cannot re-admit Claude-sourced servers while the kill switch is off merely because the normal runtime loader is gated empty.
pub(crate) fn load_claude_json_mcp_servers_for_attribution(
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    let Some(home) = xai_dirs::home_dir() else {
        return vec![];
    };
    load_claude_json_mcp_servers_from(&home.join(".claude.json"), cwd)
}

/// Load ~/.claude.json MCP servers as McpServerConfig map (for merging into load_mcp_servers).
pub(crate) fn load_claude_json_mcp_servers_as_configs(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> IndexMap<String, McpServerConfig> {
    if !compat.claude.mcps {
        return IndexMap::new();
    }
    // Phase 2 cutoff: if the user has imported, skip reading ~/.claude.json.
    if crate::claude_import::is_claude_import_marked_with_log(
        "load_claude_json_mcp_servers_as_configs",
    ) {
        return IndexMap::new();
    }
    load_claude_json_mcp_servers_as_configs_unfiltered(cwd)
}

/// Like [`load_claude_json_mcp_servers_as_configs`] but bypasses the import-marker gate.
/// Used by the `/import-claude` scanner so users can re-import items they previously skipped, even after the runtime cutoff is active.
pub(crate) fn load_claude_json_mcp_servers_as_configs_unfiltered(
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let Some(home) = xai_dirs::home_dir() else {
        return IndexMap::new();
    };
    let claude_json_path = home.join(".claude.json");
    load_claude_json_mcp_servers_from_as_configs(&claude_json_path, cwd)
}

fn load_claude_json_mcp_servers_from_as_configs(
    claude_json_path: &std::path::Path,
    cwd: &std::path::Path,
) -> IndexMap<String, McpServerConfig> {
    let content = match std::fs::read_to_string(claude_json_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to read ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse ~/.claude.json"
            );
            return IndexMap::new();
        }
    };
    let config = claude_json_mcp_from_value(&value);

    let mut result = IndexMap::new();

    // Per-project MCP servers (local scope, higher priority)
    let cwd_key = cwd.to_string_lossy();
    if let Some(project) = config.projects.get(cwd_key.as_ref()) {
        for (name, cfg) in &project.mcp_servers {
            result.insert(name.clone(), cfg.clone());
        }
    }

    // User-level MCP servers (lower priority)
    for (name, cfg) in &config.user_mcp.mcp_servers {
        result.entry(name.clone()).or_insert(cfg.clone());
    }
    tracing::info!(
        project_count = config
            .projects
            .get(cwd_key.as_ref())
            .map(|p| p.mcp_servers.len())
            .unwrap_or(0),
        user_level_count = config.user_mcp.mcp_servers.len(),
        total_count = result.len(),
        "MCP servers loaded from ~/.claude.json"
    );

    result
}

/// Load MCP servers from editor MCP config files. Scans project-level `<cwd>/.cursor/mcp.json` first (higher priority), then global `~/.cursor/mcp.json`. Both use the `{"mcpServers": {...}}` format identical to `.mcp.json`.
/// Gated by `compat.cursor.mcps`.
pub(crate) fn load_cursor_mcp_servers(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> Vec<acp::McpServer> {
    if !compat.cursor.mcps {
        return vec![];
    }
    let mut result = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Project-level (higher priority)
    let project_path = cwd.join(".cursor").join("mcp.json");
    for server in load_mcp_json_file(&project_path) {
        let name = match &server {
            acp::McpServer::Http(acp::McpServerHttp { name, .. })
            | acp::McpServer::Sse(acp::McpServerSse { name, .. })
            | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
            // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
            _ => continue,
        };
        if seen_names.insert(name) {
            result.push(server);
        }
    }

    // Global (lower priority)
    if let Some(home) = xai_dirs::home_dir() {
        let global_path = home.join(".cursor").join("mcp.json");
        for server in load_mcp_json_file(&global_path) {
            let name = match &server {
                acp::McpServer::Http(acp::McpServerHttp { name, .. })
                | acp::McpServer::Sse(acp::McpServerSse { name, .. })
                | acp::McpServer::Stdio(acp::McpServerStdio { name, .. }) => name.clone(),
                // TODO(acp-0.10): `McpServer` is #[non_exhaustive].
                _ => continue,
            };
            if seen_names.insert(name) {
                result.push(server);
            }
        }
    }

    result
}

/// Load Cursor MCP servers as McpServerConfig map (for merging into load_mcp_servers).
///
/// Scans project-level `<cwd>/.cursor/mcp.json` first, then global.
pub(crate) fn load_cursor_mcp_servers_as_configs(
    cwd: &std::path::Path,
    compat: &CompatConfig,
) -> IndexMap<String, McpServerConfig> {
    if !compat.cursor.mcps {
        return IndexMap::new();
    }
    let mut result = IndexMap::new();

    // Project-level (higher priority)
    let project_path = cwd.join(".cursor").join("mcp.json");
    if project_path.is_file()
        && let Some(config) = read_mcp_json(&project_path)
    {
        for (name, cfg) in config.mcp_servers {
            result.insert(name, cfg);
        }
    }

    // Global (lower priority; or_insert so project wins)
    if let Some(home) = xai_dirs::home_dir() {
        let global_path = home.join(".cursor").join("mcp.json");
        if global_path.is_file()
            && let Some(config) = read_mcp_json(&global_path)
        {
            for (name, cfg) in config.mcp_servers {
                result.entry(name).or_insert(cfg);
            }
        }
    }

    result
}

/// Inner implementation that accepts the file path, making it testable.
fn load_claude_json_mcp_servers_from(
    claude_json_path: &std::path::Path,
    cwd: &std::path::Path,
) -> Vec<acp::McpServer> {
    let content = match std::fs::read_to_string(claude_json_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(
                path = %claude_json_path.display(),
                error = %e,
                "failed to parse claude.json"
            );
            return vec![];
        }
    };
    let config = claude_json_mcp_from_value(&value);

    let sub = &crate::config::expand_env_vars_in_string;
    let mut servers = Vec::new();

    // Per-project MCP servers (local scope, higher priority)
    let cwd_key = cwd.to_string_lossy();
    if let Some(project) = config.projects.get(cwd_key.as_ref()) {
        let label = format!("~/.claude.json projects[{}]", cwd_key);
        servers.extend(parse_mcp_config(project, &label, sub));
    }

    // User-level MCP servers (lower priority)
    if !config.user_mcp.mcp_servers.is_empty() {
        servers.extend(parse_mcp_config(&config.user_mcp, "~/.claude.json", sub));
    }

    servers
}

/// Build an `McpConfig` from a JSON value, skipping any `mcpServers` entry that fails to deserialize instead of dropping the whole file.
/// Mirrors the per-entry tolerance of [`parse_mcp_servers_with_problems`] for TOML.
/// One bad entry in a `.mcp.json` or `~/.claude.json` cannot take out its siblings.
fn mcp_config_from_json_value(value: &serde_json::Value) -> McpConfig {
    let mut mcp_servers = IndexMap::new();
    if let Some(entries) = value.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, entry) in entries {
            match serde_json::from_value::<McpServerConfig>(entry.clone()) {
                Ok(config) => {
                    mcp_servers.insert(name.clone(), config);
                }
                Err(error) => tracing::warn!(
                    server = %name,
                    error = %error,
                    "skipping invalid MCP server entry in JSON config"
                ),
            }
        }
    }
    McpConfig { mcp_servers }
}

/// Parsed `~/.claude.json` MCP view: top-level user servers plus per-project maps.
struct ClaudeJsonMcp {
    user_mcp: McpConfig,
    projects: HashMap<String, McpConfig>,
}

/// Build the `~/.claude.json` MCP view from a JSON value, tolerating bad entries per server (see [`mcp_config_from_json_value`]).
fn claude_json_mcp_from_value(value: &serde_json::Value) -> ClaudeJsonMcp {
    let user_mcp = mcp_config_from_json_value(value);
    let mut projects = HashMap::new();
    if let Some(entries) = value.get("projects").and_then(|v| v.as_object()) {
        for (path, project) in entries {
            projects.insert(path.clone(), mcp_config_from_json_value(project));
        }
    }
    ClaudeJsonMcp { user_mcp, projects }
}

/// Returns `None` on I/O or top-level parse errors (logged); individual bad `mcpServers` entries are skipped, not fatal.
pub(crate) fn read_mcp_json(path: &std::path::Path) -> Option<McpConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to read MCP JSON");
        })
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to parse MCP JSON");
        })
        .ok()?;
    Some(mcp_config_from_json_value(&value))
}

/// Like `load_mcp_servers_with_project` but returns raw configs without filtering by `enabled`.
fn load_all_mcp_configs(cwd: &std::path::Path) -> IndexMap<String, McpServerConfig> {
    load_mcp_server_configs_with_project(cwd)
        .into_iter()
        .map(|(name, (config, _))| (name, config))
        .collect()
}

/// Load all configured MCP servers with the scope each definition came from (`"user"` or `"project"`).
/// Overlays project-scoped `.grok/config.toml` files from `cwd` up to the repo root onto the user-tier config, nearest definition winning.
/// Overrides work the same way as in [`get_mcp_server_config_with_project`].
=======
>>>>>>> upstream
pub fn load_mcp_server_configs_with_project(
    cwd: &std::path::Path,
) -> IndexMap<String, (McpServerConfig, &'static str)> {
<<<<<<< fork
    let global_config = crate::config::load_effective_config()
        .unwrap_or_else(|_| TomlValue::Table(toml::map::Map::new()));
    toml_mcp_server_configs_from(&global_config, cwd)
}

/// The ONE global+project TOML overlay walk — merge, discovery, the OAuth map, and scope tags all
/// derive from it. Project `.grok/config.json` definitions replace user-tier ones, nearest wins.
fn toml_mcp_server_configs_from(
    global_config: &TomlValue,
    cwd: &std::path::Path,
) -> IndexMap<String, (McpServerConfig, &'static str)> {
    let mut servers: IndexMap<String, (McpServerConfig, &'static str)> =
        parse_mcp_servers_from_toml(global_config)
            .into_iter()
            .map(|(name, config)| (name, (config, MCP_SCOPE_USER)))
            .collect();

    // find_project_configs is repo-root-first, so nearer files overwrite.
    for config_path in crate::config::find_project_configs(cwd) {
        if let Ok(root) = crate::config::load_config_file(&config_path) {
            let project_servers = parse_mcp_servers_from_toml(&root);
            if !project_servers.is_empty() {
                tracing::debug!(
                    count = project_servers.len(),
                    path = %config_path.display(),
                    "Loaded project-scoped MCP servers from .grok/config.json"
                );
                for (name, config) in project_servers {
                    servers.insert(name, (config, MCP_SCOPE_PROJECT));
                }
            }
        }
    }

    servers
}

/// MCP config problems across the same layers as [`load_mcp_server_configs_with_project`], for `grok inspect`.
pub(crate) fn load_mcp_server_problems_with_project(
    cwd: &std::path::Path,
) -> Vec<McpServerConfigProblem> {
    let mut problems = Vec::new();
    if let Ok(global_config) = crate::config::load_effective_config() {
        problems.extend(parse_mcp_servers_with_problems(&global_config).problems);
    }
    for config_path in crate::config::find_project_configs(cwd) {
        if let Ok(root) = crate::config::load_config_file(&config_path) {
            problems.extend(parse_mcp_servers_with_problems(&root).problems);
        }
    }
    problems
}

/// MCP server names with `enabled = false` in config.json (including project overrides).
pub fn disabled_mcp_server_names(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut disabled: std::collections::HashSet<String> = load_all_mcp_configs(cwd)
        .into_iter()
        .filter(|(_, cfg)| !cfg.enabled)
        .map(|(name, _)| name)
        .collect();

    // Also check the `disabled_mcp_servers` array in config.json.
    if let Ok(root) = crate::config::load_effective_config()
        && let Some(arr) = root.get("disabled_mcp_servers").and_then(|v| v.as_array())
    {
        for val in arr {
            if let Some(name) = val.as_str() {
                disabled.insert(name.to_string());
            }
        }
    }
||||||| base
    cwd: &std::path::Path,
) -> IndexMap<String, (McpServerConfig, &'static str)> {
    let global_config = crate::config::load_effective_config()
        .unwrap_or_else(|_| TomlValue::Table(toml::map::Map::new()));
    toml_mcp_server_configs_from(&global_config, cwd)
}

/// The ONE global+project TOML overlay walk — merge, discovery, the OAuth map, and scope tags all
/// derive from it. Project `.grok/config.toml` definitions replace user-tier ones, nearest wins.
fn toml_mcp_server_configs_from(
    global_config: &TomlValue,
    cwd: &std::path::Path,
) -> IndexMap<String, (McpServerConfig, &'static str)> {
    let mut servers: IndexMap<String, (McpServerConfig, &'static str)> =
        parse_mcp_servers_from_toml(global_config)
            .into_iter()
            .map(|(name, config)| (name, (config, MCP_SCOPE_USER)))
            .collect();

    // find_project_configs is repo-root-first, so nearer files overwrite.
    for config_path in crate::config::find_project_configs(cwd) {
        if let Ok(root) = crate::config::load_config_file(&config_path) {
            let project_servers = parse_mcp_servers_from_toml(&root);
            if !project_servers.is_empty() {
                tracing::debug!(
                    count = project_servers.len(),
                    path = %config_path.display(),
                    "Loaded project-scoped MCP servers from .grok/config.toml"
                );
                for (name, config) in project_servers {
                    servers.insert(name, (config, MCP_SCOPE_PROJECT));
                }
            }
        }
    }

    servers
}

/// MCP config problems across the same layers as [`load_mcp_server_configs_with_project`], for `grok inspect`.
pub(crate) fn load_mcp_server_problems_with_project(
    cwd: &std::path::Path,
) -> Vec<McpServerConfigProblem> {
    let mut problems = Vec::new();
    if let Ok(global_config) = crate::config::load_effective_config() {
        problems.extend(parse_mcp_servers_with_problems(&global_config).problems);
    }
    for config_path in crate::config::find_project_configs(cwd) {
        if let Ok(root) = crate::config::load_config_file(&config_path) {
            problems.extend(parse_mcp_servers_with_problems(&root).problems);
        }
    }
    problems
}

/// MCP server names with `enabled = false` in config.toml (including project overrides).
pub fn disabled_mcp_server_names(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut disabled: std::collections::HashSet<String> = load_all_mcp_configs(cwd)
        .into_iter()
        .filter(|(_, cfg)| !cfg.enabled)
        .map(|(name, _)| name)
        .collect();

    // Also check the `disabled_mcp_servers` array in config.toml.
    if let Ok(root) = crate::config::load_effective_config()
        && let Some(arr) = root.get("disabled_mcp_servers").and_then(|v| v.as_array())
    {
        for val in arr {
            if let Some(name) = val.as_str() {
                disabled.insert(name.to_string());
            }
        }
    }
=======
    xai_grok_config::mcp_servers::load_mcp_server_configs_with_project(
        &crate::config::find_project_configs(cwd),
    )
}
>>>>>>> upstream

pub(crate) fn load_mcp_server_problems_with_project(
    cwd: &std::path::Path,
) -> Vec<McpServerConfigProblem> {
    xai_grok_config::mcp_servers::load_mcp_server_problems_with_project(
        &crate::config::find_project_configs(cwd),
    )
}

/// Excludes gateway connectors (`managed_gateway:…`), which the `/mcps` Space disables through
/// `disabled_mcp_tools.__managed_gateway_connectors`. A `grok_com_*` name is known only when a
/// definition declares it, not by its prefix.
pub fn cli_known_mcp_server_names(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut names = disabled_mcp_server_names(cwd);
    // The merge drops setup-required and invalid entries, which `mcp list` still shows
    names.extend(all_toml_mcp_server_names(cwd));

    let registry = load_cli_plugin_registry(cwd);
    let compat = CompatConfig::default();
    for (server, _) in crate::session::managed_mcp::merge_managed_mcp_servers_sourced(
        cwd,
        Some(&registry),
        &compat,
    ) {
        let name = crate::session::managed_mcp::mcp_server_name(&server);
        if !name.is_empty() {
            names.insert(name.to_string());
        }
    }
    names
}

pub fn disabled_mcp_server_names(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    xai_grok_config::mcp_servers::disabled_mcp_server_names(&crate::config::find_project_configs(
        cwd,
    ))
}

pub(crate) fn all_toml_mcp_server_names(
    cwd: &std::path::Path,
) -> std::collections::HashSet<String> {
    xai_grok_config::mcp_servers::all_toml_mcp_server_names(&crate::config::find_project_configs(
        cwd,
    ))
}

/// Resolves the same cwd-effective `[plugins]` table as session startup, so trusted project plugins are included.
pub fn load_cli_plugin_registry(cwd: &std::path::Path) -> xai_grok_agent::plugins::PluginRegistry {
    let trust_store = xai_grok_agent::plugins::TrustStore::load();
    // Resolve/record the folder-trust verdict first: the effective-plugins resolve below gates project [plugins].paths on it
    let project_trusted = crate::agent::folder_trust::resolve_and_record(cwd, None, false);
    let mut plugin_config = xai_grok_workspace::plugins::resolve_effective_plugins_config(
        xai_grok_workspace::plugins::PluginConfigInputs {
            effective_config: crate::config::load_effective_config().ok().as_ref(),
            home: xai_dirs::home_dir().as_deref(),
            grok_home: xai_grok_config::user_grok_home().as_deref(),
            cwd,
            trust: xai_grok_hooks::trust::Trust::from_verdict(project_trusted),
            claude_import: crate::claude_import::import_marker(),
        },
    );
    let discovered = xai_grok_agent::plugins::discover_plugins(
        Some(cwd),
        &plugin_config,
        &trust_store,
        project_trusted,
    );
    plugin_config.populate_plugin_lists(&discovered);
    xai_grok_agent::plugins::PluginRegistry::from_discovered(
        discovered,
        &plugin_config.disabled,
        &plugin_config.enabled,
    )
}

fn config_path() -> PathBuf {
    // Live `$GROK_HOME` first: `grok_home()` is OnceLock and misses EnvGuard/tests.
    let home = xai_dirs::resolve_grok_home().unwrap_or_else(crate::util::grok_home::grok_home);
    // A legacy `config.toml` migrates here; a broken one stays put, and the loader reports its error
    match xai_grok_config::user_config_file(&home) {
        Ok(path) => path,
        Err(_) => home.join(xai_grok_config::USER_CONFIG_FILENAME),
    }
}

<<<<<<< fork
/// Path to the user-level config file (`~/.grok/config.json`).
||||||| base
/// Path to the user-level config file (`~/.grok/config.toml`).
=======
>>>>>>> upstream
pub fn user_config_path() -> PathBuf {
    config_path()
}

<<<<<<< fork
/// Path to the project-level config file to read (`<dir>/.grok/config.json`, or a legacy `config.toml`).
||||||| base
/// Path to a project-level config file (`<dir>/.grok/config.toml`).
=======
>>>>>>> upstream
pub fn project_config_path(dir: &std::path::Path) -> PathBuf {
    xai_grok_config::project_config_file(dir)
}

/// Path to the project-level config file to write
/// (`<dir>/.grok/config.json`).
pub fn project_config_path_for_write(dir: &std::path::Path) -> std::io::Result<PathBuf> {
    xai_grok_config::project_config_file_for_write(dir)
}

/// Checks raw key presence rather than deserializing, so malformed entries (the ones users most need `mcp remove` for) are still reported.
pub fn mcp_server_defined_at(path: &std::path::Path, server_name: &str) -> bool {
    let Ok(root) = crate::config::load_config_file(path) else {
        return false;
    };
    root.get("mcp_servers")
        .and_then(|v| v.as_table())
        .is_some_and(|servers| servers.contains_key(server_name))
}

<<<<<<< fork
/// Returns `None` when `[cli] use_leader` is not set in the config, or `Some(true/false)` when explicitly configured.
/// The `None` lets callers fall through to a remote flag when the user hasn't expressed a local preference.
||||||| base
/// Synchronously load `[cli] npm_registry` from config.toml.
pub fn load_npm_registry_sync() -> Option<String> {
    let root: TomlValue = crate::config::load_effective_config().ok()?;
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(cli)) = table.get("cli")
    {
        cli.get("npm_registry")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

pub(crate) fn load_gcs_service_account_key_sync() -> Option<String> {
    let root: TomlValue = crate::config::load_effective_config().ok()?;
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(endpoints)) = table.get("endpoints")
    {
        endpoints
            .get("gcs_service_account_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}
/// Returns `None` when `[cli] use_leader` is not set in the config, or `Some(true/false)` when explicitly configured.
/// The `None` lets callers fall through to a remote flag when the user hasn't expressed a local preference.
=======
pub fn load_npm_registry_sync() -> Option<String> {
    let root: TomlValue = crate::config::load_effective_config().ok()?;
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(cli)) = table.get("cli")
    {
        cli.get("npm_registry")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

pub(crate) fn load_gcs_service_account_key_sync() -> Option<String> {
    let root: TomlValue = crate::config::load_effective_config().ok()?;
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(endpoints)) = table.get("endpoints")
    {
        endpoints
            .get("gcs_service_account_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}
/// `None` lets callers fall through to a remote flag when the user hasn't expressed a local preference.
>>>>>>> upstream
pub fn use_leader_from_toml_opt(root: &TomlValue) -> Option<bool> {
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(cli)) = table.get("cli")
    {
        cli.get("use_leader").and_then(|v| v.as_bool())
    } else {
        None
    }
}

pub fn use_leader_from_toml(root: &TomlValue) -> bool {
    use_leader_from_toml_opt(root).unwrap_or(false)
}

<<<<<<< fork
/// Returns `Some(true/false)` when `cli.session_registry` is set in config.json, `None` when absent (allowing remote settings fallback).
/// Local config takes precedence over remote settings.
||||||| base
/// Returns `Some(true/false)` when `[cli] session_registry` is set in config.toml, `None` when absent (allowing remote settings fallback).
/// Local config takes precedence over remote settings.
=======
>>>>>>> upstream
pub(crate) fn session_registry_from_toml_opt(root: &TomlValue) -> Option<bool> {
    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(cli)) = table.get("cli")
    {
        cli.get("session_registry").and_then(|v| v.as_bool())
    } else {
        None
    }
}

/// Overrides `cli.session_registry`; usable before `~/.grok/config.json` exists.
pub const SESSION_REGISTRY_ENV_VAR: &str = "GROK_SESSION_REGISTRY";

pub(crate) fn session_registry_from_env_opt() -> Option<bool> {
    xai_grok_config::env_bool(SESSION_REGISTRY_ENV_VAR)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrySource {
    Env,
<<<<<<< fork
    /// `cli.session_registry` in config.json.
||||||| base
    /// `[cli] session_registry` in config.toml.
=======
>>>>>>> upstream
    ConfigToml,
}

impl RegistrySource {
    pub const fn label(self) -> &'static str {
        match self {
            RegistrySource::Env => SESSION_REGISTRY_ENV_VAR,
            RegistrySource::ConfigToml => "`cli.session_registry`",
        }
    }
}

/// Env var, then `[cli] session_registry`; `None` defers to remote settings.
pub fn session_registry_local_override_sourced(
    root: Option<&TomlValue>,
) -> Option<(bool, RegistrySource)> {
    if let Some(v) = session_registry_from_env_opt() {
        return Some((v, RegistrySource::Env));
    }
    root.and_then(session_registry_from_toml_opt)
        .map(|v| (v, RegistrySource::ConfigToml))
}

pub(crate) fn session_registry_local_override(root: Option<&TomlValue>) -> Option<bool> {
    session_registry_local_override_sourced(root).map(|(v, _)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toml::Value as TomlValue;

    #[test]
    #[serial_test::serial]
    fn session_registry_local_override_precedence() {
        let toml_true: TomlValue = toml::from_str("[cli]\nsession_registry = true").unwrap();
        {
            let _g = xai_grok_test_support::EnvGuard::set(SESSION_REGISTRY_ENV_VAR, "false");
            assert_eq!(
                session_registry_local_override_sourced(Some(&toml_true)),
                Some((false, RegistrySource::Env)),
                "env wins and reports itself as the source"
            );
        }
        {
            let _g = xai_grok_test_support::EnvGuard::set(SESSION_REGISTRY_ENV_VAR, "bogus");
            assert_eq!(
                session_registry_local_override_sourced(Some(&toml_true)),
                Some((true, RegistrySource::ConfigToml)),
                "unrecognized env values defer to config.toml"
            );
        }
        {
            let _g = xai_grok_test_support::EnvGuard::unset(SESSION_REGISTRY_ENV_VAR);
            assert_eq!(session_registry_local_override_sourced(None), None);
        }
    }

    #[test]
    #[serial_test::serial]
    fn load_cli_plugin_registry_includes_project_config_path_plugins() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());

        let repo = tempfile::tempdir().unwrap();
        gix::init(repo.path()).unwrap();

        let plugin_dir = repo.path().join("proj-plugin");
        let agents_dir = plugin_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "proj-plugin", "agents": "./agents"}"#,
        )
        .unwrap();
        std::fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Project reviewer\n---\nBody.\n",
        )
        .unwrap();

        let grok = repo.path().join(".grok");
        std::fs::create_dir_all(&grok).unwrap();
        std::fs::write(
            grok.join("config.toml"),
            format!("[plugins]\npaths = [\"{}\"]\n", plugin_dir.display()),
        )
        .unwrap();

        let registry = load_cli_plugin_registry(repo.path());
        assert!(
            registry.get("proj-plugin").is_some(),
            "project [plugins].paths plugin must be discovered"
        );
        let agents = xai_grok_agent::discovery::plugin_agents(&registry);
        assert!(
            agents
                .iter()
                .any(|a| a.qualified_name == "proj-plugin:reviewer"),
            "project config-path plugin agent must be enumerable, got: {:?}",
            agents.iter().map(|a| &a.qualified_name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mcp_server_defined_at_checks_raw_key_presence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[mcp_servers.broken]\nurll = \"https://x.example\"\n",
        )
        .unwrap();

        assert!(mcp_server_defined_at(&path, "broken"));
        assert!(!mcp_server_defined_at(&path, "other"));
        assert!(!mcp_server_defined_at(
            &dir.path().join("missing.toml"),
            "broken"
        ));
    }

    #[test]
    fn test_use_leader_parsing_true() {
        let toml_str = r#"
[cli]
use_leader = true
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert!(use_leader_from_toml(&root));
    }

    #[test]
    fn test_use_leader_parsing_false() {
        let toml_str = r#"
[cli]
use_leader = false
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert!(!use_leader_from_toml(&root));
    }

    #[test]
    fn test_use_leader_default_false() {
        let toml_str = r#"
[cli]
auto_update = true
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert!(!use_leader_from_toml(&root));
    }

    #[test]
    fn test_use_leader_no_cli_section() {
        let toml_str = r#"
[models]
default = "grok-code-fast-1"
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        if let TomlValue::Table(ref table) = root {
            let has_cli = table.get("cli").is_some();
            assert!(!has_cli);
        }
        assert!(!use_leader_from_toml(&root));
    }

    #[test]
    fn test_use_leader_opt_returns_some_true() {
        let toml_str = r#"
[cli]
use_leader = true
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert_eq!(use_leader_from_toml_opt(&root), Some(true));
    }

    #[test]
    fn test_use_leader_opt_returns_some_false() {
        let toml_str = r#"
[cli]
use_leader = false
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert_eq!(use_leader_from_toml_opt(&root), Some(false));
    }

    #[test]
    fn test_use_leader_opt_returns_none_when_absent() {
        let toml_str = r#"
[cli]
auto_update = true
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert_eq!(use_leader_from_toml_opt(&root), None);
    }

    #[test]
    fn test_use_leader_opt_returns_none_when_no_cli_section() {
        let toml_str = r#"
[models]
default = "grok-code-fast-1"
"#;
        let root: TomlValue = toml::from_str(toml_str).unwrap();
        assert_eq!(use_leader_from_toml_opt(&root), None);
    }

    #[test]
    fn skills_config_default_is_empty() {
        let cfg = SkillsConfig::default();
        assert!(cfg.paths.is_empty());
        assert!(cfg.ignore.is_empty());
    }

    #[test]
    fn skills_config_parses_paths_and_ignore() {
        let root = toml::from_str::<TomlValue>(
            r#"
[skills]
paths = ["~/.grok/skills", "~/.grok/skills/special/SKILL.md"]
ignore = ["~/.grok/skills/noisy/SKILL.md"]
"#,
        )
        .unwrap();
        let TomlValue::Table(ref table) = root else {
            panic!()
        };
        let cfg = table
            .get("skills")
            .and_then(|v| v.clone().try_into::<SkillsConfig>().ok())
            .unwrap_or_default();
        assert_eq!(
            cfg.paths,
            vec!["~/.grok/skills", "~/.grok/skills/special/SKILL.md"]
        );
        assert_eq!(cfg.ignore, vec!["~/.grok/skills/noisy/SKILL.md"]);
    }

    #[test]
<<<<<<< fork
    fn test_project_scoped_mcp_preserves_unrelated_global_servers() {
        let global_toml = r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
enabled = true

[mcp_servers.buildkite]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.buildkite.com/mcp"]
enabled = true
"#;
        let global_root = toml::from_str::<TomlValue>(global_toml).unwrap();
        let global_servers = parse_mcp_servers_from_toml(&global_root);

        // Project only overrides linear
        let project_toml = r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
enabled = false
"#;
        let project_root = toml::from_str::<TomlValue>(project_toml).unwrap();
        let project_servers = parse_mcp_servers_from_toml(&project_root);

        let mut merged: IndexMap<String, McpServerConfig> = IndexMap::new();
        for (name, config) in &global_servers {
            merged.insert(name.clone(), config.clone());
        }
        for (name, config) in &project_servers {
            merged.insert(name.clone(), config.clone());
        }

        assert_eq!(merged.len(), 2);
        assert!(merged.get("buildkite").unwrap().enabled);
        assert!(!merged.get("linear").unwrap().enabled);
    }

    #[test]
    fn test_mcp_server_config_parses_tool_timeouts() {
        let toml_str = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
tool_timeout_sec = 60
tool_timeouts = { create_issue = 120, search_repositories = 30 }
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let github = servers.get("github").unwrap();

        assert_eq!(github.tool_timeout_sec, Some(60));
        let tt = github.tool_timeouts.as_ref().unwrap();
        assert_eq!(tt.get("create_issue"), Some(&120));
        assert_eq!(tt.get("search_repositories"), Some(&30));
        assert_eq!(tt.get("nonexistent"), None);
    }

    #[test]
    fn test_mcp_server_config_tool_timeouts_defaults_to_none() {
        let toml_str = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let fs = servers.get("filesystem").unwrap();

        assert!(fs.tool_timeouts.is_none());
        assert!(fs.tool_timeout_sec.is_none());
        assert!(fs.expose_image_base64.is_none());
    }

    #[test]
    fn test_mcp_server_config_parses_expose_image_base64() {
        let toml_str = r#"
[mcp_servers.grafana]
url = "https://grafana.example/mcp"
expose_image_base64 = true
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let grafana = servers.get("grafana").unwrap();
        assert_eq!(grafana.expose_image_base64, Some(true));
    }

    #[test]
    fn mcp_json_oauth_block_parsed_into_oauth_config() {
        let json = r#"{
            "mcpServers": {
                "slack": {
                    "type": "http",
                    "url": "https://mcp.slack.example/mcp",
                    "oauth": { "clientId": "slack-byo-client", "callbackPort": 3118 }
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).expect("parse .mcp.json");
        let slack = config.mcp_servers.get("slack").expect("slack server");

        let block = slack.oauth.as_ref().expect("oauth block parsed");
        assert_eq!(block.client_id.as_deref(), Some("slack-byo-client"));
        assert_eq!(block.callback_port, Some(3118));

        let oauth = slack.oauth_config().expect("oauth_config from block");
        assert_eq!(oauth.client_id.as_deref(), Some("slack-byo-client"));
        assert_eq!(oauth.callback_port, Some(3118));
    }

    #[test]
    fn load_cursor_mcp_servers_as_configs_parses_cursor_mcp_json() {
        // Overriding HOME/USERPROFILE mutates process-global env and races parallel tests
        // So this tests the underlying read_mcp_json to McpConfig round-trip instead of the home-anchored global path
        let dir = tempfile::tempdir().unwrap();
        let mcp_json_path = dir.path().join("mcp.json");
        std::fs::write(
            &mcp_json_path,
            r#"{
                "mcpServers": {
                    "test_server": {
                        "command": "node",
                        "args": ["server.js"]
                    }
                }
            }"#,
        )
        .unwrap();
        let config = read_mcp_json(&mcp_json_path).expect("should parse cursor mcp.json");
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("test_server"));
    }

    /// Path-based loader used by kill-switch attribution (no mcps/import gates).
    #[test]
    fn load_claude_json_mcp_servers_from_reads_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let claude_json = dir.path().join("claude.json");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        // Build via serde so Windows paths with `\` are JSON-escaped correctly (string replace into a JSON literal produces invalid escapes)
        let cwd_key = cwd.to_string_lossy().into_owned();
        let fixture = serde_json::json!({
            "mcpServers": {
                "killswitch-claude": { "command": "true" }
            },
            "projects": {
                cwd_key: {
                    "mcpServers": {
                        "killswitch-claude-proj": { "command": "true" }
                    }
                }
            }
        });
        std::fs::write(
            &claude_json,
            serde_json::to_string(&fixture).expect("serialize claude.json fixture"),
        )
        .unwrap();

        let servers = load_claude_json_mcp_servers_from(&claude_json, &cwd);
        let names: Vec<_> = servers
            .iter()
            .map(|s| match s {
                acp::McpServer::Stdio(stdio) => stdio.name.as_str(),
                acp::McpServer::Http(http) => http.name.as_str(),
                acp::McpServer::Sse(sse) => sse.name.as_str(),
                _ => "",
            })
            .collect();
        assert!(
            names.contains(&"killswitch-claude"),
            "user-level server must load: {names:?}"
        );
        assert!(
            names.contains(&"killswitch-claude-proj"),
            "project-scoped server must load: {names:?}"
        );
    }

    #[test]
    fn load_cursor_mcp_servers_as_configs_returns_empty_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mcp_json_path = dir.path().join("mcp.json");
        assert!(read_mcp_json(&mcp_json_path).is_none());
    }

    #[test]
    fn mcp_json_env_var_default_value() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp_path = tmp.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
                "mcpServers": {
                    "api": {
                        "url": "${GROK_TEST_MCP_UNSET_VAR_12345:-https://fallback.example.com}/mcp"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_json_file(&mcp_path);
        assert_eq!(servers.len(), 1);
        let Some(server) = servers.first() else {
            panic!("expected one server: {servers:?}");
        };
        match server {
            acp::McpServer::Http(acp::McpServerHttp { url, .. }) => {
                assert_eq!(url, "https://fallback.example.com/mcp");
            }
            _other => panic!("expected Http"),
        }
    }

    #[test]
    fn mcp_json_all_toml_names_includes_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let grok_dir = tmp.path().join(".grok");
        std::fs::create_dir_all(&grok_dir).unwrap();
        std::fs::write(
            grok_dir.join("config.toml"),
            r#"
[mcp_servers.enabled_one]
url = "https://example.com"

[mcp_servers.disabled_one]
command = "/ignored"
enabled = false
"#,
        )
        .unwrap();
        gix::init(tmp.path()).unwrap();

        let names = all_toml_mcp_server_names(tmp.path());
        assert!(names.contains("enabled_one"));
        assert!(names.contains("disabled_one"));
    }

    #[test]
||||||| base
    fn test_project_scoped_mcp_preserves_unrelated_global_servers() {
        let global_toml = r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
enabled = true

[mcp_servers.buildkite]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.buildkite.com/mcp"]
enabled = true
"#;
        let global_root = toml::from_str::<TomlValue>(global_toml).unwrap();
        let global_servers = parse_mcp_servers_from_toml(&global_root);

        // Project only overrides linear
        let project_toml = r#"
[mcp_servers.linear]
command = "npx"
args = ["-y", "mcp-remote", "https://mcp.linear.app/mcp"]
enabled = false
"#;
        let project_root = toml::from_str::<TomlValue>(project_toml).unwrap();
        let project_servers = parse_mcp_servers_from_toml(&project_root);

        let mut merged: IndexMap<String, McpServerConfig> = IndexMap::new();
        for (name, config) in &global_servers {
            merged.insert(name.clone(), config.clone());
        }
        for (name, config) in &project_servers {
            merged.insert(name.clone(), config.clone());
        }

        assert_eq!(merged.len(), 2);
        assert!(merged.get("buildkite").unwrap().enabled);
        assert!(!merged.get("linear").unwrap().enabled);
    }

    #[test]
    fn test_mcp_server_config_parses_tool_timeouts() {
        let toml_str = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
tool_timeout_sec = 60
tool_timeouts = { create_issue = 120, search_repositories = 30 }
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let github = servers.get("github").unwrap();

        assert_eq!(github.tool_timeout_sec, Some(60));
        let tt = github.tool_timeouts.as_ref().unwrap();
        assert_eq!(tt.get("create_issue"), Some(&120));
        assert_eq!(tt.get("search_repositories"), Some(&30));
        assert_eq!(tt.get("nonexistent"), None);
    }

    #[test]
    fn test_mcp_server_config_tool_timeouts_defaults_to_none() {
        let toml_str = r#"
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let fs = servers.get("filesystem").unwrap();

        assert!(fs.tool_timeouts.is_none());
        assert!(fs.tool_timeout_sec.is_none());
        assert!(fs.expose_image_base64.is_none());
    }

    #[test]
    fn test_mcp_server_config_parses_expose_image_base64() {
        let toml_str = r#"
[mcp_servers.grafana]
url = "https://grafana.example/mcp"
expose_image_base64 = true
"#;
        let root = toml::from_str::<TomlValue>(toml_str).unwrap();
        let servers = parse_mcp_servers_from_toml(&root);
        let grafana = servers.get("grafana").unwrap();
        assert_eq!(grafana.expose_image_base64, Some(true));
    }

    #[test]
    fn mcp_json_oauth_block_parsed_into_oauth_config() {
        let json = r#"{
            "mcpServers": {
                "slack": {
                    "type": "http",
                    "url": "https://mcp.slack.example/mcp",
                    "oauth": { "clientId": "slack-byo-client", "callbackPort": 3118 }
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).expect("parse .mcp.json");
        let slack = config.mcp_servers.get("slack").expect("slack server");

        let block = slack.oauth.as_ref().expect("oauth block parsed");
        assert_eq!(block.client_id.as_deref(), Some("slack-byo-client"));
        assert_eq!(block.callback_port, Some(3118));

        let oauth = slack.oauth_config().expect("oauth_config from block");
        assert_eq!(oauth.client_id.as_deref(), Some("slack-byo-client"));
        assert_eq!(oauth.callback_port, Some(3118));
    }

    #[test]
    fn load_cursor_mcp_servers_as_configs_parses_cursor_mcp_json() {
        // Overriding HOME/USERPROFILE mutates process-global env and races parallel tests
        // So this tests the underlying read_mcp_json to McpConfig round-trip instead of the home-anchored global path
        let dir = tempfile::tempdir().unwrap();
        let mcp_json_path = dir.path().join("mcp.json");
        std::fs::write(
            &mcp_json_path,
            r#"{
                "mcpServers": {
                    "test_server": {
                        "command": "node",
                        "args": ["server.js"]
                    }
                }
            }"#,
        )
        .unwrap();
        let config = read_mcp_json(&mcp_json_path).expect("should parse cursor mcp.json");
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("test_server"));
    }

    /// Path-based loader used by kill-switch attribution (no mcps/import gates).
    #[test]
    fn load_claude_json_mcp_servers_from_reads_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let claude_json = dir.path().join("claude.json");
        let cwd = dir.path().join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        // Build via serde so Windows paths with `\` are JSON-escaped correctly (string replace into a JSON literal produces invalid escapes)
        let cwd_key = cwd.to_string_lossy().into_owned();
        let fixture = serde_json::json!({
            "mcpServers": {
                "killswitch-claude": { "command": "true" }
            },
            "projects": {
                cwd_key: {
                    "mcpServers": {
                        "killswitch-claude-proj": { "command": "true" }
                    }
                }
            }
        });
        std::fs::write(
            &claude_json,
            serde_json::to_string(&fixture).expect("serialize claude.json fixture"),
        )
        .unwrap();

        let servers = load_claude_json_mcp_servers_from(&claude_json, &cwd);
        let names: Vec<_> = servers
            .iter()
            .map(|s| match s {
                acp::McpServer::Stdio(stdio) => stdio.name.as_str(),
                acp::McpServer::Http(http) => http.name.as_str(),
                acp::McpServer::Sse(sse) => sse.name.as_str(),
                _ => "",
            })
            .collect();
        assert!(
            names.contains(&"killswitch-claude"),
            "user-level server must load: {names:?}"
        );
        assert!(
            names.contains(&"killswitch-claude-proj"),
            "project-scoped server must load: {names:?}"
        );
    }

    #[test]
    fn load_cursor_mcp_servers_as_configs_returns_empty_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let mcp_json_path = dir.path().join("mcp.json");
        assert!(read_mcp_json(&mcp_json_path).is_none());
    }

    #[test]
    fn mcp_json_env_var_default_value() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp_path = tmp.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
                "mcpServers": {
                    "api": {
                        "url": "${GROK_TEST_MCP_UNSET_VAR_12345:-https://fallback.example.com}/mcp"
                    }
                }
            }"#,
        )
        .unwrap();

        let servers = load_mcp_json_file(&mcp_path);
        assert_eq!(servers.len(), 1);
        let Some(server) = servers.first() else {
            panic!("expected one server: {servers:?}");
        };
        match server {
            acp::McpServer::Http(acp::McpServerHttp { url, .. }) => {
                assert_eq!(url, "https://fallback.example.com/mcp");
            }
            _other => panic!("expected Http"),
        }
    }

    #[test]
    fn mcp_json_all_toml_names_includes_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let grok_dir = tmp.path().join(".grok");
        std::fs::create_dir_all(&grok_dir).unwrap();
        std::fs::write(
            grok_dir.join("config.toml"),
            r#"
[mcp_servers.enabled_one]
url = "https://example.com"

[mcp_servers.disabled_one]
command = "/ignored"
enabled = false
"#,
        )
        .unwrap();
        git2::Repository::init(tmp.path()).unwrap();

        let names = all_toml_mcp_server_names(tmp.path());
        assert!(names.contains("enabled_one"));
        assert!(names.contains("disabled_one"));
    }

    #[test]
=======
>>>>>>> upstream
    fn mcp_json_candidate_paths_include_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        gix::init(tmp.path()).unwrap();

        let paths = mcp_json_candidate_paths(&nested);
        assert_eq!(
            paths,
            vec![
                tmp.path().join(".mcp.json"),
                tmp.path().join("a").join(".mcp.json"),
                nested.join(".mcp.json"),
            ]
        );
    }

    #[tokio::test]
    async fn mcp_preferences_save_refuses_corrupt_file_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp_preferences.json");
        std::fs::write(&path, "not json").unwrap();
        let prefs = McpPreferencesFile {
            version: 1,
            servers: HashMap::from([(
                "acme".to_string(),
                McpServerPreferences {
                    values: HashMap::from([("site".to_string(), "us5".to_string())]),
                    source: Some(McpPreferenceSource {
                        kind: "plugin".to_string(),
                        plugin: Some("acme".to_string()),
                        scope: None,
                    }),
                    updated_at: Some("2026-06-19T00:00:00Z".to_string()),
                },
            )]),
        };

        assert!(save_mcp_preferences_to(&path, &prefs).await.is_err());

        std::fs::remove_file(&path).unwrap();
        save_mcp_preferences_to(&path, &prefs).await.unwrap();
        let loaded = load_mcp_preferences_from(&path).file();
        let Some(acme) = loaded.servers.get("acme") else {
            panic!("expected acme server: {:?}", loaded.servers);
        };
        assert_eq!(acme.values.get("site").map(String::as_str), Some("us5"));
        assert_eq!(
            acme.source.as_ref().and_then(|s| s.plugin.as_deref()),
            Some("acme")
        );
    }

    #[tokio::test]
    async fn save_mcp_server_config_at_refuses_an_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[models\ndefault = \"grok-4.6\"\n").unwrap();
        let config = McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "/bin/echo".to_string(),
                args: vec![],
                env: None,
                cwd: None,
            },
            enabled: true,
            oauth: None,
            setup: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            tool_timeouts: None,
            expose_image_base64: None,
        };

        let err = save_mcp_server_config_at(&path, "qa-echo", &config)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("unparseable"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[models\ndefault = \"grok-4.6\"\n",
            "the file must be untouched"
        );
        let err = delete_mcp_server_config_at(&path, "qa-echo")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unparseable"), "{err}");
    }

    #[test]
    fn apply_mcp_server_enabled_updates_array_and_per_server_field() {
        let mut root: TomlValue = toml::from_str(
            r#"
[mcp_servers.local]
command = "npx"
enabled = true
"#,
        )
        .unwrap();
        let table = root.as_table_mut().unwrap();

        apply_mcp_server_enabled(table, "local", false);
        let disabled = table
            .get("disabled_mcp_servers")
            .and_then(|v| v.as_array())
            .expect("disabled_mcp_servers array");
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled.first().and_then(|v| v.as_str()), Some("local"));
        assert_eq!(
            table
                .get("mcp_servers")
                .and_then(|v| v.as_table())
                .and_then(|s| s.get("local"))
                .and_then(|v| v.as_table())
                .and_then(|s| s.get("enabled"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        apply_mcp_server_enabled(table, "local", true);
        assert!(table.get("disabled_mcp_servers").is_none());
        assert_eq!(
            table
                .get("mcp_servers")
                .and_then(|v| v.as_table())
                .and_then(|s| s.get("local"))
                .and_then(|v| v.as_table())
                .and_then(|s| s.get("enabled"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn apply_mcp_server_enabled_managed_name_only_updates_array() {
        let mut root: TomlValue = toml::from_str("disabled_mcp_servers = []\n").unwrap();
        let table = root.as_table_mut().unwrap();

        apply_mcp_server_enabled(table, "grok_com_slack", false);
        let disabled = table
            .get("disabled_mcp_servers")
            .and_then(|v| v.as_array())
            .expect("disabled_mcp_servers array");
        assert_eq!(disabled.len(), 1);
        assert_eq!(
            disabled.first().and_then(|v| v.as_str()),
            Some("grok_com_slack")
        );
        assert!(table.get("mcp_servers").is_none());

        apply_mcp_server_enabled(table, "grok_com_slack", true);
        assert!(table.get("disabled_mcp_servers").is_none());
        assert!(table.get("mcp_servers").is_none());
    }

    #[tokio::test]
    async fn clear_sticky_project_disabled_at_only_flips_false() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"mcp": {"servers": {"proj": {"command": "npx", "enabled": false}}}}"#,
        )
        .unwrap();
        assert!(
            clear_sticky_project_disabled_at(&path, "proj")
                .await
                .unwrap()
        );
        let body = std::fs::read_to_string(&path).unwrap();
<<<<<<< fork
        assert!(body.contains("\"enabled\": true"), "{body}");
        // Already true: no write.
||||||| base
        assert!(body.contains("enabled = true"), "{body}");
        // Already true: no write.
=======
        assert!(body.contains("enabled = true"), "{body}");
>>>>>>> upstream
        assert!(
            !clear_sticky_project_disabled_at(&path, "proj")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn enable_unstick_only_touches_nearest_project_definition() {
        let tmp = tempfile::tempdir().unwrap();
        gix::init(tmp.path()).unwrap();
        let nested = tmp.path().join("pkg");
        std::fs::create_dir_all(nested.join(".grok")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".grok")).unwrap();

        let sticky = r#"{"ui": {"theme": "keep-me"}, "mcp": {"servers": {"svc": {"command": "npx", "enabled": false}}}}"#;
        let ancestor = tmp.path().join(".grok").join("config.json");
        let nearer = nested.join(".grok").join("config.json");
        std::fs::write(&ancestor, sticky).unwrap();
        std::fs::write(&nearer, sticky).unwrap();

        assert_eq!(
            nearest_project_mcp_definition(&nested, "svc").as_ref(),
            Some(&nearer)
        );

        let path = nearest_project_mcp_definition(&nested, "svc").unwrap();
        clear_sticky_project_disabled_at(&path, "svc")
            .await
            .unwrap();

        let nearer_body = std::fs::read_to_string(&nearer).unwrap();
        assert!(
            nearer_body.contains("\"enabled\": true"),
            "nearest should be unstuck: {nearer_body}"
        );
        assert!(
            nearer_body.contains("keep-me"),
            "unrelated fields must survive: {nearer_body}"
        );
        let ancestor_body = std::fs::read_to_string(&ancestor).unwrap();
        assert!(
            ancestor_body.contains("\"enabled\": false"),
            "shadowed ancestor must stay sticky: {ancestor_body}"
        );

        set_sticky_project_disabled_at(&nearer, "svc")
            .await
            .unwrap();
        let re_stuck = std::fs::read_to_string(&nearer).unwrap();
        assert!(
            re_stuck.contains("\"enabled\": false"),
            "re-stick must set enabled=false: {re_stuck}"
        );
        assert!(
            re_stuck.contains("keep-me"),
            "re-stick must preserve unrelated fields: {re_stuck}"
        );
    }

    #[tokio::test]
    async fn restore_mcp_server_enabled_after_enable_scopes_tiers() {
        let project = tempfile::tempdir().unwrap();
        let project_cfg = project.path().join("config.json");
        std::fs::write(
            &project_cfg,
            r#"{"ui": {"theme": "keep-me"}, "mcp": {"servers": {"svc": {"command": "true", "enabled": false}}}}"#,
        )
        .unwrap();

        clear_sticky_project_disabled_at(&project_cfg, "svc")
            .await
            .unwrap();
        let unstuck = std::fs::read_to_string(&project_cfg).unwrap();
        assert!(unstuck.contains("\"enabled\": true"), "{unstuck}");
        assert!(unstuck.contains("keep-me"), "{unstuck}");

        restore_mcp_server_enabled_after_enable("svc", std::slice::from_ref(&project_cfg))
            .await
            .unwrap();

        let project_body = std::fs::read_to_string(&project_cfg).unwrap();
        assert!(
            project_body.contains("\"enabled\": false"),
            "{project_body}"
        );
        assert!(project_body.contains("keep-me"), "{project_body}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn project_enable_replaces_config_symlink_not_referent() {
        let tmp = tempfile::tempdir().unwrap();
        gix::init(tmp.path()).unwrap();
        let grok = tmp.path().join(".grok");
        std::fs::create_dir_all(&grok).unwrap();
        let outside = tmp.path().join("outside.json");
        std::fs::write(
            &outside,
            r#"{"mcp": {"servers": {"svc": {"command": "true", "enabled": false}}}}"#,
        )
        .unwrap();
        let project_cfg = grok.join("config.json");
        std::os::unix::fs::symlink(&outside, &project_cfg).unwrap();

        assert!(
            clear_sticky_project_disabled_at(&project_cfg, "svc")
                .await
                .unwrap()
        );
        assert!(
            !std::fs::symlink_metadata(&project_cfg)
                .unwrap()
                .file_type()
                .is_symlink(),
            "project slot must become a regular file"
        );
        let body = std::fs::read_to_string(&project_cfg).unwrap();
        assert!(body.contains("\"enabled\": true"), "{body}");
        assert!(
            std::fs::read_to_string(&outside)
                .unwrap()
                .contains("\"enabled\": false"),
            "external referent must stay sticky-disabled"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn persist_mcp_toml_follows_user_config_symlink() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let outside = home.path().join("dotfiles").join("config.json");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(
            &outside,
            r#"{"mcp": {"servers": {"keep": {"command": "true"}}}}"#,
        )
        .unwrap();
        let slot = home.path().join("config.json");
        std::os::unix::fs::symlink(&outside, &slot).unwrap();

        persist_mcp_toml(
            &config_path(),
            r#"{"mcp": {"servers": {"keep": {"command": "true", "enabled": false}}}}"#,
        )
        .await
        .unwrap();
        assert!(
            std::fs::symlink_metadata(&slot)
                .unwrap()
                .file_type()
                .is_symlink(),
            "user slot must stay a symlink"
        );
        assert_eq!(outside, std::fs::read_link(&slot).unwrap());
        assert!(
            std::fs::read_to_string(&outside)
                .unwrap()
                .contains("\"enabled\": false")
        );
    }

    fn test_stdio_server() -> McpServerConfig {
        toml::from_str("command = \"true\"\n").expect("stdio fixture")
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn save_mcp_server_config_at_refuses_unparseable() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let slot = home.path().join("config.json");
        std::fs::write(&slot, "{not valid\n").unwrap();
        let err = save_mcp_server_config_at(&config_path(), "svc", &test_stdio_server())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unparseable"),
            "expected refuse, got {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&slot).unwrap(),
            "{not valid\n",
            "file must be left intact"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn save_mcp_server_config_at_follows_user_symlink() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let outside = home.path().join("dotfiles").join("config.json");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(
            &outside,
            r#"{"mcp": {"servers": {"keep": {"command": "true"}}}}"#,
        )
        .unwrap();
        let slot = home.path().join("config.json");
        std::os::unix::fs::symlink(&outside, &slot).unwrap();

        save_mcp_server_config_at(&config_path(), "svc", &test_stdio_server())
            .await
            .unwrap();
        assert!(
            std::fs::symlink_metadata(&slot)
                .unwrap()
                .file_type()
                .is_symlink(),
            "user slot must stay a symlink"
        );
        let body = std::fs::read_to_string(&outside).unwrap();
        assert!(body.contains("\"svc\""), "{body}");
        assert!(body.contains("\"keep\""), "{body}");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn save_mcp_disabled_tools_follows_user_symlink() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let outside = home.path().join("dotfiles").join("config.json");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(
            &outside,
            r#"{"mcp": {"servers": {"keep": {"command": "true"}}}}"#,
        )
        .unwrap();
        let slot = home.path().join("config.json");
        std::os::unix::fs::symlink(&outside, &slot).unwrap();

        save_mcp_disabled_tools("keep", &["tool_a".to_string()])
            .await
            .unwrap();
        assert!(
            std::fs::symlink_metadata(&slot)
                .unwrap()
                .file_type()
                .is_symlink(),
            "user slot must stay a symlink"
        );
        let body = std::fs::read_to_string(&outside).unwrap();
        assert!(body.contains("\"keep\""), "{body}");
        assert!(body.contains("\"disabled_tools\""), "{body}");
        assert!(body.contains("tool_a"), "{body}");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn save_mcp_disabled_tools_refuses_unparseable() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let slot = home.path().join("config.json");
        std::fs::write(&slot, "{not valid\n").unwrap();
        let err = save_mcp_disabled_tools("svc", &["tool_a".to_string()])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unparseable"),
            "expected refuse, got {err}"
        );
        assert_eq!(std::fs::read_to_string(&slot).unwrap(), "{not valid\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn delete_mcp_server_config_at_follows_user_symlink() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let outside = home.path().join("dotfiles").join("config.json");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(
            &outside,
            r#"{"mcp": {"servers": {"keep": {"command": "true"}, "svc": {"command": "true"}}}}"#,
        )
        .unwrap();
        let slot = home.path().join("config.json");
        std::os::unix::fs::symlink(&outside, &slot).unwrap();

        assert!(
            delete_mcp_server_config_at(&config_path(), "svc")
                .await
                .unwrap()
        );
        assert!(
            std::fs::symlink_metadata(&slot)
                .unwrap()
                .file_type()
                .is_symlink(),
            "user slot must stay a symlink"
        );
        let body = std::fs::read_to_string(&outside).unwrap();
        assert!(!body.contains("\"svc\""), "{body}");
        assert!(body.contains("\"keep\""), "{body}");
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn delete_mcp_server_config_at_refuses_unparseable() {
        let home = tempfile::tempdir().unwrap();
        let _env = xai_grok_test_support::EnvGuard::set("GROK_HOME", home.path());
        let slot = home.path().join("config.json");
        std::fs::write(&slot, "{not valid\n").unwrap();
        let err = delete_mcp_server_config_at(&config_path(), "svc")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unparseable"),
            "expected refuse, got {err}"
        );
        assert_eq!(std::fs::read_to_string(&slot).unwrap(), "{not valid\n");
    }

    #[tokio::test]
    async fn write_toml_table_if_changed_refuses_unparseable() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.json");
        std::fs::write(&path, "{not valid\n").unwrap();
        let err = write_toml_table_if_changed(&path, |_t| {})
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unparseable"),
            "expected refuse, got {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{not valid\n",
            "file must be left intact"
        );
    }
}
