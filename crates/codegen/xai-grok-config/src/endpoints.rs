//! The `[endpoints]` config table, its environment variable overrides, and the URLs resolved from it.
//!
//! The auxiliary services (feedback, trace upload, managed config, telemetry) resolve to the cli-chat-proxy.
//! Only API-key inference uses `xai_api_base_url`.
//! An endpoint nobody configured resolves to a blank URL.
use serde::{Deserialize, Serialize};
use xai_grok_env::{PROD_CLI_CHAT_PROXY_BASE_URL, env_bool, env_string};
pub const CLI_CHAT_PROXY_BASE_URL_DEFAULT: &str = PROD_CLI_CHAT_PROXY_BASE_URL;
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(default)]
pub struct EndpointsConfig {
    /// The cli-chat-proxy base URL. `None` means unset, and every URL derived from it is blank.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_chat_proxy_base_url: Option<String>,
    /// Base URL for the public xAI API. Blank unless configured.
    pub xai_api_base_url: String,
    /// The only endpoints a model request may reach. Empty allows none. `GROK_ALLOWED_ENDPOINTS` adds to it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_endpoints: Vec<String>,
    /// An extra access header value for matching first-party hosts, used only with the optional non-production feature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_test_key: Option<String>,
    /// Env: `GROK_MODELS_BASE_URL`. Setting it makes `has_custom_endpoint` true.
    /// The models list URL defaults to `{models_base_url}/models`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_base_url: Option<String>,
    /// Env: `GROK_MODELS_LIST_URL`. Overrides the default `{base}/models` list URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_list_url: Option<String>,
    /// Env: `GROK_FEEDBACK_BASE_URL`. Where feedback submissions go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_base_url: Option<String>,
    /// Env: `GROK_TRACE_UPLOAD_URL`. Where trace uploads go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_upload_url: Option<String>,
    /// Env: `GROK_MANAGED_CONFIG_URL`. The managed config endpoint.
    /// The default is `{proxy_url()}/deployment/config`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_config_url: Option<String>,
    /// Env: `OTEL_EXPORTER_OTLP_ENDPOINT`. The OTLP collector base URL, before `/v1/traces` is appended.
    /// The internal trace pipeline uses it only while `external_otel_master_switch` is off, as a deprecated fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_exporter_otlp_endpoint: Option<String>,
    /// Env: `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`. The full traces endpoint, used verbatim and preferred over `otel_exporter_otlp_endpoint`.
    /// The internal trace pipeline treats it like `otel_exporter_otlp_endpoint`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_exporter_otlp_traces_endpoint: Option<String>,
    /// Env: `OTEL_EXPORTER_OTLP_HEADERS`. Extra export headers in the form `k=v,k2=v2`.
    /// The internal trace pipeline treats it like `otel_exporter_otlp_endpoint`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_exporter_otlp_headers: Option<String>,
    /// Env: `GROK_INTERNAL_OTLP_TRACES_ENDPOINT`. The full internal traces endpoint, used verbatim and preferred over the legacy `OTEL_*` vars.
    /// Developers set it to send internal spans elsewhere, for example to a local collector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grok_internal_otlp_traces_endpoint: Option<String>,
    /// Env: `GROK_INTERNAL_OTLP_HEADERS`. Extra `k=v,k2=v2` debug headers for the internal export.
    /// They are preferred over the legacy `OTEL_EXPORTER_OTLP_HEADERS`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grok_internal_otlp_headers: Option<String>,
    /// Whether the external OTEL stream is turned on, set at construction by [`external_otel_master_switch_resolved`].
    /// When true, the internal trace pipeline ignores the standard `OTEL_EXPORTER_OTLP_*` vars.
    #[serde(skip)]
    pub external_otel_master_switch: bool,
    /// Env: `OTEL_TRACES_EXPORTER`. `otlp` (default) or `none` to disable spans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_traces_exporter: Option<String>,
    /// Env: `OTEL_BSP_SCHEDULE_DELAY` (OTel) or `OTEL_TRACES_EXPORT_INTERVAL` (Claude alias).
    /// Batch flush interval (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_traces_export_interval: Option<u64>,
    /// Env: `OTEL_EXPORTER_OTLP_TIMEOUT`. Export HTTP timeout (ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_exporter_otlp_timeout: Option<u64>,
    /// `load_management_api_key_sync()` reads this key.
    /// Declaring the field stops `serde_ignored` from reporting the key as unrecognized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_api_key: Option<String>,
}

