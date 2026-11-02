//! `@sonnet`, `@haiku` and `@opus`: the user spawns a background subagent on a model family.

use super::*;

use xai_grok_tools::implementations::grok_build::task::backend::{ChannelBackend, SubagentBackend};
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
};

/// The model families a mention can name.
pub const MODEL_MENTION_FAMILIES: &[&str] = &["sonnet", "haiku", "opus"];

/// What a mention spawned.
#[derive(Debug, Clone)]
pub struct ModelMentionSpawn {
    pub subagent_id: String,
    pub model_id: String,
}

/// Pick the newest catalog model whose id names `family`. A higher Claude
/// version wins. Between equal versions the shorter id wins, so the moving
/// alias beats a dated snapshot.
pub(crate) fn resolve_family_model<'a>(
    family: &str,
    ids: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let family = family.to_ascii_lowercase();
    ids.into_iter()
        .filter(|id| id.to_ascii_lowercase().contains(&family))
        .max_by(|a, b| {
            claude_version(a)
                .cmp(&claude_version(b))
                .then_with(|| b.len().cmp(&a.len()))
                .then_with(|| b.cmp(a))
        })
        .map(str::to_owned)
}

/// The `(major, minor)` in a Claude id: `claude-sonnet-4-5`, `claude-3-7-sonnet`, or the
/// same behind a gateway prefix. The first part of one or digits is the major, so a
/// snapshot date is never read as a version.
fn claude_version(id: &str) -> Option<(u32, u32)> {
    let lower = id.to_ascii_lowercase();
    let tail = lower.split("claude").nth(1).unwrap_or(&lower);
    let mut parts = tail
        .split(['-', '.', '_', '@', ':', '/'])
        .filter(|p| !p.is_empty())
        .skip_while(|p| !matches!(p.len(), 1 | 2) || !p.bytes().all(|b| b.is_ascii_digit()));
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()
        .filter(|p| matches!(p.len(), 1 | 2) && p.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor))
}

/// A short label for the subagent row: the first words of the task.
fn mention_description(family: &str, prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(5).collect();
    if words.is_empty() {
        format!("@{family}")
    } else {
        format!("@{family}: {}", words.join(" "))
    }
}

