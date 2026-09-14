//! `send_message` — text between a session and the subagents it owns.
//!
//! Both directions ride the same tool. A parent addresses one of its children
//! by the subagent id `task` returned. A child addresses the session that
//! spawned it with the literal `parent`. A message arrives as a mid-turn user
//! message, so the recipient reads it at its next drain point and keeps the
//! work it is streaming.

use std::sync::Arc;

use crate::register_resource;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

use super::task::types::{
    SessionIdResource, SubagentEvent, SubagentEventSender, SubagentMessageChildRequest,
    SubagentMessageOutcome,
};

pub const SEND_MESSAGE_TOOL_NAME: &str = "send_message";

/// Recipient spellings that mean "the session that spawned me".
const PARENT_ALIASES: &[&str] = &["parent", "main", "parent_session", "main_session"];

/// True when `to` names the parent session rather than a subagent id.
pub fn addresses_parent(to: &str) -> bool {
    let normalized = to.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    PARENT_ALIASES.contains(&normalized.as_str())
}

// ---------------------------------------------------------------------------
// Parent delivery handle
// ---------------------------------------------------------------------------

type ParentDeliverFn = dyn Fn(&str) -> Result<(), String> + Send + Sync;

/// Host-injected route from a subagent to the session that spawned it.
///
/// Only a child session carries this resource. The host owns the provenance
/// text, because the host knows which subagent this session is and the tool
/// does not.
#[derive(Clone)]
pub struct ParentMessenger(Arc<ParentDeliverFn>);

impl ParentMessenger {
    pub fn new(deliver: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(deliver))
    }

    /// Deliver `text` to the parent session, or report why it could not land.
    pub fn deliver(&self, text: &str) -> Result<(), String> {
        (self.0)(text)
    }
}

impl std::fmt::Debug for ParentMessenger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParentMessenger").finish()
    }
}

register_resource!("grok_build", "ParentMessenger", ParentMessenger);

// ---------------------------------------------------------------------------
// Input / output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageInput {
    #[schemars(
        description = "Recipient. The subagent id returned by `task` to message one of your own subagents, or \"parent\" to message the session that spawned you."
    )]
    pub to: String,

    #[schemars(description = "The message text. The recipient reads it as a user message.")]
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageOutput {
    pub delivered_to: String,
    pub summary: String,
}

impl xai_tool_runtime::ToolOutput for SendMessageOutput {}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SendMessageTool;

