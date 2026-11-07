// config_schema_tests.rs holds the config schema tests code.

use super::*;

const UPDATE_ENV: &str = "GROK_UPDATE_CONFIG_SCHEMA";

fn schema_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../schemas/config.schema.json")
}

#[test]
fn config_schema_file_matches_the_config_types() {
    let generated = config_json_schema_text();
    let path = schema_path();
    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap();
    assert!(
        committed == generated,
        "{} is not the schema the config types make. Regenerate it with \
         `{UPDATE_ENV}=1 cargo test -p xai-grok-shell --lib config_schema`.",
        path.display()
    );
    assert!(
        xai_grok_config::format::CONFIG_SCHEMA_JSON == generated,
        "xai_grok_config::format::CONFIG_SCHEMA_JSON is stale; rebuild after you regenerate the file"
    );
}

#[test]
fn root_properties_are_the_json_layout_sections() {
    let schema = config_json_schema();
    let props = schema["properties"].as_object().unwrap();
    for internal in [
        "model_providers",
        "toolset",
        "mcp_servers",
        "subagents",
        "grok_com_config",
    ] {
        assert!(
            !props.contains_key(internal),
            "`{internal}` is an internal key; the JSON layout moves it into a section"
        );
    }
    for section in [
        "models",
        "login",
        "ui",
        "tools",
        "mcp",
        "agents",
        "extensions",
        "policy",
    ] {
        assert!(props.contains_key(section), "no `{section}` section");
    }
    assert_eq!(schema["additionalProperties"], json!(false));
}

fn validator() -> jsonschema::Validator {
    jsonschema::validator_for(&config_json_schema()).unwrap()
}

fn errors(validator: &jsonschema::Validator, doc: &Value) -> Vec<String> {
    validator
        .iter_errors(doc)
        .map(|e| format!("{}: {e}", e.instance_path))
        .collect()
}

#[test]
fn a_full_json_layout_config_is_valid() {
    let doc = json!({
        "$schema": "./config.schema.json",
        "models": {
            "default": "grok-4",
            "providers": {
                "local": {
                    "base_url": "http://localhost:11434/v1",
                    "env_key": ["LOCAL_KEY", "OTHER_KEY"],
                    "api_backend": "chat_completions",
                    "extra_headers": { "x-team": "core" }
                }
            },
            "definitions": {
                "local/qwen": {
                    "model": "qwen3",
                    "model_provider": "local",
                    "context_window": 131072,
                    "reasoning_effort": "high"
                }
            }
        },
        "login": { "token_header": "x-grok", "force_login_team_uuid": ["a", "b"] },
        "ui": { "theme": "dark", "show_timestamps": true, "keep_text_selection": true },
        "tools": {
            "respect_gitignore": true,
            "bash": { "timeout_secs": 120, "auto_background_on_timeout": false },
            "web_search": { "allowed_domains": ["docs.rs"] }
        },
        "mcp": {
            "max_output_bytes": 65536,
            "servers": {
                "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] },
                "remote": { "url": "https://mcp.example.invalid/sse", "enabled": false }
            }
        },
        "agents": {
            "subagents": { "enabled": true, "max_depth": 2, "models": { "explore": "grok-4" } },
            "goal": { "enabled": true },
            "doom_loop_recovery": { "max_retries": "unlimited" }
        },
        "hooks": {
            "Stop": [
                { "matcher": "*", "hooks": [{ "type": "command", "command": "make check", "timeout": 60 }] }
            ]
        },
        "extensions": { "skills": { "paths": ["~/skills"] }, "plugins": { "enabled": ["demo"] } },
        "policy": {
            "allowed_mcp_servers": [{ "serverName": "github" }, { "serverUrl": "https://mcp.example.invalid/*" }],
            "allow_managed_hooks_only": false
        },
        "features": { "web_fetch": false, "codebase_indexing": ["src/**", "!target/**"] },
        "version_overrides": [
            { "minimum_version": "1.0.0", "ui": { "theme": "light" } }
        ],
        "campaigns": [{ "id": "c1", "models": { "default": "grok-4-fast" } }]
    });
    let validator = validator();
    assert_eq!(errors(&validator, &doc), Vec::<String>::new());
}

#[test]
fn an_unknown_root_key_is_invalid() {
    let validator = validator();
    let doc = json!({ "ui": { "theme": "dark" }, "toolset": { "bash": {} } });
    assert!(
        !validator.is_valid(&doc),
        "`toolset` is an internal key and must not validate at the root of config.json"
    );
    assert!(!validator.is_valid(&json!({ "bogus": 1 })));
    assert!(
        !validator.is_valid(
            &json!({ "version_overrides": [{ "minimum_version": "1.0.0", "bogus": 1 }] })
        ),
        "a patch element is held to the root keys"
    );
}

#[test]
fn a_wrong_type_is_invalid() {
    let validator = validator();
    assert!(!validator.is_valid(&json!({ "ui": { "show_timestamps": "yes" } })));
    assert!(!validator.is_valid(&json!({ "tools": { "bash": { "timeout_secs": "long" } } })));
    assert!(!validator.is_valid(&json!({ "mcp": { "servers": { "x": { "command": 5 } } } })));
}

/// A layer can disable a server that another layer defines, so an entry with no transport is valid.
#[test]
fn an_mcp_server_override_without_a_transport_is_valid() {
    let validator = validator();
    let doc = json!({ "mcp": { "servers": { "x": { "enabled": false } } } });
    assert!(
        validator.is_valid(&doc),
        "{:?}",
        validator
            .iter_errors(&doc)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
}
