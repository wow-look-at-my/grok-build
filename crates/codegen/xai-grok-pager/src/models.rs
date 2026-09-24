//! `grok models` subcommand.

use anyhow::Result;
use tokio_util::sync::CancellationToken;
use xai_grok_shell::agent::config::Config as AgentConfig;
use xai_grok_shell::cli_models::{AuthStatus, list_models};

use crate::client_identity::{PAGER_CLIENT_TYPE, pager_client_version};

pub async fn list_available_models(agent_config: &AgentConfig) -> Result<()> {
    let primary_auth = AuthStatus::resolve(agent_config);

    let cancel = CancellationToken::new();
    xai_grok_telemetry::startup::mark_utility_process();
    let spawned = crate::acp::spawn::spawn_grok_shell(agent_config.clone(), &cancel, None).await?;
    // Cancel and join on every return path, including the `?` below
    let _agent_guard =
        crate::acp::spawn::AgentShutdownGuard::new(cancel.clone(), Some(spawned.thread_handle));

    let state = list_models(
        &spawned.channel.tx,
        PAGER_CLIENT_TYPE,
        pager_client_version(),
    )
    .await?;
    println!("{}", auth_status_line(primary_auth));
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

fn auth_status_line(primary: AuthStatus) -> String {
    match primary {
        AuthStatus::ApiKey => "You are using XAI_API_KEY.".to_string(),
        AuthStatus::LoggedIn(host) => format!("You are logged in with {host}."),
        AuthStatus::ModelCredentials(model) => format!("Model '{model}' is using its own API key."),
        AuthStatus::NotAuthenticated => "You are not authenticated.".to_string(),
    }
}
