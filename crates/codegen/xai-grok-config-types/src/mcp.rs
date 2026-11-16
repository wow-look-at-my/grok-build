//! This module re-exports the MCP server config types from `xai-grok-config`.
//! It also holds `RelaySyncConfig`.

use serde::{Deserialize, Serialize};

pub use xai_grok_config::{
    KNOWN_MCP_SERVER_FIELDS, McpConfig, McpJsonOAuthBlock, McpPreferenceSource, McpPreferencesFile,
    McpServerConfig, McpServerConfigProblem, McpServerPreferences, McpServerProblemSeverity,
    McpServerTransportConfig, McpSetupConfig, McpSetupDerivedValue, McpSetupField,
    McpSetupFieldType, McpSetupOption, McpSetupResolution,
};

/// Configuration for relay session sharing, set in config.json under the `session.relay` object.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelaySyncConfig {
    pub enabled: Option<bool>,
}

impl RelaySyncConfig {
    /// Check if relay sync is enabled. `GROK_RELAY_SYNC_ENABLED` takes precedence over config.
    pub fn is_enabled(&self) -> bool {
        if let Ok(env_val) = std::env::var("GROK_RELAY_SYNC_ENABLED") {
            return env_val.eq_ignore_ascii_case("true") || env_val == "1";
        }
        self.enabled.unwrap_or(false)
    }
}
<<<<<<< fork
#[cfg(test)]
mod wire_alias_tests {
    use super::{McpServerTransportConfig, McpSetupConfig};

    /// `.mcp.json` written for VS Code names the address `urlTemplate`, Claude
    /// names it `url`, and a file maintained for both editors names both. With a
    /// bare `#[serde(alias)]` the second key is a `duplicate field` error, which
    /// drops the server from the config entirely.
    #[test]
    fn an_http_server_reads_its_url_under_any_one_spelling() {
        for json in [
            r#"{"command":"serve","args":[]}"#,
            r#"{"url":"https://mcp.example/v1"}"#,
            r#"{"urlTemplate":"https://mcp.example/v1"}"#,
            r#"{"url_template":"https://mcp.example/v1"}"#,
        ] {
            let transport: McpServerTransportConfig =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{json} must parse: {e}"));
            match transport {
                McpServerTransportConfig::Stdio { command, .. } => assert_eq!(command, "serve"),
                McpServerTransportConfig::StreamableHttp { url, .. } => {
                    assert_eq!(url, "https://mcp.example/v1", "{json}");
                }
            }
        }
    }

    #[test]
    fn an_http_server_naming_every_url_key_under_one_value_parses_once() {
        let transport: McpServerTransportConfig = serde_json::from_str(
            r#"{"url":"https://mcp.example/v1","urlTemplate":"https://mcp.example/v1","url_template":"https://mcp.example/v1"}"#,
        )
        .expect("one address named three ways is one address");
        assert!(
            matches!(&transport, McpServerTransportConfig::StreamableHttp { url, .. }
                if url == "https://mcp.example/v1"),
            "{transport:?}"
        );
    }

    /// Addresses in one entry decide where the server's traffic goes.
    #[test]
    fn an_http_server_whose_url_spellings_disagree_is_an_error_naming_the_field() {
        let err = serde_json::from_str::<McpServerTransportConfig>(
            r#"{"url":"https://a.example/v1","urlTemplate":"https://b.example/v1"}"#,
        )
        .expect_err("conflicting addresses must not resolve silently");
        let message = err.to_string();
        assert!(message.contains("url"), "{message}");
        assert!(message.contains("urlTemplate"), "{message}");
    }

