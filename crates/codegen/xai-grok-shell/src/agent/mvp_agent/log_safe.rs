//! Views of credential-carrying values for a log line. Names stay and values go.

use serde_json::Value;

const REDACTED: &str = "[redacted]";

fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value)
        .unwrap_or_else(|error| Value::String(format!("not serializable: {error}")))
}

/// Replace every value of a JSON object.
fn redact_object_values(object: Option<&mut Value>) {
    let Some(Value::Object(map)) = object else {
        return;
    };
    for value in map.values_mut() {
        *value = Value::String(REDACTED.to_owned());
    }
}

/// Replace the `value` of every `{name, value}` entry in a JSON array.
fn redact_entry_values(entries: Option<&mut Value>) {
    let Some(Value::Array(entries)) = entries else {
        return;
    };
    for entry in entries {
        if let Some(value) = entry.get_mut("value") {
            *value = Value::String(REDACTED.to_owned());
        }
    }
}

/// A session request. Each MCP server keeps its header names and its
/// environment variable names.
pub(super) fn session_request(request: &impl serde::Serialize) -> Value {
    let mut request = to_value(request);
    let Some(Value::Array(servers)) = request.get_mut("mcpServers") else {
        return request;
    };
    for server in servers {
        redact_entry_values(server.get_mut("headers"));
        redact_entry_values(server.get_mut("env"));
    }
    request
}

/// A sampler config. The key is reported as present, and each header and
/// query parameter keeps its name.
pub(super) fn sampler_config(config: &impl serde::Serialize) -> Value {
    let mut config = to_value(config);
    if let Some(key) = config.get_mut("api_key")
        && !key.is_null()
    {
        *key = Value::String(REDACTED.to_owned());
    }
    redact_object_values(config.get_mut("extra_headers"));
    redact_object_values(config.get_mut("query_params"));
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol as acp;

    #[test]
    fn session_request_keeps_names_and_drops_header_and_env_values() {
        let request: acp::NewSessionRequest = serde_json::from_value(serde_json::json!({
            "cwd": "/work",
            "mcpServers": [
                {
                    "type": "http",
                    "name": "search",
                    "url": "https://mcp.example/mcp",
                    "headers": [{ "name": "Authorization", "value": "Bearer header-secret" }],
                },
                {
                    "name": "local",
                    "command": "npx",
                    "args": [],
                    "env": [{ "name": "TOKEN", "value": "env-secret" }],
                },
            ],
        }))
        .expect("a session request with an HTTP and a stdio MCP server");

        let logged = session_request(&request).to_string();

        assert!(!logged.contains("header-secret"), "{logged}");
        assert!(!logged.contains("env-secret"), "{logged}");
        for kept in ["search", "https://mcp.example/mcp", "Authorization", "npx", "TOKEN"] {
            assert!(logged.contains(kept), "{kept} is missing from {logged}");
        }
    }

    #[test]
    fn sampler_config_keeps_names_and_drops_key_header_and_query_values() {
        let config = serde_json::json!({
            "api_key": "sk-key-secret",
            "base_url": "https://gateway.example/v1",
            "extra_headers": { "X-Relay-Token": "relay-secret" },
            "query_params": { "api-version": "query-secret" },
            "env_http_headers": { "X-Tenant": "TENANT_VAR" },
        });

        let logged = sampler_config(&config).to_string();

        for secret in ["sk-key-secret", "relay-secret", "query-secret"] {
            assert!(!logged.contains(secret), "{secret} is in {logged}");
        }
        for kept in [
            "https://gateway.example/v1",
            "X-Relay-Token",
            "api-version",
            "TENANT_VAR",
        ] {
            assert!(logged.contains(kept), "{kept} is missing from {logged}");
        }
    }

    #[test]
    fn sampler_config_reports_an_absent_key_as_absent() {
        let logged = sampler_config(&serde_json::json!({ "api_key": null }));

        assert_eq!(logged["api_key"], Value::Null);
    }
}
