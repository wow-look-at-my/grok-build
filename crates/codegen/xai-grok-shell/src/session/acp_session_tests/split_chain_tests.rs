//! Split-and-tee chains run in order. `&&` skips the rest of an and-list after a failure, and `;` runs on.

use super::command_split::{ChainMember, SplitChain};
use super::support::*;
use super::*;

fn todo_call(id: &str) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_string(),
        kind: "function".to_string(),
        function: crate::sampling::types::ToolCallFunction::new(
            "todo_write",
            r#"{"todos":[{"id":"t1","content":"do","status":"pending"}]}"#,
        ),
        vendor: Default::default(),
    }
}

fn chain(members: &[(&str, bool)]) -> SplitChain {
    SplitChain {
        members: members
            .iter()
            .map(|(id, and)| ChainMember {
                id: id.to_string(),
                only_if_previous_succeeded: *and,
            })
            .collect(),
    }
}

async fn tool_results(actor: &SessionActor) -> Vec<(String, String)> {
    actor
        .chat_state_handle
        .get_conversation()
        .await
        .into_iter()
        .filter_map(|item| match item {
            ConversationItem::ToolResult(r) => Some((r.tool_call_id, r.content.to_string())),
            _ => None,
        })
        .collect()
}

/// `a && b; c` where `a` fails: `b` is reported as not run and `c` runs.
#[tokio::test(flavor = "current_thread")]
async fn and_skips_after_a_failure_and_semicolon_runs_on() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;
            // No workspace session is bound, so every todo_write dispatch fails.
            actor
                .execute_tool_calls_with_chains(
                    vec![todo_call("a"), todo_call("a_split2"), todo_call("a_split3")],
                    vec![chain(&[
                        ("a", false),
                        ("a_split2", true),
                        ("a_split3", false),
                    ])],
                )
                .await
                .expect("execute_tool_calls_with_chains must not error");

            let results = tool_results(&actor).await;
            let ids: Vec<&str> = results.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(ids, ["a", "a_split2", "a_split3"], "{results:?}");
            assert!(!results[0].1.starts_with("Not run"), "{results:?}");
            assert!(
                results[1].1.starts_with("Not run: it was joined with &&"),
                "{results:?}"
            );
            assert!(!results[2].1.starts_with("Not run"), "{results:?}");
        })
        .await;
}

/// A chain member runs only after the member before it finished.
#[tokio::test(flavor = "current_thread")]
async fn a_chain_runs_after_its_predecessor_and_beside_other_calls() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;
            actor
                .workspace_ops
                .bind_local_session(
                    &actor.session_id_string(),
                    actor.tool_context.cwd.as_path().to_path_buf(),
                    actor.tool_context.hunk_tracker_handle.clone(),
                    actor.agent.borrow().tool_bridge().toolset(),
                    None,
                )
                .expect("bind_local_session must succeed");
            actor
                .execute_tool_calls_with_chains(
                    vec![todo_call("x"), todo_call("x_split2"), todo_call("other")],
                    vec![chain(&[("x", false), ("x_split2", true)])],
                )
                .await
                .expect("execute_tool_calls_with_chains must not error");

            let results = tool_results(&actor).await;
            let pos = |id: &str| results.iter().position(|(r, _)| r == id).unwrap();
            assert_eq!(results.len(), 3, "{results:?}");
            assert!(pos("x") < pos("x_split2"), "{results:?}");
            // `x` succeeded, so the && member ran.
            assert!(
                !results[pos("x_split2")].1.starts_with("Not run"),
                "{results:?}"
            );
        })
        .await;
}