    /// The address stayed required: an HTTP server with none is not a server,
    /// and a shadow that defaulted it would invent one.
    #[test]
    fn an_http_server_with_no_url_key_at_all_is_still_an_error() {
        let err = serde_json::from_str::<McpServerTransportConfig>(r#"{"headers":{}}"#)
            .expect_err("a url-less http entry must not deserialize");
        assert!(err.to_string().contains("url"), "{err}");
    }

    #[test]
    fn an_http_server_writes_the_canonical_url_and_never_an_alias() {
        let json = serde_json::to_value(&McpServerTransportConfig::StreamableHttp {
            url: "https://mcp.example/v1".into(),
            transport_type: None,
            bearer_token_env_var: None,
            headers: None,
            oauth_client_id: None,
            oauth_client_secret_env_var: None,
            oauth_scopes: None,
        })
        .unwrap();
        assert_eq!(json["url"], "https://mcp.example/v1");
        assert!(json.get("urlTemplate").is_none(), "{json}");
        assert!(json.get("url_template").is_none(), "{json}");
    }

    #[test]
    fn a_setup_block_reads_its_answers_under_either_key_and_folds_them() {
        let answers = r#"{"site":{"from":"input","map":{"us":"us1"}}}"#;
        let canonical: McpSetupConfig =
            serde_json::from_str(&format!(r#"{{"variables":{answers}}}"#)).unwrap();
        let alias: McpSetupConfig =
            serde_json::from_str(&format!(r#"{{"values":{answers}}}"#)).unwrap();
        assert_eq!(canonical, alias);
        assert_eq!(canonical.variables.len(), 1);

        let both: McpSetupConfig =
            serde_json::from_str(&format!(r#"{{"variables":{answers},"values":{answers}}}"#))
                .expect("one answer map named twice is one map");
        assert_eq!(both.variables.len(), 1);

        let err = serde_json::from_str::<McpSetupConfig>(
            r#"{"variables":{"site":{"from":"a","map":{}}},"values":{"site":{"from":"b","map":{}}}}"#,
        )
        .expect_err("two answer maps must not resolve silently");
        assert!(err.to_string().contains("variables"), "{err}");

        let json = serde_json::to_value(&both).unwrap();
        assert_eq!(json["variables"]["site"]["from"], "input");
        assert!(json.get("values").is_none(), "{json}");
    }
}
||||||| base

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpConfig {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: IndexMap<String, McpServerConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site_select_setup_json() -> &'static str {
        r#"{
            "mcpServers": {
                "acme": {
                    "type": "http",
                    "urlTemplate": "{{url}}",
                    "setup": {
                        "fields": [{
                            "id": "site",
                            "label": "Site",
                            "type": "select",
                            "required": true,
                            "default": "us1",
                            "options": [
                                {"label": "US1", "value": "us1"},
                                {"label": "US5", "value": "us5"}
                            ]
                        }],
                        "values": {
                            "url": {
                                "from": "site",
                                "map": {
                                    "us1": "https://mcp.example.com/v1/mcp",
                                    "us5": "https://mcp.us5.example.com/v1/mcp"
                                }
                            }
                        }
                    }
                }
            }
        }"#
    }

    #[test]
    fn transport_less_entry_fails_to_deserialize() {
        for value in [
            serde_json::json!({ "enabled": false }),
            serde_json::json!({ "enabled": true }),
            serde_json::json!({}),
        ] {
            assert!(
                serde_json::from_value::<McpServerConfig>(value.clone()).is_err(),
                "transport-less entry must not deserialize: {value}"
            );
        }
    }

    #[test]
    fn blank_transport_field_is_detected_symmetrically() {
        let blank_url: McpServerConfig =
            serde_json::from_value(serde_json::json!({ "url": "  " })).unwrap();
        assert_eq!(blank_url.blank_transport_field(), Some("url"));

        let blank_command: McpServerConfig =
            serde_json::from_value(serde_json::json!({ "command": "\t" })).unwrap();
        assert_eq!(blank_command.blank_transport_field(), Some("command"));

        let ok: McpServerConfig =
            serde_json::from_value(serde_json::json!({ "command": "npx" })).unwrap();
        assert_eq!(ok.blank_transport_field(), None);
    }

    /// A newly added field cannot silently escape `KNOWN_MCP_SERVER_FIELDS`.
    #[test]
    fn known_mcp_server_fields_cover_serialized_keys() {
        let stdio = McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "npx".into(),
                args: vec!["-y".into()],
                env: Some(HashMap::from([("A".into(), "b".into())])),
                cwd: Some("/tmp".into()),
            },
            enabled: true,
            oauth: Some(McpJsonOAuthBlock::default()),
            setup: None,
            startup_timeout_sec: Some(10),
            tool_timeout_sec: Some(20),
            tool_timeouts: Some(HashMap::from([("t".into(), 1)])),
            expose_image_base64: Some(true),
        };
        let http = McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "https://x/mcp".into(),
                transport_type: Some("http".into()),
                bearer_token_env_var: Some("TOK".into()),
                headers: Some(HashMap::from([("H".into(), "v".into())])),
                oauth_client_id: Some("id".into()),
                oauth_client_secret_env_var: Some("SEC".into()),
                oauth_scopes: Some(vec!["s".into()]),
            },
            enabled: true,
            oauth: None,
            setup: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            tool_timeouts: None,
            expose_image_base64: None,
        };
        for config in [stdio, http] {
            let value = serde_json::to_value(&config).unwrap();
            for key in value.as_object().unwrap().keys() {
                assert!(
                    KNOWN_MCP_SERVER_FIELDS.contains(&key.as_str()),
                    "field `{key}` is serialized but missing from KNOWN_MCP_SERVER_FIELDS"
                );
            }
        }
    }

    #[test]
    fn stdio_and_http_still_parse() {
        let stdio: McpServerConfig = serde_json::from_value(serde_json::json!({
            "command": "npx",
            "args": ["-y", "pkg"]
        }))
        .unwrap();
        assert!(stdio.enabled);
        assert!(matches!(
            stdio.transport,
            McpServerTransportConfig::Stdio { .. }
        ));

        let http: McpServerConfig = serde_json::from_value(serde_json::json!({
            "url": "https://mcp.example.com/mcp"
        }))
        .unwrap();
        assert!(matches!(
            http.transport,
            McpServerTransportConfig::StreamableHttp { .. }
        ));
        assert!(http.to_acp_mcp_server("x").is_some());
    }

    #[test]
    fn mcp_setup_schema_parses_and_missing_preference_requires_setup() {
        let config: McpConfig = serde_json::from_str(site_select_setup_json()).unwrap();
        let server = config.mcp_servers.get("acme").unwrap();
        let setup = server.setup.as_ref().unwrap();
        let [field] = setup.fields.as_slice() else {
            panic!("expected exactly one setup field: {:?}", setup.fields);
        };
        assert_eq!(field.id, "site");
        assert_eq!(field.default.as_deref(), Some("us1"));
        assert!(setup.variables.contains_key("url"));
        assert!(matches!(
            server.resolve_setup(None),
            McpSetupResolution::Required(_)
        ));
        assert!(server.to_acp_mcp_server("acme").is_none());
    }

    #[test]
    fn mcp_setup_valid_preference_resolves_mapped_url() {
        let config: McpConfig = serde_json::from_str(site_select_setup_json()).unwrap();
        let server = config.mcp_servers.get("acme").unwrap();
        let prefs = McpServerPreferences {
            values: HashMap::from([("site".to_string(), "us5".to_string())]),
            source: None,
            updated_at: None,
        };
        let resolved = match server.resolve_setup(Some(&prefs)) {
            McpSetupResolution::Resolved(config) => config,
            other => panic!("expected resolved config, got {other:?}"),
        };
        assert!(resolved.setup.is_none());
        assert!(resolved.to_acp_mcp_server("acme").is_some());
        match &resolved.transport {
            McpServerTransportConfig::StreamableHttp { url, .. } => {
                assert_eq!(url, "https://mcp.us5.example.com/v1/mcp");
            }
            _ => panic!("expected http config"),
        }
    }

    #[test]
    fn mcp_setup_invalid_preference_value_requires_setup() {
        let setup = McpSetupConfig {
            fields: vec![McpSetupField {
                id: "site".into(),
                label: "Site".into(),
                field_type: McpSetupFieldType::Select,
                required: true,
                default: Some("us1".into()),
                options: vec![McpSetupOption {
                    label: "US1".into(),
                    value: "us1".into(),
                }],
            }],
            variables: HashMap::new(),
        };
        let config = McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "{{url}}".into(),
                transport_type: None,
                bearer_token_env_var: None,
                headers: None,
                oauth_client_id: None,
                oauth_client_secret_env_var: None,
                oauth_scopes: None,
            },
            enabled: true,
            oauth: None,
            setup: Some(setup),
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            tool_timeouts: None,
            expose_image_base64: None,
        };
        let prefs = McpServerPreferences {
            values: HashMap::from([("site".to_string(), "us5".to_string())]),
            source: None,
            updated_at: None,
        };
        assert!(matches!(
            config.resolve_setup(Some(&prefs)),
            McpSetupResolution::Required(_)
        ));
    }

    #[test]
    fn mcp_setup_multi_field_schema_is_invalid() {
        let setup = McpSetupConfig {
            fields: vec![
                McpSetupField {
                    id: "a".into(),
                    label: "A".into(),
                    field_type: McpSetupFieldType::Select,
                    required: true,
                    default: None,
                    options: vec![McpSetupOption {
                        label: "1".into(),
                        value: "1".into(),
                    }],
                },
                McpSetupField {
                    id: "b".into(),
                    label: "B".into(),
                    field_type: McpSetupFieldType::Select,
                    required: true,
                    default: None,
                    options: vec![McpSetupOption {
                        label: "2".into(),
                        value: "2".into(),
                    }],
                },
            ],
            variables: HashMap::new(),
        };
        let config = McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "https://example.com".into(),
                transport_type: None,
                bearer_token_env_var: None,
                headers: None,
                oauth_client_id: None,
                oauth_client_secret_env_var: None,
                oauth_scopes: None,
            },
            enabled: true,
            oauth: None,
            setup: Some(setup),
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            tool_timeouts: None,
            expose_image_base64: None,
        };
        assert!(matches!(
            config.resolve_setup(None),
            McpSetupResolution::Invalid(_)
        ));
        assert!(config.to_acp_mcp_server("x").is_none());
    }
}
=======
>>>>>>> upstream
