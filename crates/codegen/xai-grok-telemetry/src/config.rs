//! These types live here so the data-collector engine can construct a [`TelemetryClient`](crate::client::TelemetryClient) without depending on shell.
//!
//! Shell still re-exports these types from their original paths so existing call sites (and `Config` derive impls) compile unchanged.
use serde::{Deserialize, Serialize};
use xai_grok_env::env_bool;
/// Telemetry mode: `true`/`false` (legacy bool) or `"session_metrics"` (string). `Disabled`: nothing sent (enterprise
/// default); `SessionMetrics`: metadata-only lifecycle events, no content; `Enabled`: full product telemetry (events and
/// Mixpanel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelemetryMode {
    #[default]
    Disabled,
    SessionMetrics,
    Enabled,
}
impl TelemetryMode {
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
    /// True for both `SessionMetrics` and `Enabled`.
    pub fn session_metrics_enabled(&self) -> bool {
        matches!(self, Self::SessionMetrics | Self::Enabled)
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enabled" | "full" => Some(Self::Enabled),
            "0" | "false" | "no" | "off" | "disabled" => Some(Self::Disabled),
            "session-metrics" | "session_metrics" => Some(Self::SessionMetrics),
            _ => None,
        }
    }
}
#[cfg(test)]
mod telemetry_mode_tests {
    use super::TelemetryMode;
    /// A parent process hands its resolved mode to spawned children via `GROK_TELEMETRY_ENABLED={mode}` (Display).
    /// Every Display output must parse back to the same mode.
    #[test]
    fn display_round_trips_through_parse() {
        for mode in [
            TelemetryMode::Enabled,
            TelemetryMode::Disabled,
            TelemetryMode::SessionMetrics,
        ] {
            assert_eq!(
                TelemetryMode::parse(&mode.to_string()),
                Some(mode),
                "Display value for {mode:?} must parse back to itself"
            );
        }
    }
}
impl std::fmt::Display for TelemetryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "false"),
            Self::SessionMetrics => write!(f, "session_metrics"),
            Self::Enabled => write!(f, "true"),
        }
    }
}
impl From<bool> for TelemetryMode {
    fn from(b: bool) -> Self {
        if b { Self::Enabled } else { Self::Disabled }
    }
}
impl serde::Serialize for TelemetryMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Enabled => serializer.serialize_bool(true),
            Self::SessionMetrics => serializer.serialize_str("session_metrics"),
        }
    }
}
/// Wire format for `[features] telemetry`: accepts `true`, `false`, or `"session_metrics"`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TelemetryModeValue {
    Bool(bool),
    Str(String),
}
impl<'de> serde::Deserialize<'de> for TelemetryMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match TelemetryModeValue::deserialize(deserializer)? {
            TelemetryModeValue::Bool(b) => Ok(Self::from(b)),
            TelemetryModeValue::Str(s) => Ok(Self::parse(&s).unwrap_or_else(|| {
                tracing::warn!(
                    value = %s,
                    "TELEMETRY_MODE_UNKNOWN: unrecognized telemetry mode; treating as disabled",
                );
                Self::Disabled
            })),
        }
    }
}
/// Parse an env var as a `TelemetryMode`. Returns `None` if unset or empty.
pub fn env_telemetry_mode(name: &str) -> Option<TelemetryMode> {
    let value = std::env::var(name).ok()?;
    TelemetryMode::parse(&value)
}
/// Parse `[telemetry] otel_timeout` / `otel_metric_export_interval`: docs say
/// `number`, so TOML integers must not fail-close config load. Strings still
/// work (`"10000"`). Stored as decimal strings to match the env-var overlay.
fn deserialize_opt_ms_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrString {
        Int(i64),
        Str(String),
    }
    Ok(match Option::<IntOrString>::deserialize(deserializer)? {
        None => None,
        Some(IntOrString::Int(i)) => Some(i.to_string()),
        Some(IntOrString::Str(s)) => Some(s),
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, try_from = "TelemetryConfigWire")]
pub struct TelemetryConfig {
    /// Declared for `serde_ignored`. Actual toggle is `[features] telemetry`.
    #[serde(default)]
    pub enabled: Option<bool>,
    pub events_url: Option<String>,
    pub events_api_key: Option<String>,
    pub mixpanel_token: Option<String>,
    pub mixpanel_enabled: bool,
    /// `None` inherits from `[features] telemetry`; `Some(false)` disables GCS uploads only.
    pub trace_upload: Option<bool>,
    pub otel_enabled: Option<bool>,
    /// External OTEL metrics exporter: `otlp` | `console` | `none`.
    pub otel_metrics_exporter: Option<String>,
    /// External OTEL logs/events exporter: `otlp` | `console` | `none`.
    pub otel_logs_exporter: Option<String>,
    /// External OTLP base endpoint (`/v1/logs`, `/v1/metrics` appended for HTTP).
    pub otel_endpoint: Option<String>,
    /// External OTLP transport: `http/protobuf` | `grpc`.
    pub otel_protocol: Option<String>,
    pub otel_certificate: Option<String>,
    pub otel_client_certificate: Option<String>,
    pub otel_client_key: Option<String>,
    /// External OTEL content gate (admins can pin to `false` via requirements).
    pub otel_log_user_prompts: Option<bool>,
    /// External OTEL content gate (admins can pin to `false` via requirements).
    pub otel_log_tool_details: Option<bool>,
    /// External OTEL content gate. Unset follows `otel_log_user_prompts`.
    pub otel_log_assistant_responses: Option<bool>,
    /// External OTEL content gate for full tool/MCP bodies (default off).
    pub otel_log_tool_content: Option<bool>,
    /// Milliseconds as a decimal string. TOML accepts integer or string.
    #[serde(default, deserialize_with = "deserialize_opt_ms_string")]
    pub otel_timeout: Option<String>,
    /// Milliseconds as a decimal string. TOML accepts integer or string.
    #[serde(default, deserialize_with = "deserialize_opt_ms_string")]
    pub otel_metric_export_interval: Option<String>,
    pub otel_logs_endpoint: Option<String>,
    pub otel_metrics_endpoint: Option<String>,
    pub otel_logs_protocol: Option<String>,
    pub otel_metrics_protocol: Option<String>,
    pub otel_logs_certificate: Option<String>,
    pub otel_metrics_certificate: Option<String>,
    pub otel_logs_client_certificate: Option<String>,
    pub otel_logs_client_key: Option<String>,
    pub otel_metrics_client_certificate: Option<String>,
    pub otel_metrics_client_key: Option<String>,
    pub otel_metrics_include_session_id: Option<bool>,
}

