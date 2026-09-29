//! Experimental split-and-tee.

use std::sync::Arc;

use agent_client_protocol as acp;
use xai_grok_sampling_types::ConversationItem;
use xai_grok_sampling_types::conversation::ToolCall;
use xai_grok_tools::implementations::grok_build::bash::command_plan::split_command_list;

use super::SessionActor;

/// One tool call of a split chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChainMember {
    pub(super) id: String,
    pub(super) only_if_previous_succeeded: bool,
}

/// The tool calls one model call was split into, in run order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SplitChain {
    pub(super) members: Vec<ChainMember>,
}

/// How a finished tool call ended, as the next member of its chain reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallOutcome {
    Succeeded,
    Failed,
    /// The command still runs in the background. A later command would run beside it, not after it.
    Backgrounded,
}

/// What the next member of a chain does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChainStep {
    Run,
    /// Report `reason` as the call's result and carry `outcome` to the next
    /// member.
    Skip {
        reason: String,
        outcome: CallOutcome,
    },
}

pub(super) fn chain_step(
    previous: Option<CallOutcome>,
    only_if_previous_succeeded: bool,
) -> ChainStep {
    match previous {
        Some(CallOutcome::Backgrounded) => ChainStep::Skip {
            reason: "Not run: the command before it in the same call moved to the background and \
			         can still be running, so this one would not run after it. Run it again once \
			         that task finishes."
                .to_string(),
            outcome: CallOutcome::Backgrounded,
        },
        Some(CallOutcome::Succeeded) => ChainStep::Run,
        Some(CallOutcome::Failed) | None if only_if_previous_succeeded => ChainStep::Skip {
            reason:
                "Not run: it was joined with && to the command before it, which did not succeed."
                    .to_string(),
            outcome: CallOutcome::Failed,
        },
        Some(CallOutcome::Failed) | None => ChainStep::Run,
    }
}

/// The argument keys of the bash tool, as this session's model sees them.
pub(super) struct BashKeys<'a> {
    pub(super) command: &'a str,
    pub(super) is_background: &'a str,
}

/// Replace each splittable bash call in `items` with one call per command.
///
/// The first call keeps the model's id, so it takes over the row the pager
/// drew while the model wrote it. The provider fields ride on that first
/// call only, since they belong to the call the model made.
pub(super) fn split_bash_calls(
    items: &mut [ConversationItem],
    is_bash: impl Fn(&str) -> bool,
    keys: &BashKeys<'_>,
) -> Vec<SplitChain> {
    let mut chains = Vec::new();
    for item in items {
        let ConversationItem::Assistant(assistant) = item else {
            continue;
        };
        if !assistant.tool_calls.iter().any(|c| is_bash(&c.name)) {
            continue;
        }
        let mut calls = Vec::with_capacity(assistant.tool_calls.len());
        for call in std::mem::take(&mut assistant.tool_calls) {
            match split_call(&call, &is_bash, keys) {
                Some((split, chain)) => {
                    calls.extend(split);
                    chains.push(chain);
                }
                None => calls.push(call),
            }
        }
        assistant.tool_calls = calls;
    }
    chains
}

fn split_call(
    call: &ToolCall,
    is_bash: &impl Fn(&str) -> bool,
    keys: &BashKeys<'_>,
) -> Option<(Vec<ToolCall>, SplitChain)> {
    if !is_bash(&call.name) {
        return None;
    }
    let serde_json::Value::Object(args) = serde_json::from_str(&call.arguments).ok()? else {
        return None;
    };
    let background = args
        .get(keys.is_background)
        .and_then(xai_tool_types::serde_lenient::lenient_bool_from_json)
        .unwrap_or(false);
    if background {
        return None;
    }
    let segments = split_command_list(args.get(keys.command)?.as_str()?)?;
    let mut calls = Vec::with_capacity(segments.len());
    let mut members = Vec::with_capacity(segments.len());
    for (idx, segment) in segments.into_iter().enumerate() {
        let id = if idx == 0 {
            call.id.to_string()
        } else {
            format!("{}_split{}", call.id, idx + 1)
        };
        let mut args = args.clone();
        args.insert(
            keys.command.to_string(),
            serde_json::Value::String(segment.command),
        );
        calls.push(ToolCall {
            id: Arc::from(id.as_str()),
            name: call.name.clone(),
            arguments: Arc::from(serde_json::Value::Object(args).to_string()),
            vendor: if idx == 0 {
                call.vendor.clone()
            } else {
                Default::default()
            },
        });
        members.push(ChainMember {
            id,
            only_if_previous_succeeded: segment.only_if_previous_succeeded,
        });
    }
    Some((calls, SplitChain { members }))
}

