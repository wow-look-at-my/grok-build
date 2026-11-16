use agent_client_protocol as acp;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;

use crate::mcp_servers::{McpServerOAuth, McpServerTransport};

/// The external `acp::McpServer` type has no field for OAuth client settings.
#[derive(Debug, Clone, Default)]
pub struct McpOAuthConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub callback_port: Option<u16>,
}

impl McpOAuthConfig {
    pub fn is_configured(&self) -> bool {
        self.client_id.is_some()
    }
}

pub type McpOAuthConfigMap = HashMap<String, McpOAuthConfig>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged, try_from = "McpServerTransportConfigWire")]
pub enum McpServerTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<HashMap<String, String>>,
        /// The ACP stdio server config has no `cwd` field yet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    StreamableHttp {
        /// The server's URL.
        url: String,
        #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
        transport_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_env_var: Option<String>,
        /// Re-read on every request, so a token rotated on disk applies without a reconnect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_token_file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers: Option<HashMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_client_secret_env_var: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_scopes: Option<Vec<String>>,
    },
}

// Written by hand: the derived schema of the wire type marks every HTTP key optional, so it would accept any object
impl schemars::JsonSchema for McpServerTransportConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "McpServerTransportConfig".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let strings = serde_json::json!({ "type": "array", "items": { "type": "string" } });
        let string_map =
            serde_json::json!({ "type": "object", "additionalProperties": { "type": "string" } });
        // No member is required: one layer can set only `enabled` on a server that another layer defines
        schemars::json_schema!({
            "description": "A stdio server sets `command`; a streamable HTTP server sets `url` (`urlTemplate` and `url_template` are other spellings).",
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "args": strings,
                "env": string_map,
                "cwd": { "type": "string" },
                "url": { "type": "string" },
                "urlTemplate": { "type": "string" },
                "url_template": { "type": "string" },
                "type": { "type": "string" },
                "bearer_token_env_var": {
                    "description": "Name of the environment variable to read and set for `Authorization: Bearer <token>`.",
                    "type": "string"
                },
                "bearer_token_file": {
                    "description": "Path of a file that holds the bearer token. It is read again on every request.",
                    "type": "string"
                },
                "headers": string_map,
                "oauth_client_id": { "type": "string" },
                "oauth_client_secret_env_var": { "type": "string" },
                "oauth_scopes": strings
            }
        })
    }
}

impl McpServerTransportConfig {
    /// The keys the streamable-HTTP transport reads its URL under.
    pub const URL_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("url", &["urlTemplate", "url_template"]);
}

/// `McpServerTransportConfig` with each URL spelling as its own field.
/// It stays `untagged`, and the fold happens in the outer `TryFrom`, not inside a variant.
/// An untagged variant that fails reports only "data did not match any variant", so the conflict text would be lost.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum McpServerTransportConfigWire {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
        #[serde(default)]
        cwd: Option<String>,
    },
    StreamableHttp {
        #[serde(default)]
        url: Option<String>,
        #[serde(default, rename = "urlTemplate")]
        url_template_camel: Option<String>,
        #[serde(default, rename = "url_template")]
        url_template_snake: Option<String>,
        #[serde(default, rename = "type")]
        transport_type: Option<String>,
        #[serde(default)]
        bearer_token_env_var: Option<String>,
        #[serde(default)]
        bearer_token_file: Option<String>,
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
        #[serde(default)]
        oauth_client_id: Option<String>,
        #[serde(default)]
        oauth_client_secret_env_var: Option<String>,
        #[serde(default)]
        oauth_scopes: Option<Vec<String>>,
    },
}

/// Why a transport block could not become a [`McpServerTransportConfig`].
#[derive(Debug)]
pub enum McpTransportConfigError {
    Alias(xai_tool_types::AliasConflict),
    /// `url` is required: an HTTP server with no address is not a server.
    MissingUrl,
    /// `command` is required for the stdio arm.
    MissingCommand,
}

impl std::fmt::Display for McpTransportConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alias(conflict) => conflict.fmt(f),
            Self::MissingUrl => f.write_str("missing field `url`"),
            Self::MissingCommand => f.write_str("missing field `command`"),
        }
    }
}

impl std::error::Error for McpTransportConfigError {}

impl From<xai_tool_types::AliasConflict> for McpTransportConfigError {
    fn from(value: xai_tool_types::AliasConflict) -> Self {
        Self::Alias(value)
    }
}