impl TelemetryConfig {
    /// The keys [`otel_protocol`](Self::otel_protocol) is read under.
    pub const OTEL_PROTOCOL_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("otel_protocol", &["otel_transport"]);
}

/// `TelemetryConfig` with each transport-key spelling as its own field, so a
/// table naming both folds under [`TelemetryConfig::OTEL_PROTOCOL_KEYS`]
/// instead of tripping serde's duplicate-field check. This table can arrive
/// from a remote campaign patch. This is merged into the same value the config
/// is read from and is not limited to any field set.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TelemetryConfigWire {
    enabled: Option<bool>,
    events_url: Option<String>,
    events_api_key: Option<String>,
    mixpanel_token: Option<String>,
    mixpanel_enabled: bool,
    trace_upload: Option<bool>,
    otel_enabled: Option<bool>,
    otel_metrics_exporter: Option<String>,
    otel_logs_exporter: Option<String>,
    otel_endpoint: Option<String>,
    otel_protocol: Option<String>,
    otel_transport: Option<String>,
    otel_certificate: Option<String>,
    otel_client_certificate: Option<String>,
    otel_client_key: Option<String>,
    otel_log_user_prompts: Option<bool>,
    otel_log_tool_details: Option<bool>,
    otel_log_assistant_responses: Option<bool>,
    otel_log_tool_content: Option<bool>,
    #[serde(deserialize_with = "deserialize_opt_ms_string")]
    otel_timeout: Option<String>,
    #[serde(deserialize_with = "deserialize_opt_ms_string")]
    otel_metric_export_interval: Option<String>,
    otel_logs_endpoint: Option<String>,
    otel_metrics_endpoint: Option<String>,
    otel_logs_protocol: Option<String>,
    otel_metrics_protocol: Option<String>,
    otel_logs_certificate: Option<String>,
    otel_metrics_certificate: Option<String>,
    otel_logs_client_certificate: Option<String>,
    otel_logs_client_key: Option<String>,
    otel_metrics_client_certificate: Option<String>,
    otel_metrics_client_key: Option<String>,
    otel_metrics_include_session_id: Option<bool>,
}