impl crate::types::tool_metadata::ToolMetadata for SendMessageTool {
    fn kind(&self) -> ToolKind {
        ToolKind::SendMessage
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Send a message to a running subagent you spawned, or to the session that spawned you. \
         Set `to` to a subagent id from `task` to steer that subagent mid-run, or to \"parent\" \
         to report progress, ask a question, or hand back a finding without waiting for your run \
         to end. The recipient reads the text as a user message at its next step; it is not \
         interrupted mid-stream."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for SendMessageTool {
    type Args = SendMessageInput;
    type Output = SendMessageOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(SEND_MESSAGE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            SEND_MESSAGE_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "new_tool.send_message", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: SendMessageInput,
    ) -> Result<SendMessageOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;

        let text = input.message.trim().to_owned();
        if text.is_empty() {
            return Err(xai_tool_runtime::ToolError::custom(
                "send_message_empty",
                "`message` is empty. Send the text you want the recipient to read.",
            ));
        }
        let to = input.to.trim().to_owned();
        if to.is_empty() {
            return Err(xai_tool_runtime::ToolError::custom(
                "send_message_no_recipient",
                "`to` is empty. Name a subagent id from `task`, or \"parent\".",
            ));
        }

        let resources = shared_resources(&ctx)?;

        if addresses_parent(&to) {
            let messenger = {
                let res = resources.lock().await;
                res.get::<ParentMessenger>().cloned()
            };
            let messenger = messenger.ok_or_else(|| {
                xai_tool_runtime::ToolError::custom(
                    "send_message_no_parent",
                    "This session has no parent to message — it was not spawned by another \
                     agent. Reply to the user instead.",
                )
            })?;
            messenger.deliver(&text).map_err(|reason| {
                xai_tool_runtime::ToolError::custom(
                    "send_message_parent_unreachable",
                    format!("Could not deliver to the parent session: {reason}"),
                )
            })?;
            return Ok(SendMessageOutput {
                delivered_to: "parent".to_string(),
                summary: "Delivered to the parent session. It reads the message at its next \
                          step; there is no reply on this call."
                    .to_string(),
            });
        }

        let (sender, parent_session_id) = {
            let res = resources.lock().await;
            let sender = res.get::<SubagentEventSender>().map(|s| s.0.clone());
            let session_id = res.get::<SessionIdResource>().map(|s| s.0.clone());
            (sender, session_id)
        };
        let sender = sender.ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "send_message_no_subagents",
                "Subagents are not enabled for this session, so there is no subagent to message.",
            )
        })?;
        let parent_session_id = parent_session_id.ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "send_message_no_session_id",
                "This session has no id registered, so a subagent cannot be addressed safely.",
            )
        })?;

        let (respond_to, outcome_rx) = tokio::sync::oneshot::channel();
        sender
            .send(SubagentEvent::MessageChild(SubagentMessageChildRequest {
                parent_session_id,
                subagent_id: to.clone(),
                text,
                respond_to,
            }))
            .map_err(|_| {
                xai_tool_runtime::ToolError::custom(
                    "send_message_coordinator_closed",
                    "The subagent coordinator is gone — the session may be shutting down.",
                )
            })?;

        let outcome = outcome_rx.await.map_err(|_| {
            xai_tool_runtime::ToolError::custom(
                "send_message_no_ack",
                "The subagent coordinator dropped the response channel without answering. The \
                 message may not have been delivered.",
            )
        })?;

        match outcome {
            SubagentMessageOutcome::Delivered => Ok(SendMessageOutput {
                delivered_to: to,
                summary: "Delivered. The subagent reads the message at its next step; there is \
                          no reply on this call."
                    .to_string(),
            }),
            SubagentMessageOutcome::Queued => Ok(SendMessageOutput {
                delivered_to: to,
                summary: "The subagent has not started yet. The message is held and delivered \
                          the moment it starts."
                    .to_string(),
            }),
            SubagentMessageOutcome::NotOwned => Err(xai_tool_runtime::ToolError::custom(
                "send_message_not_owned",
                format!(
                    "Subagent `{to}` belongs to another session. You can only message the \
                         subagents you spawned."
                ),
            )),
            SubagentMessageOutcome::NotFound => Err(xai_tool_runtime::ToolError::custom(
                "send_message_unknown_subagent",
                format!(
                    "No running subagent with id `{to}`. It has already finished, or the id \
                         is wrong — `task` returns the id to use."
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_aliases_are_recognized_in_any_case_or_spelling() {
        for spelling in ["parent", "PARENT", " Parent ", "main", "parent-session"] {
            assert!(
                addresses_parent(spelling),
                "{spelling} should address parent"
            );
        }
    }

    #[test]
    fn a_subagent_id_never_reads_as_the_parent() {
        for id in [
            "0199f0c3-1a2b-7c3d-9e4f-5a6b7c8d9e0f",
            "parental",
            "my-parent-task",
        ] {
            assert!(!addresses_parent(id), "{id} should address a subagent");
        }
    }

    #[test]
    fn a_messenger_reports_the_hosts_failure_verbatim() {
        let messenger = ParentMessenger::new(|_| Err("parent session ended".to_string()));
        assert_eq!(
            messenger.deliver("hello"),
            Err("parent session ended".to_string())
        );
    }

    #[test]
    fn a_messenger_hands_the_text_to_the_host_unchanged() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let messenger = ParentMessenger::new(move |text| {
            sink.lock().expect("lock").push(text.to_owned());
            Ok(())
        });
        messenger.deliver("the build is red").expect("delivered");
        assert_eq!(*seen.lock().expect("lock"), vec!["the build is red"]);
    }
}