impl TryFrom<McpServerTransportConfigWire> for McpServerTransportConfig {
    type Error = McpTransportConfigError;

    fn try_from(wire: McpServerTransportConfigWire) -> Result<Self, Self::Error> {
        Ok(match wire {
            McpServerTransportConfigWire::Stdio {
                command,
                args,
                env,
                cwd,
            } => Self::Stdio {
                command,
                args,
                env,
                cwd,
            },
            McpServerTransportConfigWire::StreamableHttp {
                url,
                url_template_camel,
                url_template_snake,
                transport_type,
                bearer_token_env_var,
                bearer_token_file,
                headers,
                oauth_client_id,
                oauth_client_secret_env_var,
                oauth_scopes,
            } => Self::StreamableHttp {
                // An HTTP entry that names none of the keys has no address, so it is an error, not a default.
                url: McpServerTransportConfig::URL_KEYS
                    .fold(vec![url, url_template_camel, url_template_snake])?
                    .ok_or(McpTransportConfigError::MissingUrl)?,
                transport_type,
                bearer_token_env_var,
                bearer_token_file,
                headers,
                oauth_client_id,
                oauth_client_secret_env_var,
                oauth_scopes,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpServerProblemSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfigProblem {
    pub server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub severity: McpServerProblemSeverity,
    pub message: String,
}

/// Listed by hand because `serde_ignored` cannot see unknown keys inside the flattened untagged transport enum.
pub const KNOWN_MCP_SERVER_FIELDS: &[&str] = &[
    "args",
    "bearer_token_env_var",
    "bearer_token_file",
    "command",
    "cwd",
    "enabled",
    "env",
    "expose_image_base64",
    "headers",
    "oauth",
    "oauth_client_id",
    "oauth_client_secret_env_var",
    "oauth_scopes",
    "setup",
    "startup_timeout_sec",
    "tool_timeout_sec",
    "tool_timeouts",
    "type",
    "url",
    "urlTemplate",
    "url_template",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpJsonOAuthBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_port: Option<u16>,
}

/// A server's setup prompt: the fields it asks for, and the answers already recorded under them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(try_from = "McpSetupConfigWire")]
pub struct McpSetupConfig {
    #[serde(default)]
    pub fields: Vec<McpSetupField>,
    /// The recorded answers.
    #[serde(default)]
    pub variables: HashMap<String, McpSetupDerivedValue>,
}

impl McpSetupConfig {
    /// The keys [`variables`](Self::variables) is read under.
    pub const VARIABLES_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("variables", &["values"]);
}

/// `McpSetupConfig` with each answers-key spelling as its own field.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct McpSetupConfigWire {
    #[serde(default)]
    fields: Vec<McpSetupField>,
    #[serde(default)]
    variables: Option<HashMap<String, McpSetupDerivedValue>>,
    #[serde(default, rename = "values")]
    values: Option<HashMap<String, McpSetupDerivedValue>>,
}

impl TryFrom<McpSetupConfigWire> for McpSetupConfig {
    type Error = xai_tool_types::AliasConflict;

    fn try_from(wire: McpSetupConfigWire) -> Result<Self, Self::Error> {
        Ok(Self {
            fields: wire.fields,
            variables: McpSetupConfig::VARIABLES_KEYS
                .fold(vec![wire.variables, wire.values])?
                .unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct McpSetupField {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: McpSetupFieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<McpSetupOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum McpSetupFieldType {
    Select,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct McpSetupOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub struct McpSetupDerivedValue {
    pub from: String,
    pub map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpPreferenceSource {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerPreferences {
    #[serde(default)]
    pub values: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<McpPreferenceSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpPreferencesFile {
    pub version: u32,
    #[serde(default)]
    pub servers: HashMap<String, McpServerPreferences>,
}

impl Default for McpPreferencesFile {
    fn default() -> Self {
        Self {
            version: 1,
            servers: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum McpSetupResolution {
    Resolved(Box<McpServerConfig>),
    Required(McpSetupConfig),
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpServerConfig {
    #[serde(flatten)]
    pub transport: McpServerTransportConfig,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpJsonOAuthBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<McpSetupConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_sec: Option<u64>,
    /// Seconds per tool name. Tools not listed here fall back to `tool_timeout_sec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeouts: Option<HashMap<String, u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose_image_base64: Option<bool>,
}

impl McpServerConfig {
    pub fn blank_transport_field(&self) -> Option<&'static str> {
        match &self.transport {
            McpServerTransportConfig::Stdio { command, .. } if command.trim().is_empty() => {
                Some("command")
            }
            McpServerTransportConfig::StreamableHttp { url, .. } if url.trim().is_empty() => {
                Some("url")
            }
            _ => None,
        }
    }
}

impl McpServerConfig {
    /// Only a schema with one select field resolves until the TUI can collect more fields.
    pub fn resolve_setup(&self, preferences: Option<&McpServerPreferences>) -> McpSetupResolution {
        let Some(setup) = self.setup.as_ref() else {
            return McpSetupResolution::Resolved(Box::new(self.clone()));
        };

        let [field] = setup.fields.as_slice() else {
            return McpSetupResolution::Invalid(
                "setup schema must declare exactly one select field (v0)".to_string(),
            );
        };
        if !matches!(field.field_type, McpSetupFieldType::Select) || field.options.is_empty() {
            return McpSetupResolution::Invalid(
                "setup field must be a non-empty select (v0)".to_string(),
            );
        }

        let Some(preferences) = preferences else {
            return McpSetupResolution::Required(setup.clone());
        };
        let Some(value) = preferences.values.get(&field.id) else {
            return McpSetupResolution::Required(setup.clone());
        };
        if !field.options.iter().any(|option| option.value == *value) {
            return McpSetupResolution::Required(setup.clone());
        }

        let mut variables = HashMap::new();
        for (name, derived) in &setup.variables {
            if derived.from != field.id {
                return McpSetupResolution::Invalid(format!(
                    "setup variable '{name}' references unknown field '{}'",
                    derived.from
                ));
            }
            let Some(mapped) = derived.map.get(value) else {
                return McpSetupResolution::Required(setup.clone());
            };
            variables.insert(name.clone(), mapped.clone());
        }

        let mut resolved = self.clone();
        resolved.setup = None;
        match render_setup_templates(&mut resolved, &variables) {
            Ok(()) => McpSetupResolution::Resolved(Box::new(resolved)),
            Err(e) => McpSetupResolution::Invalid(e),
        }
    }

    pub fn expand_strings(&mut self, sub: &dyn Fn(&str) -> String) {
        let Ok(()) = self.try_for_each_substitutable_field::<Infallible>(|value| {
            *value = sub(value);
            Ok(())
        });
    }

    /// Every transport string that `${VAR}` expansion and setup templates rewrite.
    /// Both passes walk this one list so they cannot drift apart.
    fn try_for_each_substitutable_field<E>(
        &mut self,
        mut visit: impl FnMut(&mut String) -> Result<(), E>,
    ) -> Result<(), E> {
        match &mut self.transport {
            McpServerTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                visit(command)?;
                for arg in args.iter_mut() {
                    visit(arg)?;
                }
                for value in env.iter_mut().flat_map(HashMap::values_mut) {
                    visit(value)?;
                }
                if let Some(cwd) = cwd.as_mut() {
                    visit(cwd)?;
                }
            }
            McpServerTransportConfig::StreamableHttp {
                url,
                headers,
                bearer_token_file,
                ..
            } => {
                visit(url)?;
                for value in headers.iter_mut().flat_map(HashMap::values_mut) {
                    visit(value)?;
                }
                if let Some(path) = bearer_token_file.as_mut() {
                    visit(path)?;
                }
            }
        }
        Ok(())
    }

    pub fn to_acp_mcp_server(&self, name: impl Into<String>) -> Option<acp::McpServer> {
        if !self.enabled || self.setup.is_some() {
            return None;
        }
        let name = name.into();
        Some(self.resolve_transport(&name)?.into_acp_mcp_server(name))
    }

    /// A `bearer_token_env_var` token is the last `Authorization` header.
    pub(crate) fn resolve_transport(&self, name: &str) -> Option<McpServerTransport> {
        match &self.transport {
            McpServerTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Some(McpServerTransport::Stdio {
                command: PathBuf::from(command),
                args: args.clone(),
                env: env
                    .iter()
                    .flatten()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                cwd: cwd.as_ref().map(PathBuf::from),
            }),
            McpServerTransportConfig::StreamableHttp {
                url,
                transport_type,
                bearer_token_env_var,
                bearer_token_file,
                headers,
                ..
            } => {
                if url.is_empty() {
                    return None;
                }
                let mut headers: Vec<(String, String)> = headers
                    .iter()
                    .flatten()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                if let Some(env_var) = bearer_token_env_var
                    && let Some(header) = bearer_authorization_header(name, env_var)
                {
                    headers.push(header);
                }
                let url = url.clone();
                let bearer_token_file = bearer_token_file.clone();
                Some(if is_sse_transport(transport_type.as_deref(), &url) {
                    McpServerTransport::Sse {
                        url,
                        headers,
                        bearer_token_file,
                    }
                } else {
                    McpServerTransport::Http {
                        url,
                        headers,
                        bearer_token_file,
                    }
                })
            }
        }
    }

    pub fn oauth_config(&self) -> Option<McpOAuthConfig> {
        let settings = self.oauth_settings()?;
        Some(McpOAuthConfig {
            client_secret: resolve_oauth_client_secret(settings.client_secret_env_var.as_ref()),
            client_id: Some(settings.client_id),
            scopes: settings.scopes,
            callback_port: settings.callback_port,
        })
    }

    /// The HTTP `oauth_client_*` fields win over the `oauth` block when they set a client id.
    /// Neither applies without a client id.
    pub(crate) fn oauth_settings(&self) -> Option<McpServerOAuth> {
        if let McpServerTransportConfig::StreamableHttp {
            oauth_client_id: Some(client_id),
            oauth_client_secret_env_var,
            oauth_scopes,
            ..
        } = &self.transport
        {
            return Some(McpServerOAuth {
                client_id: client_id.clone(),
                client_secret_env_var: oauth_client_secret_env_var.clone(),
                scopes: oauth_scopes.clone(),
                callback_port: None,
            });
        }
        let block = self.oauth.as_ref()?;
        Some(McpServerOAuth {
            client_id: block.client_id.clone()?,
            client_secret_env_var: block.client_secret_env_var.clone(),
            scopes: block.scopes.clone(),
            callback_port: block.callback_port,
        })
    }
}

fn render_setup_templates(
    config: &mut McpServerConfig,
    variables: &HashMap<String, String>,
) -> Result<(), String> {
    config.try_for_each_substitutable_field(|value| {
        *value = render_setup_template(value, variables)?;
        Ok(())
    })
}

fn render_setup_template(
    input: &str,
    variables: &HashMap<String, String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let (prefix, after_start) = rest.split_at(start);
        out.push_str(prefix);

        let Some(after_start) = after_start.strip_prefix("{{") else {
            return Err("unterminated setup variable template".to_string());
        };
        let Some(end) = after_start.find("}}") else {
            return Err("unterminated setup variable template".to_string());
        };
        let Some(key) = after_start.get(..end) else {
            return Err("unterminated setup variable template".to_string());
        };
        let key = key.trim();
        let Some(value) = variables.get(key) else {
            return Err(format!("unresolved setup variable '{key}'"));
        };
        out.push_str(value);

        let Some(remaining) = after_start.get(end..) else {
            return Err("unterminated setup variable template".to_string());
        };
        let Some(remaining) = remaining.strip_prefix("}}") else {
            return Err("unterminated setup variable template".to_string());
        };
        rest = remaining;
    }
    out.push_str(rest);
    Ok(out)
}

fn bearer_authorization_header(server_name: &str, env_var: &str) -> Option<(String, String)> {
    match std::env::var(env_var) {
        Ok(token) => Some(("Authorization".to_owned(), format!("Bearer {token}"))),
        Err(_) => {
            tracing::warn!(
                "MCP server '{}': bearer_token_env_var '{}' not set in environment",
                server_name,
                env_var
            );
            None
        }
    }
}

fn is_sse_transport(transport_type: Option<&str>, url: &str) -> bool {
    transport_type.is_some_and(|transport| transport.eq_ignore_ascii_case("sse"))
        || url.ends_with("/sse")
}

fn resolve_oauth_client_secret(env_var: Option<&String>) -> Option<String> {
    let env_var = env_var?;
    match std::env::var(env_var) {
        Ok(secret) => Some(secret),
        Err(_) => {
            tracing::warn!(
                env_var = env_var.as_str(),
                "MCP OAuth client_secret env var is configured but not set in the environment; \
                 proceeding without a client secret"
            );
            None
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: IndexMap<String, McpServerConfig>,
}

#[cfg(test)]
#[path = "mcp_server_config_tests.rs"]
mod tests;