/// `[endpoints]` keys of the removed direct S3 and direct GCS upload modes.
/// A config that names one fails to load. Trace uploads go through the proxy only.
pub const REMOVED_DIRECT_UPLOAD_KEYS: &[&str] = &[
    "trace_upload_bucket",
    "trace_upload_region",
    "trace_upload_credentials_file",
    "trace_upload_credentials",
    "trace_upload_endpoint_url",
    "gcs_service_account_key",
];

/// Env vars of the removed direct S3 and direct GCS upload modes.
pub const REMOVED_DIRECT_UPLOAD_ENV_VARS: &[&str] = &[
    "GROK_TRACE_UPLOAD_BUCKET",
    "GROK_TRACE_UPLOAD_REGION",
    "GROK_TRACE_UPLOAD_CREDENTIALS_FILE",
    "GROK_TRACE_UPLOAD_ENDPOINT_URL",
    "GROK_TELEMETRY_GCS_BUCKET",
];

/// The message for a setting of a removed upload mode.
pub fn removed_direct_upload_message(setting: &str) -> String {
    format!(
        "`{setting}` selects a direct S3 or direct GCS trace upload, and those upload modes \
         were removed. Trace uploads go through the proxy only. Remove `{setting}`."
    )
}

/// The first removed direct-upload env var that is set, as an error message.
pub fn removed_direct_upload_env_error() -> Option<String> {
    REMOVED_DIRECT_UPLOAD_ENV_VARS
        .iter()
        .find(|name| std::env::var_os(name).is_some())
        .map(|name| removed_direct_upload_message(name))
}

/// Why an `[endpoints]` table did not load.
#[derive(Debug, thiserror::Error)]
pub enum EndpointsConfigError {
    #[error(transparent)]
    Alias(#[from] xai_tool_types::AliasConflict),
    #[error("{}", removed_direct_upload_message(&format!("endpoints.{}", .0)))]
    RemovedUploadMode(&'static str),
}
impl EndpointsConfig {
    /// The keys [`models_list_url`](Self::models_list_url) is read under.
    pub const MODELS_LIST_URL_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("models_list_url", &["models_endpoint"]);
}

/// `EndpointsConfig` with each list-URL key spelling as its own field.
/// A table that names both keys folds under [`EndpointsConfig::MODELS_LIST_URL_KEYS`] and does not fail as a duplicate field.
/// An absent key is unset, never an error.
/// `external_otel_master_switch` is not here: construction fills it.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EndpointsConfigWire {
    cli_chat_proxy_base_url: Option<String>,
    xai_api_base_url: String,
    allowed_endpoints: Vec<String>,
    alpha_test_key: Option<String>,
    models_base_url: Option<String>,
    models_list_url: Option<String>,
    models_endpoint: Option<String>,
    feedback_base_url: Option<String>,
    trace_upload_url: Option<String>,
    // Removed direct-upload keys: read only to refuse them.
    trace_upload_bucket: Option<serde::de::IgnoredAny>,
    trace_upload_region: Option<serde::de::IgnoredAny>,
    trace_upload_credentials_file: Option<serde::de::IgnoredAny>,
    trace_upload_credentials: Option<serde::de::IgnoredAny>,
    trace_upload_endpoint_url: Option<serde::de::IgnoredAny>,
    gcs_service_account_key: Option<serde::de::IgnoredAny>,
    managed_config_url: Option<String>,
    otel_exporter_otlp_endpoint: Option<String>,
    otel_exporter_otlp_traces_endpoint: Option<String>,
    otel_exporter_otlp_headers: Option<String>,
    grok_internal_otlp_traces_endpoint: Option<String>,
    grok_internal_otlp_headers: Option<String>,
    otel_traces_exporter: Option<String>,
    otel_traces_export_interval: Option<u64>,
    otel_exporter_otlp_timeout: Option<u64>,
    management_api_key: Option<String>,
}

impl TryFrom<EndpointsConfigWire> for EndpointsConfig {
    type Error = EndpointsConfigError;