impl SessionActor {
    /// Spawn a background subagent on the newest model of `family`, with
    /// `effort` when set. The call returns once the coordinator records the
    /// child. An unknown family, a family with no model in the catalog, or a
    /// refused spawn returns an error the user reads.
    pub(super) async fn handle_spawn_model_agent(
        &self,
        family: &str,
        effort: Option<&str>,
        prompt: &str,
    ) -> Result<ModelMentionSpawn, String> {
        let family = family.to_ascii_lowercase();
        if !MODEL_MENTION_FAMILIES.contains(&family.as_str()) {
            return Err(format!(
                "@{family} is not a model family. Use @{}.",
                MODEL_MENTION_FAMILIES.join(", @")
            ));
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(format!("@{family} needs a task after it."));
        }
        if let Some(raw) = effort
            && let Err(err) = raw.parse::<crate::sampling::ReasoningEffort>()
        {
            return Err(format!("'{raw}' is not an effort level: {err}"));
        }
        let snapshot = self.models_manager.task_model_catalog_snapshot(true);
        let Some(model_id) =
            resolve_family_model(&family, snapshot.eligible.iter().map(|m| m.id.as_str()))
        else {
            let mut ids: Vec<&str> = snapshot.eligible.iter().map(|m| m.id.as_str()).collect();
            ids.sort_unstable();
            return Err(format!(
                "No {family} model is available. Available models: {}.",
                if ids.is_empty() {
                    "none".to_owned()
                } else {
                    ids.join(", ")
                }
            ));
        };
        let Some(event_tx) = self.tool_context.subagent_event_tx.clone() else {
            return Err("This session cannot spawn subagents.".to_owned());
        };
        // A user Stop latches spawns closed until a turn reopens them.
        self.open_subagent_spawn_admission();

        let id = uuid::Uuid::now_v7().to_string();
        let request = SubagentRequest {
            id: id.clone(),
            prompt: prompt.to_owned(),
            description: mention_description(&family, prompt),
            subagent_type: xai_tool_types::default_subagent_type(),
            parent_session_id: self.session_id_string(),
            // No owning prompt: a Stop on the main turn must not cancel a child the user started.
            parent_prompt_id: None,
            resume_from: None,
            cwd: Some(self.tool_context.cwd.as_str().to_owned()),
            runtime_overrides: SubagentRuntimeOverrides {
                model: Some(model_id.clone()),
                reasoning_effort: effort.map(str::to_owned),
                ..Default::default()
            },
            run_in_background: true,
            surface_completion: true,
            await_to_completion: false,
            fork_context: false,
            owner: SubagentOwner::Task,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            spawn_root: Default::default(),
            tool_call_id: None,
        };

        let backend = ChannelBackend::for_session(event_tx, self.session_id_string());
        let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
        let mut spawn_task =
            tokio::spawn(async move { backend.spawn(request, Some(registered_tx)).await });
        tokio::select! {
            biased;
            registered = registered_rx => {
                if registered.is_err() {
                    return Err(spawn_failure(spawn_task.await));
                }
            }
            joined = &mut spawn_task => {
                return Err(spawn_failure(joined));
            }
        }
        let log_id = id.clone();
        tokio::spawn(async move {
            if let Ok(Ok(result)) = spawn_task.await
                && !result.success
            {
                tracing::warn!(
                    subagent_id = %log_id,
                    error = ?result.error,
                    "model mention subagent failed",
                );
            }
        });
        Ok(ModelMentionSpawn {
            subagent_id: id,
            model_id,
        })
    }
}

/// The reason a spawn ended before the coordinator recorded the child.
fn spawn_failure(
    joined: Result<
        Result<
            xai_grok_tools::implementations::grok_build::task::types::SubagentResult,
            xai_tool_runtime::ToolError,
        >,
        tokio::task::JoinError,
    >,
) -> String {
    match joined {
        Ok(Ok(result)) => result
            .error
            .unwrap_or_else(|| "the subagent ended before it started".to_owned()),
        Ok(Err(err)) => err.to_string(),
        Err(err) => format!("the spawn task failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_newest_model_of_the_family() {
        let ids = [
            "claude-sonnet-4-5",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-3-7-sonnet-20250219",
            "claude-haiku-5-5",
        ];
        assert_eq!(
            resolve_family_model("sonnet", ids).as_deref(),
            Some("claude-sonnet-5-5")
        );
        assert_eq!(
            resolve_family_model("opus", ids).as_deref(),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            resolve_family_model("HAIKU", ids).as_deref(),
            Some("claude-haiku-5-5")
        );
    }

    #[test]
    fn the_alias_beats_its_dated_snapshot() {
        let ids = ["claude-sonnet-4-5-20250929", "claude-sonnet-4-5"];
        assert_eq!(
            resolve_family_model("sonnet", ids).as_deref(),
            Some("claude-sonnet-4-5")
        );
    }

    #[test]
    fn a_gateway_prefix_still_resolves() {
        let ids = [
            "anthropic/claude-sonnet-4.6",
            "anthropic/claude-sonnet-4.5",
            "grok-4",
        ];
        assert_eq!(
            resolve_family_model("sonnet", ids).as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
    }

    #[test]
    fn no_model_of_the_family_resolves_to_none() {
        assert_eq!(
            resolve_family_model("opus", ["grok-4", "claude-sonnet-4-5"]),
            None
        );
    }

    #[test]
    fn the_description_names_the_family_and_the_task() {
        assert_eq!(
            mention_description("sonnet", "fix the flaky test in the pager crate please"),
            "@sonnet: fix the flaky test in"
        );
        assert_eq!(mention_description("haiku", "  "), "@haiku");
    }
}