impl TryFrom<TelemetryConfigWire> for TelemetryConfig {
    type Error = xai_tool_types::AliasConflict;

    fn try_from(wire: TelemetryConfigWire) -> Result<Self, Self::Error> {
        Ok(Self {
            enabled: wire.enabled,
            events_url: wire.events_url,
            events_api_key: wire.events_api_key,
            mixpanel_token: wire.mixpanel_token,
            mixpanel_enabled: wire.mixpanel_enabled,
            trace_upload: wire.trace_upload,
            otel_enabled: wire.otel_enabled,
            otel_metrics_exporter: wire.otel_metrics_exporter,
            otel_logs_exporter: wire.otel_logs_exporter,
            otel_endpoint: wire.otel_endpoint,
            otel_protocol: TelemetryConfig::OTEL_PROTOCOL_KEYS
                .fold(vec![wire.otel_protocol, wire.otel_transport])?,
            otel_certificate: wire.otel_certificate,
            otel_client_certificate: wire.otel_client_certificate,
            otel_client_key: wire.otel_client_key,
            otel_log_user_prompts: wire.otel_log_user_prompts,
            otel_log_tool_details: wire.otel_log_tool_details,
            otel_log_assistant_responses: wire.otel_log_assistant_responses,
            otel_log_tool_content: wire.otel_log_tool_content,
            otel_timeout: wire.otel_timeout,
            otel_metric_export_interval: wire.otel_metric_export_interval,
            otel_logs_endpoint: wire.otel_logs_endpoint,
            otel_metrics_endpoint: wire.otel_metrics_endpoint,
            otel_logs_protocol: wire.otel_logs_protocol,
            otel_metrics_protocol: wire.otel_metrics_protocol,
            otel_logs_certificate: wire.otel_logs_certificate,
            otel_metrics_certificate: wire.otel_metrics_certificate,
            otel_logs_client_certificate: wire.otel_logs_client_certificate,
            otel_logs_client_key: wire.otel_logs_client_key,
            otel_metrics_client_certificate: wire.otel_metrics_client_certificate,
            otel_metrics_client_key: wire.otel_metrics_client_key,
            otel_metrics_include_session_id: wire.otel_metrics_include_session_id,
        })
    }
}
fn internal_defaults() -> (Option<String>, Option<String>, Option<String>, bool) {
    (None, None, None, false)
}
fn build_env_default(value: Option<&'static str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
impl Default for TelemetryConfig {
    fn default() -> Self {
        let (baked_url, baked_key, baked_token, baked_enabled) = internal_defaults();
        let build_url = build_env_default(option_env!("GROK_TELEMETRY_BUILD_EVENTS_URL"));
        let build_key = build_env_default(option_env!("GROK_TELEMETRY_BUILD_EVENTS_API_KEY"));
        let build_token = build_env_default(option_env!("GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN"));
        let mixpanel_enabled = baked_enabled || build_token.is_some();
        let (events_url, events_api_key, mixpanel_token) = (
            build_url.or(baked_url),
            build_key.or(baked_key),
            build_token.or(baked_token),
        );
        Self {
            enabled: None,
            events_url,
            events_api_key,
            mixpanel_token,
            mixpanel_enabled,
            trace_upload: None,
            otel_enabled: None,
            otel_metrics_exporter: None,
            otel_logs_exporter: None,
            otel_endpoint: None,
            otel_protocol: None,
            otel_certificate: None,
            otel_client_certificate: None,
            otel_client_key: None,
            otel_log_user_prompts: None,
            otel_log_tool_details: None,
            otel_log_assistant_responses: None,
            otel_log_tool_content: None,
            otel_timeout: None,
            otel_metric_export_interval: None,
            otel_logs_endpoint: None,
            otel_metrics_endpoint: None,
            otel_logs_protocol: None,
            otel_metrics_protocol: None,
            otel_logs_certificate: None,
            otel_metrics_certificate: None,
            otel_logs_client_certificate: None,
            otel_logs_client_key: None,
            otel_metrics_client_certificate: None,
            otel_metrics_client_key: None,
            otel_metrics_include_session_id: None,
        }
    }
}
impl TelemetryConfig {
    /// Clears every sink still carrying its baked `internal-telemetry-defaults` value; the events
    /// key follows the URL, so an explicit URL keeps a baked key. Returns whether anything was cleared.
    pub(crate) fn disarm_baked_sinks(&mut self) -> bool {
        let (baked_url, _, baked_token, _) = internal_defaults();
        let events_cleared = self
            .events_url
            .take_if(|url| baked_url.as_deref() == Some(url.as_str()))
            .is_some();
        if events_cleared {
            self.events_api_key = None;
        }
        let token_cleared = self
            .mixpanel_token
            .take_if(|token| baked_token.as_deref() == Some(token.as_str()))
            .is_some();
        if token_cleared {
            self.mixpanel_enabled = false;
        }
        events_cleared || token_cleared
    }
    pub fn apply_env_overrides(&mut self) {
        self.normalize();
        if let Some(value) = Self::env_override("GROK_TELEMETRY_EVENTS_URL") {
            self.events_url = value;
        }
        if let Some(value) = Self::env_override("GROK_TELEMETRY_EVENTS_API_KEY") {
            self.events_api_key = value;
        }
        if let Some(value) = Self::env_override("GROK_TELEMETRY_MIXPANEL_TOKEN") {
            self.mixpanel_token = value;
        }
        if let Some(value) = env_bool("GROK_TELEMETRY_MIXPANEL_ENABLED") {
            self.mixpanel_enabled = value;
        }
        if let Some(value) = env_bool("GROK_TELEMETRY_TRACE_UPLOAD") {
            self.trace_upload = Some(value);
        }
    }
    fn normalize(&mut self) {
        self.events_url = Self::normalize_optional_string(self.events_url.take());
        self.events_api_key = Self::normalize_optional_string(self.events_api_key.take());
        self.mixpanel_token = Self::normalize_optional_string(self.mixpanel_token.take());
    }
    fn env_override(name: &str) -> Option<Option<String>> {
        match std::env::var(name) {
            Ok(value) => Some(Self::normalize_optional_string(Some(value))),
            Err(_) => None,
        }
    }
    fn normalize_optional_string(value: Option<String>) -> Option<String> {
        value.and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }
}
/// Derive a stable ID (UUIDv5) from a secret key, so the key itself never leaves.
pub fn key_id_from_key(key: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes()).to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_env_default_normalizes() {
        assert_eq!(build_env_default(None), None);
        assert_eq!(build_env_default(Some("")), None);
        assert_eq!(build_env_default(Some(" \t ")), None);
        assert_eq!(build_env_default(Some(" key ")), Some("key".to_owned()));
    }
    /// The key follows the URL: a baked key next to an explicit URL stays, or the explicit sink would post nothing.
    #[test]
    fn disarm_baked_sinks_keeps_explicit_sinks() {
        let mut cfg = TelemetryConfig {
            events_url: Some("http://127.0.0.1:9/events".into()),
            mixpanel_token: Some("explicit-token".into()),
            mixpanel_enabled: true,
            ..TelemetryConfig::default()
        };
        let key_before = cfg.events_api_key.clone();
        assert!(
            !cfg.disarm_baked_sinks(),
            "explicit sinks must not count as cleared"
        );
        assert_eq!(
            (
                cfg.events_url.as_deref(),
                cfg.events_api_key == key_before,
                cfg.mixpanel_token.as_deref(),
                cfg.mixpanel_enabled
            ),
            (
                Some("http://127.0.0.1:9/events"),
                true,
                Some("explicit-token"),
                true
            )
        );
    }
    #[test]
    fn default_is_build_env_layer_when_feature_off() {
        let cfg = TelemetryConfig::default();
        let url = build_env_default(option_env!("GROK_TELEMETRY_BUILD_EVENTS_URL"));
        let key = build_env_default(option_env!("GROK_TELEMETRY_BUILD_EVENTS_API_KEY"));
        let token = build_env_default(option_env!("GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN"));
        assert_eq!(cfg.mixpanel_enabled, token.is_some());
        assert_eq!(cfg.events_url, url);
        assert_eq!(cfg.events_api_key, key);
        assert_eq!(cfg.mixpanel_token, token);
    }
    #[test]
    fn otel_timeout_fields_accept_int_or_string() {
        let from_int: TelemetryConfig =
            serde_json::from_str(r#"{"otel_timeout":10000,"otel_metric_export_interval":60000}"#)
                .unwrap();
        assert_eq!(from_int.otel_timeout.as_deref(), Some("10000"));
        assert_eq!(
            from_int.otel_metric_export_interval.as_deref(),
            Some("60000")
        );
        let from_str: TelemetryConfig = serde_json::from_str(
            r#"{"otel_timeout":"10000","otel_metric_export_interval":"60000"}"#,
        )
        .unwrap();
        assert_eq!(from_str.otel_timeout.as_deref(), Some("10000"));
        assert_eq!(
            from_str.otel_metric_export_interval.as_deref(),
            Some("60000")
        );
    }
}

#[cfg(test)]
mod wire_alias_tests {
    use super::TelemetryConfig;

    fn parse(table: &str) -> Result<TelemetryConfig, serde_json::Error> {
        serde_json::from_str(&format!("{{{table}}}"))
    }

    /// A `[telemetry]` table naming the transport under both spellings is one setting stated twice. This table can arrive from a remote campaign patch, which merges into the same value the whole `Config` is read from. A duplicate-field
    /// rejection here would fail the entire config parse.
    #[test]
    fn a_table_naming_the_transport_under_both_keys_under_one_value_parses_once() {
        let cfg = parse(r#""otel_protocol":"grpc","otel_transport":"grpc""#)
            .expect("one value named under two keys is one value");
        assert_eq!(cfg.otel_protocol.as_deref(), Some("grpc"));
    }

    #[test]
    fn a_table_reading_either_transport_spelling_alone_still_works() {
        let canonical = parse(r#""otel_protocol":"http/protobuf""#).unwrap();
        assert_eq!(canonical.otel_protocol.as_deref(), Some("http/protobuf"));

        let legacy = parse(r#""otel_transport":"grpc""#).unwrap();
        assert_eq!(legacy.otel_protocol.as_deref(), Some("grpc"));
    }

    /// Different transports is a genuine conflict about where OTLP goes, so it
    /// fails and names the field rather than exporting to one of them.
    #[test]
    fn a_table_whose_transport_spellings_disagree_is_an_error_naming_the_field() {
        let err = parse(r#""otel_protocol":"grpc","otel_transport":"http/protobuf""#)
            .expect_err("a contradicted transport must not resolve silently");
        let text = err.to_string();
        assert!(text.contains("otel_protocol"), "{err}");
        assert!(text.contains("otel_transport"), "{err}");
    }

    /// The outgoing shape keeps the canonical key, so a config written back to
    /// disk does not grow the legacy spelling.
    #[test]
    fn the_transport_serializes_under_the_canonical_key_only() {
        let mut cfg = TelemetryConfig::default();
        cfg.otel_protocol = Some("grpc".to_owned());
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["otel_protocol"], "grpc");
        assert!(
            json.get("otel_transport").is_none(),
            "the alias key must not appear on the wire: {json}"
        );
    }
}
