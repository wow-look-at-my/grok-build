use super::support::*;
use super::*;
use xai_grok_tools::implementations::grok_build::grep::GrepTool;
use xai_grok_tools::registry::types::ToolConfig;

fn search_call(id: &str, path: &str) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            "search_code",
            serde_json::json!({"pattern": "needle-keep", "path": path}).to_string(),
        ),
        vendor: Default::default(),
    }
}

async fn tool_result_text(actor: &SessionActor, call_id: &str) -> String {
    let conv = actor.chat_state_handle.get_conversation().await;
    conv.iter()
        .rev()
        .find_map(|item| match item {
            xai_grok_sampling_types::ConversationItem::ToolResult(result)
                if result.tool_call_id == call_id =>
            {
                Some(result.content.to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_result for {call_id}"))
}

#[serial_test::serial(tool_call_telemetry)]
#[tokio::test(flavor = "current_thread")]
async fn renamed_grep_keeps_its_output_after_a_later_model_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = std::env::temp_dir().join(format!("grep-model-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("note.txt");
            std::fs::write(&file, "needle-keep\n").unwrap();
            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, _persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![
                ToolConfig::for_tool::<GrepTool>().with_name("search_code"),
            ])
            .await;
            let toolset = actor.agent.borrow().tool_bridge().toolset();
            let (id, version) = crate::session::telemetry::tool_identity(&toolset, "search_code");
            assert_eq!(id, "GrokBuild:grep");
            assert_eq!(version.as_deref(), Some("current"));
            let (unknown, unknown_version) =
                crate::session::telemetry::tool_identity(&toolset, "not_a_tool");
            assert_eq!(unknown, "opaque");
            assert_eq!(unknown_version, None);
            actor
                .workspace_ops
                .bind_local_session(
                    &actor.session_id_string(),
                    actor.tool_context.cwd.as_path().to_path_buf(),
                    actor.tool_context.hunk_tracker_handle.clone(),
                    toolset.clone(),
                    None,
                )
                .expect("bind_local_session");

            let mut deferred = Vec::new();
            let prepared = actor
                .prepare_tool_call(
                    search_call("prep", file.to_str().unwrap()),
                    &mut deferred,
                    Some("grok-4.6"),
                )
                .await
                .expect("prepare")
                .expect("search_code prepares");
            assert_eq!(prepared.tool_id, "GrokBuild:grep");
            assert_eq!(prepared.model_id.as_deref(), Some("grok-4.6"));
            assert_ne!(prepared.invocation_id, prepared.call_id);
            let mut config = actor
                .chat_state_handle
                .get_sampling_config()
                .await
                .expect("sampling config");
            config.model = "grok-4.5".into();
            actor
                .chat_state_handle
                .update_sampling_config(config.clone());
            assert_eq!(prepared.model_id.as_deref(), Some("grok-4.6"));

            actor
                .execute_tool_calls(
                    vec![search_call("grep-1", file.to_str().unwrap())],
                    Some("grok-4.6".into()),
                )
                .await
                .expect("execute");
            let direct = toolset
                .call(
                    "search_code",
                    serde_json::json!({"pattern": "needle-keep", "path": file.to_str().unwrap()}),
                    "direct",
                    None,
                )
                .await
                .expect("direct grep");
            let stored = tool_result_text(&actor, "grep-1").await;
            assert_eq!(stored, direct.prompt_text);
            let request = actor
                .chat_state_handle
                .build_request(Vec::new(), None, false, None, "conv".into(), "req".into())
                .await
                .expect("later request");
            assert_eq!(request.model.as_deref(), Some("grok-4.5"));
            let later = request
                .items
                .iter()
                .find_map(|item| match item {
                    xai_grok_sampling_types::ConversationItem::ToolResult(result)
                        if result.tool_call_id == "grep-1" =>
                    {
                        Some(result.content.to_string())
                    }
                    _ => None,
                })
                .expect("tool result in later request");
            assert_eq!(later, direct.prompt_text);
            let _ = std::fs::remove_dir_all(dir);
        })
        .await;
}


