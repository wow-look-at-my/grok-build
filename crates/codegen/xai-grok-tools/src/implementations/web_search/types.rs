use indexmap::IndexMap;

/// Configuration for the web search tool. Use `Disabled` when no API key is available or web search should be turned off. The `Enabled` variant
/// is inherently large (credentials, headers, domain filters) while `Disabled` is empty, but this config is built once per session and never
/// stored in bulk collections, so boxing would add indirection for no real benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WebSearchConfig {
    #[default]
    Disabled,
    Enabled {
        api_key: String,
        base_url: String,
        model: String,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        extra_headers: IndexMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alpha_test_key: Option<String>,
        /// Authoritative domain allowlist from `[toolset.web_search] allowed_domains`. When set, it governs the client-side
        /// web_search tool and the model's own per-call `allowed_domains` is ignored (see `resolve_filters`), so a configured
        /// policy cannot be bypassed. Mutually exclusive with `excluded_domains`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allowed_domains: Option<Vec<String>>,
        /// Authoritative domain blocklist from `[toolset.web_search] excluded_domains`. Like `allowed_domains` it governs
        /// outright: the model cannot un-block a domain by naming it in its own per-call `allowed_domains`, which is what makes
        /// this a real block. Mutually exclusive with `allowed_domains`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        excluded_domains: Option<Vec<String>>,
    },
    /// Kagi's own search index instead of an LLM-synthesized answer.
    ///
    /// Kagi returns ranked, already-filtered results with snippets, so no
    /// model call is made and no synthesis model has to be configured. Auth is
    /// a Search API token (`Authorization: Bot <token>`), a different scheme and
    /// credential from the Responses-API bearer the other arms use.
    Kagi {
        api_key: String,
        #[serde(default = "default_kagi_base_url")]
        base_url: String,
        /// Results to request per query. Kagi's own default applies when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        extra_headers: IndexMap<String, String>,
    },
}

/// Kagi's Search API root. Matches the documented example host; overridable so
/// the path can be pointed at a mock server in tests.
pub fn default_kagi_base_url() -> String {
    "https://kagi.com/api/v1".to_string()
}

impl WebSearchConfig {
    /// Returns `true` when the config is an enabled arm.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. } | Self::Kagi { .. })
    }

    /// Return a copy safe for returning to clients. The `api_key` is replaced with
    /// `"***REDACTED***"` and the optional extra access key field is stripped.
    pub fn redacted(&self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Enabled {
                base_url,
                model,
                extra_headers,
                allowed_domains,
                excluded_domains,
                ..
            } => Self::Enabled {
                api_key: "***REDACTED***".to_string(),
                base_url: base_url.clone(),
                model: model.clone(),
                extra_headers: extra_headers.clone(),
                alpha_test_key: None,
                allowed_domains: allowed_domains.clone(),
                excluded_domains: excluded_domains.clone(),
            },
            Self::Kagi {
                base_url,
                limit,
                extra_headers,
                ..
            } => Self::Kagi {
                api_key: "***REDACTED***".to_string(),
                base_url: base_url.clone(),
                limit: *limit,
                extra_headers: extra_headers.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_is_disabled() {
        let config = WebSearchConfig::default();
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_config_redacted() {
        let mut headers = IndexMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());
        let config = WebSearchConfig::Enabled {
            api_key: "secret-key-12345".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: headers,
            alpha_test_key: Some("alpha-secret".to_string()),
            allowed_domains: Some(vec!["docs.x.ai".to_string()]),
            excluded_domains: None,
        };
        let redacted = config.redacted();
        match redacted {
            WebSearchConfig::Enabled {
                api_key,
                base_url,
                model,
                extra_headers,
                alpha_test_key,
                allowed_domains,
                excluded_domains,
            } => {
                assert_eq!(api_key, "***REDACTED***");
                assert_eq!(base_url, "https://api.x.ai/v1");
                assert_eq!(model, "test-web-search-model");
                assert_eq!(extra_headers.get("X-Custom").unwrap(), "value");
                assert!(alpha_test_key.is_none());
                // Domain filters survive redaction (not secrets).
                assert_eq!(allowed_domains, Some(vec!["docs.x.ai".to_string()]));
                assert!(excluded_domains.is_none());
            }
            _ => panic!("Expected Enabled variant"),
        }
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = WebSearchConfig::Enabled {
            api_key: "key".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
            allowed_domains: None,
            excluded_domains: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WebSearchConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_enabled());
    }

    #[test]
    fn test_config_deserialize_from_set_options_payload() {
        let json = r#"{
            "status": "enabled",
            "api_key": "xai-abc123",
            "base_url": "https://api.x.ai/v1",
            "model": "test-web-search-model"
        }"#;
        let config: WebSearchConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_enabled());
    }
}
