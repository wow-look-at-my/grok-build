//! `grok models` subcommand.

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use xai_grok_shell::agent::config::Config as AgentConfig;
use xai_grok_shell::cli_models::{AuthStatus, list_models};

use crate::client_identity::{PAGER_CLIENT_TYPE, PAGER_CLIENT_VERSION};

pub async fn list_available_models(agent_config: &AgentConfig) -> Result<()> {
    let primary_auth = AuthStatus::resolve(agent_config);

    let cancel = CancellationToken::new();
    xai_grok_telemetry::startup::mark_utility_process();
    let spawned = crate::acp::spawn::spawn_grok_shell(agent_config.clone(), &cancel, None).await?;
    // Cancel + join on every return path, including the `?` below.
    let _agent_guard =
        crate::acp::spawn::AgentShutdownGuard::new(cancel.clone(), Some(spawned.thread_handle));

    let state = list_models(&spawned.channel.tx, PAGER_CLIENT_TYPE, PAGER_CLIENT_VERSION).await?;
    let has_codex = state
        .available_models
        .iter()
        .any(|model| model.model_id.0.starts_with("codex/"));
    for line in auth_status_lines(primary_auth, has_codex) {
        println!("{line}");
    }
    println!();

    println!("Default model: {}", state.current_model_id.0);
    println!();
    println!("Available models:");
    for m in state.available_models {
        if m.model_id == state.current_model_id {
            println!("  * {} (default)", m.model_id.0);
        } else {
            println!("  - {}", m.model_id.0);
        }
    }

    Ok(())
}

fn auth_status_lines(primary: AuthStatus, has_codex: bool) -> Vec<String> {
    let mut lines = Vec::new();
    match primary {
        AuthStatus::ApiKey => lines.push("You are using XAI_API_KEY.".to_string()),
        AuthStatus::LoggedIn(host) => lines.push(format!("You are logged in with {host}.")),
        AuthStatus::ModelCredentials(model) => {
            lines.push(format!("Model '{model}' is using its own API key."));
        }
        AuthStatus::DeploymentKey => {
            lines.push("You are authenticated via deployment key.".to_string());
        }
        AuthStatus::NotAuthenticated if !has_codex => {
            lines.push("You are not authenticated.".to_string());
        }
        AuthStatus::NotAuthenticated => {}
    }
    if has_codex {
        lines.push("You are logged in with Codex (ChatGPT).".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_only_status_is_not_reported_as_unauthenticated() {
        assert_eq!(
            auth_status_lines(AuthStatus::NotAuthenticated, true),
            vec!["You are logged in with Codex (ChatGPT)."]
        );
    }

    #[test]
    fn multiple_provider_statuses_are_both_reported() {
        assert_eq!(
            auth_status_lines(AuthStatus::LoggedIn("grok.com".to_string()), true),
            vec![
                "You are logged in with grok.com.",
                "You are logged in with Codex (ChatGPT).",
            ]
        );
    }
}