    fn try_from(wire: EndpointsConfigWire) -> Result<Self, Self::Error> {
        let removed = [
            wire.trace_upload_bucket.is_some(),
            wire.trace_upload_region.is_some(),
            wire.trace_upload_credentials_file.is_some(),
            wire.trace_upload_credentials.is_some(),
            wire.trace_upload_endpoint_url.is_some(),
            wire.gcs_service_account_key.is_some(),
        ];
        if let Some((key, _)) = REMOVED_DIRECT_UPLOAD_KEYS
            .iter()
            .zip(removed)
            .find(|(_, set)| *set)
        {
            return Err(EndpointsConfigError::RemovedUploadMode(key));
        }
        Ok(Self {
            cli_chat_proxy_base_url: wire.cli_chat_proxy_base_url,
            xai_api_base_url: wire.xai_api_base_url,
            allowed_endpoints: wire.allowed_endpoints,
            alpha_test_key: wire.alpha_test_key,
            models_base_url: wire.models_base_url,
            models_list_url: EndpointsConfig::MODELS_LIST_URL_KEYS
                .fold(vec![wire.models_list_url, wire.models_endpoint])?,
            feedback_base_url: wire.feedback_base_url,
            trace_upload_url: wire.trace_upload_url,
            managed_config_url: wire.managed_config_url,
            otel_exporter_otlp_endpoint: wire.otel_exporter_otlp_endpoint,
            otel_exporter_otlp_traces_endpoint: wire.otel_exporter_otlp_traces_endpoint,
            otel_exporter_otlp_headers: wire.otel_exporter_otlp_headers,
            grok_internal_otlp_traces_endpoint: wire.grok_internal_otlp_traces_endpoint,
            grok_internal_otlp_headers: wire.grok_internal_otlp_headers,
            external_otel_master_switch: false,
            otel_traces_exporter: wire.otel_traces_exporter,
            otel_traces_export_interval: wire.otel_traces_export_interval,
            otel_exporter_otlp_timeout: wire.otel_exporter_otlp_timeout,
            management_api_key: wire.management_api_key,
        })
    }
}

/// Forwards through [`EndpointsConfigWire`] and the fold.
impl<'de> Deserialize<'de> for EndpointsConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <EndpointsConfigWire as serde::Deserialize<'de>>::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

/// `{base}/models`, or `base` itself when it already ends in `/models`.
fn models_list_url_for_base(base: &str) -> String {
    if base.ends_with("/models") {
        base.to_owned()
    } else {
        format!("{base}/models")
    }
}
/// A blank or whitespace-only override counts as unset.
fn blank_as_unset(opt: &Option<String>) -> Option<String> {
    opt.as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
impl EndpointsConfig {
    pub fn has_custom_endpoint(&self) -> bool {
        self.models_base_url.is_some() || self.models_list_url.is_some()
    }
    /// `default()` with the managed and requirements `[endpoints]` overrides merged on top.
    /// Startup fetches use it to reach the configured endpoints.
    pub fn from_effective_config() -> Self {
        match crate::effective_config::load_effective_config() {
            Ok(cfg) => Self::from_config_value(&cfg),
            Err(_) => Self::default(),
        }
    }
    /// Merges the `[endpoints]` table from `config` over `default()`.
    /// Only the resolver methods apply defaults.
    pub fn from_config_value(config: &toml::Value) -> Self {
        let default = Self::default();
        let external_otel_master_switch = default.external_otel_master_switch;
        let mut base = match toml::Value::try_from(default) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        if let Some(endpoints) = config.get("endpoints") {
            crate::deep_merge_toml(&mut base, endpoints);
        }
        let mut resolved: Self = base.try_into().unwrap_or_default();
        resolved.external_otel_master_switch = external_otel_master_switch;
        xai_grok_extra_ca::endpoint_allowlist::set_configured(resolved.allowed_endpoints.clone());
        resolved
    }
    /// The cli-chat-proxy base URL for the auxiliary services and for inference with OAuth or session auth.
    ///
    pub fn proxy_url(&self) -> String {
        blank_as_unset(&self.cli_chat_proxy_base_url).unwrap_or_default()
    }
    /// `proxy_url` plus `suffix`, or blank when the proxy is unset.
    /// A bare suffix such as `/traces` is not a URL anyone can reach.
    fn proxy_join(&self, suffix: &str) -> String {
        let proxy = self.proxy_url();
        if proxy.is_empty() {
            return proxy;
        }
        format!("{}{suffix}", proxy.trim_end_matches('/'))
    }
    pub fn resolve_inference_base_url(&self) -> String {
        self.models_base_url
            .clone()
            .unwrap_or_else(|| self.proxy_url())
    }
    pub fn resolve_feedback_base_url(&self) -> String {
        blank_as_unset(&self.feedback_base_url).unwrap_or_else(|| self.proxy_url())
    }
    pub fn resolve_trace_upload_url(&self) -> String {
        blank_as_unset(&self.trace_upload_url).unwrap_or_else(|| self.proxy_url())
    }
    /// The managed deployment config URL for `grok setup`.
    /// The deployment key sent here must reach the proxy, never the inference host.
    pub fn resolve_managed_config_url(&self) -> String {
        blank_as_unset(&self.managed_config_url)
            .unwrap_or_else(|| self.proxy_join("/deployment/config"))
    }
    /// The traces endpoint for the internal pipeline, whose exports carry xAI auth.
    /// The default stays on `proxy_url` even when inference goes to another host.
    pub fn resolve_otlp_traces_endpoint(&self) -> String {
        if let Some(full) = blank_as_unset(&self.grok_internal_otlp_traces_endpoint) {
            return full.trim_end_matches('/').to_string();
        }
        if !self.external_otel_master_switch
            && let Some(legacy) = self.legacy_internal_otlp_traces_endpoint()
        {
            tracing::warn!(
                "Repointing the internal trace pipeline via OTEL_EXPORTER_OTLP_ENDPOINT / \
                 OTEL_EXPORTER_OTLP_TRACES_ENDPOINT is deprecated; use \
                 GROK_INTERNAL_OTLP_TRACES_ENDPOINT instead — the standard OTEL_* vars will \
                 route the external OTEL stream only in a future release"
            );
            return legacy;
        }
        self.proxy_join("/traces")
    }
    /// The internal traces endpoint from the standard `OTEL_*` vars, if any.
    /// Callers must check `external_otel_master_switch` themselves.
    fn legacy_internal_otlp_traces_endpoint(&self) -> Option<String> {
        if let Some(full) = blank_as_unset(&self.otel_exporter_otlp_traces_endpoint) {
            return Some(full.trim_end_matches('/').to_string());
        }
        blank_as_unset(&self.otel_exporter_otlp_endpoint)
            .map(|base| format!("{}/v1/traces", base.trim_end_matches('/')))
    }
    /// Extra headers for the internal export.
    /// The fallback to `otel_exporter_otlp_headers` is kept for existing users.
    pub fn resolve_otlp_headers(&self) -> Vec<(String, String)> {
        if let Some(headers) = blank_as_unset(&self.grok_internal_otlp_headers) {
            return parse_otlp_header_list(&headers);
        }
        if !self.external_otel_master_switch {
            return parse_otlp_header_list(
                self.otel_exporter_otlp_headers.as_deref().unwrap_or(""),
            );
        }
        Vec::new()
    }
    /// Whether the internal pipeline took its endpoint or headers from the standard `OTEL_EXPORTER_OTLP_*` vars.
    /// The external OTEL stream must refuse to start when this is true.
    pub fn internal_otlp_consumed_standard_vars(&self) -> bool {
        if self.external_otel_master_switch {
            return false;
        }
        let endpoint_consumed = blank_as_unset(&self.grok_internal_otlp_traces_endpoint).is_none()
            && self.legacy_internal_otlp_traces_endpoint().is_some();
        let headers_consumed = blank_as_unset(&self.grok_internal_otlp_headers).is_none()
            && blank_as_unset(&self.otel_exporter_otlp_headers).is_some();
        endpoint_consumed || headers_consumed
    }
    /// The internal pipeline honors `OTEL_TRACES_EXPORTER=none` even when `external_otel_master_switch` is on.
    /// Turning off internal span export is always safe.
    pub fn resolve_traces_export_enabled(&self) -> bool {
        !matches!(
            self.otel_traces_exporter.as_deref().map(str::trim),
            Some("none")
        )
    }
    /// The internal and external pipelines share this tuning value on purpose.
    pub fn resolve_otlp_export_interval(&self) -> Option<std::time::Duration> {
        self.otel_traces_export_interval
            .map(std::time::Duration::from_millis)
    }
    /// The internal and external pipelines share this tuning value on purpose.
    pub fn resolve_otlp_timeout(&self) -> Option<std::time::Duration> {
        self.otel_exporter_otlp_timeout
            .map(std::time::Duration::from_millis)
    }
    /// `models_list_url`, else `{models_base_url}/models`, else `{proxy_url}/models`, else blank.
    pub fn resolve_models_list_url(&self) -> String {
        if let Some(ref url) = self.models_list_url {
            return url.clone();
        }
        let base = self
            .models_base_url
            .clone()
            .unwrap_or_else(|| self.proxy_url());
        if base.trim().is_empty() {
            return String::new();
        }
        models_list_url_for_base(&base)
    }
}
/// Parses a `k=v,k2=v2` header list, the format of `OTEL_EXPORTER_OTLP_HEADERS` and `GROK_INTERNAL_OTLP_HEADERS`.
fn parse_otlp_header_list(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}
impl Default for EndpointsConfig {
    fn default() -> Self {
        Self {
            cli_chat_proxy_base_url: std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL").ok(),
            xai_api_base_url: std::env::var("GROK_XAI_API_BASE_URL").unwrap_or_default(),
            allowed_endpoints: Vec::new(),
            alpha_test_key: None,
            models_base_url: env_string("GROK_MODELS_BASE_URL"),
            models_list_url: env_string("GROK_MODELS_LIST_URL"),
            feedback_base_url: env_string("GROK_FEEDBACK_BASE_URL"),
            trace_upload_url: env_string("GROK_TRACE_UPLOAD_URL"),
            managed_config_url: env_string("GROK_MANAGED_CONFIG_URL"),
            otel_exporter_otlp_endpoint: env_string("OTEL_EXPORTER_OTLP_ENDPOINT"),
            otel_exporter_otlp_traces_endpoint: env_string("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
            otel_exporter_otlp_headers: env_string("OTEL_EXPORTER_OTLP_HEADERS"),
            grok_internal_otlp_traces_endpoint: env_string("GROK_INTERNAL_OTLP_TRACES_ENDPOINT"),
            grok_internal_otlp_headers: env_string("GROK_INTERNAL_OTLP_HEADERS"),
            external_otel_master_switch: external_otel_master_switch_resolved(),
            otel_traces_exporter: env_string("OTEL_TRACES_EXPORTER"),
            otel_traces_export_interval: env_string("OTEL_BSP_SCHEDULE_DELAY")
                .or_else(|| env_string("OTEL_TRACES_EXPORT_INTERVAL"))
                .and_then(|s| s.parse().ok()),
            otel_exporter_otlp_timeout: env_string("OTEL_EXPORTER_OTLP_TIMEOUT")
                .and_then(|s| s.parse().ok()),
            management_api_key: None,
        }
    }
}
/// Computes the external OTEL switch with the same precedence the external stream uses to turn on.
/// A mismatch would let the internal pipeline read `OTEL_*` vars meant for the external stream.
fn external_otel_master_switch_resolved() -> bool {
    external_otel_master_switch_from(
        crate::load_merged_requirements().as_ref(),
        env_bool("GROK_EXTERNAL_OTEL"),
        crate::effective_config::load_effective_config()
            .ok()
            .as_ref(),
    )
}
/// [`external_otel_master_switch_resolved`] with its inputs passed in.
fn external_otel_master_switch_from(
    requirements: Option<&toml::Value>,
    env_switch: Option<bool>,
    effective_config: Option<&toml::Value>,
) -> bool {
    let table_enabled = |v: Option<&toml::Value>| -> Option<bool> {
        v?.get("telemetry")?.get("otel_enabled")?.as_bool()
    };
    if let Some(pinned) = table_enabled(requirements) {
        return pinned;
    }
    if let Some(env) = env_switch {
        return env;
    }
    table_enabled(effective_config).unwrap_or(false)
}
#[cfg(test)]
#[path = "endpoints_tests.rs"]
mod tests;