impl SessionActor {
    /// Split the joined bash calls of a model response, when the session runs
    /// split-and-tee. Returns the chains to run in order.
    pub(super) async fn split_joined_bash_calls(
        &self,
        items: &mut [ConversationItem],
    ) -> Vec<SplitChain> {
        use xai_grok_tools::types::tool::{ToolKind, ToolNamespace};
        if !self.agent.borrow().reminder_policy().split_joined_commands
            || xai_grok_config::shell::ampersand_semantics()
                != xai_grok_config::shell::AmpersandSemantics::PosixBackground
        {
            return Vec::new();
        }
        let bridge = self.agent.borrow().tool_bridge().clone();
        let renderer = bridge.template_renderer_snapshot().await;
        let param = |name: &'static str| {
            renderer
                .as_ref()
                .and_then(|r| r.param_for_kind(ToolKind::Execute, name))
                .unwrap_or(name)
                .to_string()
        };
        let (command, is_background) = (param("command"), param("is_background"));
        let keys = BashKeys {
            command: &command,
            is_background: &is_background,
        };
        // Only the grok_build bash tool: its terminal keeps the cwd and the
        // exported variables between calls, which a split relies on.
        let is_bash = |name: &str| {
            bridge.tool_kind(name) == Some(ToolKind::Execute)
                && bridge.tool_namespace(name) == Some(ToolNamespace::GrokBuild)
        };
        split_bash_calls(items, is_bash, &keys)
    }

    /// Report a chain member that did not run, with its own row and result.
    pub(super) async fn skip_chain_call(
        &self,
        call: crate::sampling::types::ToolCallResponse,
        reason: String,
    ) -> Result<(), acp::Error> {
        let tool_call_id = acp::ToolCallId::new(Arc::from(call.id.clone()));
        let raw_input = serde_json::from_str::<serde_json::Value>(&call.function.arguments).ok();
        self.send_update(
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(tool_call_id.clone(), call.function.name.clone())
                    .kind(acp::ToolKind::Execute)
                    .status(acp::ToolCallStatus::Pending)
                    .raw_input(raw_input)
                    .meta(self.stamp_tool_meta(None, &call.function.name, None)),
            ),
            None,
        )
        .await;
        self.handle_tool_not_executed(&call.id, &tool_call_id, reason)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::conversation::AssistantItem;

    const KEYS: BashKeys<'static> = BashKeys {
        command: "command",
        is_background: "is_background",
    };

    fn assistant(calls: Vec<ToolCall>) -> ConversationItem {
        ConversationItem::Assistant(AssistantItem {
            content: Arc::from(""),
            tool_calls: calls,
            model_id: None,
            model_fingerprint: None,
            reasoning_effort: None,
        })
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: Arc::from(id),
            name: name.to_string(),
            arguments: Arc::from(args.to_string()),
            vendor: [("sig".to_string(), serde_json::json!("abc"))]
                .into_iter()
                .collect(),
        }
    }

    fn commands(item: &ConversationItem) -> Vec<(String, String)> {
        let ConversationItem::Assistant(a) = item else {
            panic!()
        };
        a.tool_calls
            .iter()
            .map(|c| {
                let args: serde_json::Value = serde_json::from_str(&c.arguments).unwrap();
                (
                    c.id.to_string(),
                    args["command"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_joined_bash_call_becomes_one_call_per_command() {
        let mut items = vec![assistant(vec![
            call("r1", "read_file", serde_json::json!({"path": "a"})),
            call(
                "b1",
                "run_terminal_cmd",
                serde_json::json!({"command": "cd x; make && make test", "description": "build"}),
            ),
        ])];
        let chains = split_bash_calls(&mut items, |n| n == "run_terminal_cmd", &KEYS);
        assert_eq!(
            commands(&items[0]),
            vec![
                ("r1".into(), String::new()),
                ("b1".into(), "cd x".into()),
                ("b1_split2".into(), "make".into()),
                ("b1_split3".into(), "make test".into()),
            ]
        );
        assert_eq!(
            chains,
            vec![SplitChain {
                members: vec![
                    ChainMember {
                        id: "b1".into(),
                        only_if_previous_succeeded: false
                    },
                    ChainMember {
                        id: "b1_split2".into(),
                        only_if_previous_succeeded: false
                    },
                    ChainMember {
                        id: "b1_split3".into(),
                        only_if_previous_succeeded: true
                    },
                ]
            }]
        );
        let ConversationItem::Assistant(a) = &items[0] else {
            panic!()
        };
        // The description rides on every call; the provider fields only on the first.
        let second: serde_json::Value = serde_json::from_str(&a.tool_calls[2].arguments).unwrap();
        assert_eq!(second["description"], "build");
        assert!(!a.tool_calls[1].vendor.is_empty());
        assert!(a.tool_calls[2].vendor.is_empty());
    }

    #[test]
    fn background_single_and_unsplittable_calls_stay_whole() {
        for args in [
            serde_json::json!({"command": "a; b", "is_background": true}),
            serde_json::json!({"command": "a; b", "is_background": "true"}),
            serde_json::json!({"command": "cargo test | tail -5"}),
            serde_json::json!({"command": "X=1; echo $X"}),
            serde_json::json!({"cmd": "a; b"}),
        ] {
            let mut items = vec![assistant(vec![call(
                "b1",
                "run_terminal_cmd",
                args.clone(),
            )])];
            assert!(
                split_bash_calls(&mut items, |_| true, &KEYS).is_empty(),
                "{args}"
            );
            let ConversationItem::Assistant(a) = &items[0] else {
                panic!()
            };
            assert_eq!(a.tool_calls.len(), 1);
        }
        // Another tool's `command` argument is not a shell command.
        let mut items = vec![assistant(vec![call(
            "m1",
            "mcp__x",
            serde_json::json!({"command": "a; b"}),
        )])];
        assert!(split_bash_calls(&mut items, |n| n == "run_terminal_cmd", &KEYS).is_empty());
    }

    #[test]
    fn renamed_argument_keys_are_honoured() {
        let keys = BashKeys {
            command: "cmd_x",
            is_background: "bg_x",
        };
        let mut items = vec![assistant(vec![call(
            "b1",
            "shell",
            serde_json::json!({"cmd_x": "a; b"}),
        )])];
        assert_eq!(split_bash_calls(&mut items, |_| true, &keys).len(), 1);
    }

    #[test]
    fn chain_steps_follow_shell_semantics() {
        use CallOutcome::*;
        assert_eq!(chain_step(Some(Succeeded), true), ChainStep::Run);
        assert_eq!(chain_step(Some(Failed), false), ChainStep::Run);
        assert!(matches!(
            chain_step(Some(Failed), true),
            ChainStep::Skip {
                outcome: Failed,
                ..
            }
        ));
        // A call that never reported (rejected, hook-denied) counts as failed.
        assert!(matches!(
            chain_step(None, true),
            ChainStep::Skip {
                outcome: Failed,
                ..
            }
        ));
        assert_eq!(chain_step(None, false), ChainStep::Run);
        // A backgrounded command stops the rest of the chain, `;` or `&&`.
        assert!(matches!(
            chain_step(Some(Backgrounded), false),
            ChainStep::Skip {
                outcome: Backgrounded,
                ..
            }
        ));
    }
}
