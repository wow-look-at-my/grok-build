//! Tests for the Messages API conversion.

use super::test_support::*;
use super::*;

fn messages_test_request(reasoning_effort: Option<crate::ReasoningEffort>) -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::user("Hello")],
        model: Some("test-model".to_string()),
        reasoning_effort,
        ..Default::default()
    }
}

#[test]
fn json_schema_and_reasoning_effort_are_orthogonal_in_output_config() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "x": { "type": "string" } },
        "required": ["x"]
    });
    let mut req = ConversationRequest::from_items(vec![ConversationItem::user("go")])
        .with_json_schema(schema);
    req.reasoning_effort = Some(crate::ReasoningEffort::High);

    let msgs = build_messages_request(&req);
    let oc = msgs.output_config.expect("output_config present");
    assert_eq!(oc.effort.as_deref(), Some("high"));
    assert!(oc.format.is_some());
    assert!(
        msgs.thinking.is_some(),
        "thinking set when effort is present"
    );
}

#[test]
fn test_messages_request_wire_format_for_supported_variants() {
    for (variant, expected) in [
        (crate::ReasoningEffort::Low, "low"),
        (crate::ReasoningEffort::Medium, "medium"),
        (crate::ReasoningEffort::High, "high"),
        (crate::ReasoningEffort::Xhigh, "xhigh"),
        (crate::ReasoningEffort::Max, "max"),
    ] {
        let req = messages_test_request(Some(variant));
        let msgs = build_messages_request(&req);
        let json = serde_json::to_value(&msgs).unwrap();
        assert_eq!(
            json.pointer("/output_config/effort")
                .and_then(|v| v.as_str()),
            Some(expected),
            "{variant:?} should map to output_config.effort={expected:?}; got: {json:#}",
        );
        assert_eq!(
            json.pointer("/thinking/type").and_then(|v| v.as_str()),
            Some("adaptive"),
            "{variant:?} should auto-pair thinking.type=adaptive; got: {json:#}",
        );
    }
}

#[test]
fn test_messages_request_omits_output_config_when_no_supported_effort() {
    let none_or_unsupported = [
        None,
        Some(crate::ReasoningEffort::None),
        Some(crate::ReasoningEffort::Minimal),
    ];
    for input in none_or_unsupported {
        let req = messages_test_request(input);
        let msgs = build_messages_request(&req);
        assert!(
            msgs.output_config.is_none(),
            "input {input:?} must not produce output_config",
        );
        assert!(
            msgs.thinking.is_none(),
            "input {input:?} must not auto-pair thinking",
        );
    }
}

#[test]
fn test_messages_request_thinking_carries_summarized_display() {
    let req = ConversationRequest {
        reasoning_effort: Some(crate::ReasoningEffort::High),
        ..ConversationRequest::from_items(vec![ConversationItem::user("hi")])
            .with_model("messages-compatible-model")
    };
    let msg = build_messages_request(&req);
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(
        json.pointer("/thinking/type").and_then(|v| v.as_str()),
        Some("adaptive"),
        "thinking.type should be 'adaptive'; got: {json:#}",
    );
    assert_eq!(
        json.pointer("/thinking/display").and_then(|v| v.as_str()),
        Some("summarized"),
        "thinking.display must be 'summarized' so 4.7+ surfaces thinking content; got: {json:#}",
    );
}

#[test]
fn test_messages_request_omits_thinking_when_effort_unset() {
    let req = ConversationRequest::from_items(vec![ConversationItem::user("hi")])
        .with_model("messages-compatible-model");
    let msg = build_messages_request(&req);
    let json = serde_json::to_value(&msg).unwrap();
    assert!(
        json.get("thinking").is_none()
            || json
                .pointer("/thinking")
                .map(|v| v.is_null())
                .unwrap_or(false),
        "thinking must be absent when reasoning_effort is unset; got: {json:#}",
    );
    assert!(
        json.get("output_config").is_none()
            || json
                .pointer("/output_config")
                .map(|v| v.is_null())
                .unwrap_or(false),
        "output_config must be absent when reasoning_effort is unset; got: {json:#}",
    );
}

#[test]
fn test_messages_request_previous_tip_skips_a_trailing_user_run() {
    let mut items = vec![
        ConversationItem::system("You are a helpful assistant."),
        ConversationItem::user("Fix the bug"),
    ];
    items.extend(agent_turn(0));
    items.extend(agent_turn(1));
    // The shape after a parallel batch: tool results, then followups.
    items.push(ConversationItem::user("[Image content]"));
    items.push(ConversationItem::user("<system-reminder>"));

    let json = serde_json::to_value(build_messages_request(
        &ConversationRequest::from_items(items).with_model("messages-compatible-model"),
    ))
    .unwrap();
    let messages = json["messages"].as_array().unwrap();

    let marked: Vec<usize> = (0..messages.len())
        .filter(|&i| marker_on_last_block(&messages[i]).is_some())
        .collect();
    let last_assistant = messages
        .iter()
        .rposition(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(marked.len(), 2, "tip and previous tip only: {json:#}");
    assert_eq!(marked[1], messages.len() - 1, "tip: {json:#}");
    assert!(
        marked[0] < last_assistant,
        "the previous tip must sit before the last assistant turn, not inside \
             the trailing user run; got {marked:?} in {json:#}",
    );
}

#[test]
fn test_messages_request_cache_breakpoint_marks_an_image_tip() {
    let req = ConversationRequest::from_items(vec![
        ConversationItem::system("You are a helpful assistant."),
        ConversationItem::User(UserItem {
            content: vec![
                ContentPart::Text {
                    text: "what is in this screenshot".into(),
                },
                ContentPart::Image {
                    url: "data:image/png;base64,iVBOR".into(),
                },
            ],
            ..Default::default()
        }),
    ])
    .with_model("messages-compatible-model");

    let json = serde_json::to_value(build_messages_request(&req)).unwrap();
    let blocks = json["messages"][0]["content"].as_array().unwrap();

    assert_eq!(blocks.last().unwrap()["type"].as_str(), Some("image"));
    assert_eq!(
        marker_on_last_block(&json["messages"][0]),
        Some("ephemeral"),
        "{json:#}",
    );
    assert!(blocks[0].get("cache_control").is_none(), "{json:#}");
}

#[test]
fn test_messages_request_cache_breakpoint_skips_thinking() {
    let req = ConversationRequest::from_items(vec![
        ConversationItem::user("Fix the bug"),
        ConversationItem::Reasoning(synthesized_reasoning_item("weighing options")),
        ConversationItem::assistant("Fixed it."),
    ])
    .with_model("messages-compatible-model");

    let json = serde_json::to_value(build_messages_request(&req)).unwrap();
    let blocks = json["messages"][1]["content"].as_array().unwrap();

    let thinking = blocks
        .iter()
        .find(|b| b["type"] == "thinking")
        .expect("reasoning should emit a thinking block");
    assert!(thinking.get("cache_control").is_none(), "{json:#}");
    assert_eq!(
        marker_on_last_block(&json["messages"][1]),
        Some("ephemeral"),
        "{json:#}",
    );
}

#[test]
fn test_btw_stripped_reasoning_produces_no_thinking_blocks() {
    // Simulate a conversation where the model responded with thinking.
    let with_reasoning = ConversationItem::Assistant(AssistantItem {
        content: "Here is the answer.".into(),
        tool_calls: vec![],
        model_id: Some("messages-compatible-model".into()),
        model_fingerprint: None,
        reasoning_effort: None,
    });

    // Reasoning now lives as a sibling `ConversationItem::Reasoning`,
    // so "stripping reasoning" means filtering those siblings out — see
    // `strip_reasoning_blocks` in xai-chat-state. Here the assistant
    // never had a sibling Reasoning, so the strip is a no-op.
    let stripped = with_reasoning;

    let req = ConversationRequest::from_items(vec![
        ConversationItem::user("hello"),
        stripped,
        ConversationItem::user("btw what is X?"),
    ]);

    let msg = build_messages_request(&req);
    let json = serde_json::to_value(&msg).unwrap();

    // No thinking blocks should appear in any message.
    let messages = json.get("messages").unwrap().as_array().unwrap();
    for (i, m) in messages.iter().enumerate() {
        if let Some(content) = m.get("content").and_then(|c| c.as_array()) {
            for block in content {
                assert_ne!(
                    block.get("type").and_then(|t| t.as_str()),
                    Some("thinking"),
                    "message[{i}] must not contain thinking blocks after stripping reasoning",
                );
            }
        }
    }

    // Top-level thinking must also be absent.
    assert!(
        json.get("thinking").is_none()
            || json
                .pointer("/thinking")
                .map(|v| v.is_null())
                .unwrap_or(false),
        "top-level thinking must be absent; got: {json:#}",
    );
}

#[test]
fn test_btw_mid_turn_truncation_removes_trailing_tool_use() {
    // Simulate a conversation that was snapshotted mid-turn: the last
    // assistant made a tool call that hasn't been answered yet.
    let mut items = vec![
        ConversationItem::system("You are a helpful assistant."),
        ConversationItem::user("Fix the bug"),
        ConversationItem::assistant("I'll look at the code."),
        // Completed tool call pair:
        ConversationItem::assistant_tool_calls(vec![ToolCall {
            id: "call_1".into(),
            name: "read_file".to_string(),
            arguments: r#"{"path": "src/main.rs"}"#.into(),
        }]),
        ConversationItem::tool_result("call_1", "fn main() {}"),
        ConversationItem::assistant("I see the issue. Let me fix it."),
        // Mid-turn: tool call with NO tool_result yet
        ConversationItem::assistant_tool_calls(vec![ToolCall {
            id: "call_2".into(),
            name: "search_replace".to_string(),
            arguments: "{}".into(),
        }]),
    ];

    // Apply the same truncation pattern as handle_side_question.
    while let Some(last) = items.last() {
        match last {
            ConversationItem::Assistant(a) if !a.tool_calls.is_empty() => {
                items.pop();
            }
            ConversationItem::ToolResult(_) => {
                items.pop();
            }
            _ => break,
        }
    }

    // Add the btw user question.
    items.push(ConversationItem::user("btw what is X?"));

    let msg = build_messages_request(&ConversationRequest::from_items(items.clone()));
    let json = serde_json::to_value(&msg).unwrap();
    let messages = json.get("messages").unwrap().as_array().unwrap();

    // The last message before the btw question should be a plain
    // assistant text (not a tool_use), so the request is valid.
    // Messages: user("Fix the bug"), asst("I'll look"), asst(tool_use call_1),
    //           user(tool_result call_1), asst("I see the issue"),
    //           user("btw what is X?")
    // The orphaned call_2 assistant must be gone.
    let last_assistant = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .unwrap();
    if let Some(content) = last_assistant.get("content").and_then(|c| c.as_array()) {
        for block in content {
            assert_ne!(
                block.get("type").and_then(|t| t.as_str()),
                Some("tool_use"),
                "last assistant must not have unanswered tool_use blocks",
            );
        }
    }

    // Verify the original complete pair (call_1) survived.
    // system + user + asst_text + asst(call_1) + tool_result(call_1) + asst_text + user(btw) = 7
    assert_eq!(items.len(), 7);
}

#[test]
fn test_btw_cross_api_messages_no_regressions() {
    let items = btw_prepare_items(btw_mid_turn_conversation());
    let req = ConversationRequest::from_items(items);
    let msg = build_messages_request(&req);
    let json = serde_json::to_value(&msg).unwrap();

    let messages = json.get("messages").unwrap().as_array().unwrap();

    // No thinking blocks anywhere.
    for (i, m) in messages.iter().enumerate() {
        if let Some(content) = m.get("content").and_then(|c| c.as_array()) {
            for block in content {
                assert_ne!(
                    block.get("type").and_then(|t| t.as_str()),
                    Some("thinking"),
                    "messages[{i}] must not contain thinking blocks",
                );
            }
        }
    }

    // Last assistant message must not have unanswered tool_use.
    let last_assistant = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .expect("should have an assistant message");
    if let Some(content) = last_assistant.get("content").and_then(|c| c.as_array()) {
        for block in content {
            assert_ne!(
                block.get("type").and_then(|t| t.as_str()),
                Some("tool_use"),
                "last assistant in btw request must not have unanswered tool_use",
            );
        }
    }

    // Top-level thinking must be absent (no reasoning_effort set).
    assert!(
        json.get("thinking").is_none() || json.pointer("/thinking").is_some_and(|v| v.is_null()),
        "top-level thinking must be absent; got: {json:#}",
    );

    // Temperature must be absent (not hardcoded).
    assert!(
        json.get("temperature").is_none()
            || json.pointer("/temperature").is_some_and(|v| v.is_null()),
        "temperature must be absent so proxy defaults can apply; got: {json:#}",
    );

    // The completed tool pair (call_1) must survive.
    let has_tool_use_call_1 = messages.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|blocks| {
                blocks.iter().any(|b| {
                    b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                        && b.get("id").and_then(|id| id.as_str()) == Some("call_1")
                })
            })
    });
    assert!(
        has_tool_use_call_1,
        "completed tool_use call_1 must survive"
    );

    let has_tool_result_call_1 = messages.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|blocks| {
                blocks.iter().any(|b| {
                    b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                        && b.get("tool_use_id").and_then(|id| id.as_str()) == Some("call_1")
                })
            })
    });
    assert!(
        has_tool_result_call_1,
        "completed tool_result for call_1 must survive"
    );
}

#[test]
fn test_tool_result_with_images_to_anthropic() {
    let req = ConversationRequest::from_items(vec![
        ConversationItem::user("Read this"),
        ConversationItem::Assistant(AssistantItem {
            content: String::new().into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".to_string(),
                arguments: "{}".into(),
            }],
            model_id: None,
            model_fingerprint: None,
            reasoning_effort: None,
        }),
        ConversationItem::tool_result_with_images(
            "call_1",
            "Read image file: photo.png",
            vec![ContentPart::Image {
                url: "data:image/png;base64,iVBOR".into(),
            }],
        ),
    ]);

    let messages_req = build_messages_request(&req);

    // Find the user message that contains the tool result
    // (the Messages API wraps tool results in user messages)
    let tool_result_msg = messages_req
        .messages
        .iter()
        .find(|m| {
            if let crate::messages::MessageContent::Blocks(blocks) = &m.content {
                blocks
                    .iter()
                    .any(|b| matches!(b, crate::messages::ContentBlock::ToolResult { .. }))
            } else {
                false
            }
        })
        .expect("Expected a message with ToolResult block");

    let crate::messages::MessageContent::Blocks(blocks) = &tool_result_msg.content else {
        panic!("Expected Blocks");
    };
    let tool_result_block = blocks
        .iter()
        .find_map(|b| {
            if let crate::messages::ContentBlock::ToolResult { content, .. } = b {
                Some(content)
            } else {
                None
            }
        })
        .unwrap();

    // Should be Blocks variant with text + image, not Text
    let crate::messages::ToolResultContent::Blocks(inner) = tool_result_block else {
        panic!("Expected ToolResultContent::Blocks, got Text");
    };
    assert_eq!(inner.len(), 2);
    assert!(
        matches!(&inner[0], crate::messages::ContentBlock::Text { text, .. } if text == "Read image file: photo.png")
    );
    assert!(
        matches!(&inner[1], crate::messages::ContentBlock::Image { source: crate::messages::ImageSource::Base64 { media_type, data }, .. } if media_type == "image/png" && data == "iVBOR")
    );
}

#[test]
fn upgrade_legacy_reasoning_singular_anthropic_no_id() {
    // Messages streaming sets id = "" (see stream/messages.rs:340).
    // The upgrader must still emit a sibling carrying text + signature.
    let raw = serde_json::json!({
        "type": "assistant",
        "content": "answer",
        "reasoning": {
            "text": "Let me think about this...",
            "encrypted": "signature-bytes-here",
            "id": ""
        },
        "model_id": "messages-compatible-model"
    });
    let mut seen = std::collections::HashSet::new();
    let siblings = upgrade_legacy_reasoning(&raw, &mut seen);
    assert_eq!(siblings.len(), 1);
    let ConversationItem::Reasoning(r) = &siblings[0] else {
        panic!("expected Reasoning sibling");
    };
    assert_eq!(r.id, "");
    assert_eq!(r.encrypted_content.as_deref(), Some("signature-bytes-here"));
}

/// Conversation items for one completed turn: a reasoning sibling and the
/// assistant that `model` produced, then a follow-up prompt.
fn switched_model_conversation(model: Option<&str>) -> Vec<ConversationItem> {
    vec![
        ConversationItem::user("q1"),
        reasoning_sibling(
            "r1",
            "weighing the options",
            Some("sig-from-the-first-model"),
        ),
        ConversationItem::Assistant(AssistantItem {
            content: "The answer.".into(),
            tool_calls: vec![],
            model_id: model.map(str::to_owned),
            model_fingerprint: None,
            reasoning_effort: None,
        }),
        ConversationItem::user("q2"),
    ]
}

fn thinking_blocks(req: &ConversationRequest) -> Vec<(String, String)> {
    build_messages_request(req)
        .messages
        .iter()
        .flat_map(|m| match &m.content {
            crate::messages::MessageContent::Blocks(blocks) => blocks.clone(),
            crate::messages::MessageContent::Text(_) => Vec::new(),
        })
        .filter_map(|b| match b {
            crate::messages::ContentBlock::Thinking {
                thinking,
                signature,
            } => Some((thinking, signature)),
            _ => None,
        })
        .collect()
}

/// Switching an in-flight conversation onto another model used to resend that
/// conversation's thinking blocks, which the Messages API answers with
/// "Invalid `signature` in `thinking` block" — a 400 on every later turn, since
/// the blocks stay in history. The signature cannot be re-minted, so the block
/// is what gives; the rest of the turn stays.
#[test]
fn thinking_minted_by_another_model_is_left_off_the_wire() {
    let req = ConversationRequest::from_items(switched_model_conversation(Some("grok-4-fast")))
        .with_model("claude-opus-5");

    assert!(
        thinking_blocks(&req).is_empty(),
        "a thinking block from grok-4-fast must not be replayed to claude-opus-5",
    );

    let json = serde_json::to_value(build_messages_request(&req)).unwrap();
    assert_eq!(
        json["messages"][1]["content"][0]["text"], "The answer.",
        "only the thinking block goes, not the turn it belongs to: {json:#}",
    );
}

/// The model answers as the dated snapshot its alias resolves to, and a
/// gateway prefixes its own routing namespace. Neither is a model switch, so
/// both must keep replaying their signatures.
#[test]
fn thinking_survives_a_snapshot_date_and_a_gateway_prefix() {
    for (origin, target) in [
        ("claude-opus-5-20260101", "claude-opus-5"),
        ("claude-opus-5", "claude-opus-5-20260101"),
        ("claude-opus-5", "anthropic/claude-opus-5"),
        ("claude-opus-5", "claude-opus-5-latest"),
    ] {
        let req = ConversationRequest::from_items(switched_model_conversation(Some(origin)))
            .with_model(target);
        assert_eq!(
            thinking_blocks(&req),
            vec![(
                "weighing the options".to_string(),
                "sig-from-the-first-model".to_string()
            )],
            "{origin} -> {target} is the same model; its thinking must be replayed",
        );
    }
}

/// A near-miss must not read as the same lineage: the shared prefix is not a
/// snapshot date, so the signature is another model's.
#[test]
fn thinking_from_a_sibling_model_is_left_off_the_wire() {
    let req = ConversationRequest::from_items(switched_model_conversation(Some("claude-opus-5")))
        .with_model("claude-opus-5-mini");
    assert!(
        thinking_blocks(&req).is_empty(),
        "claude-opus-5-mini is not claude-opus-5",
    );
}

/// History that never recorded which model produced a turn (`model_id` absent)
/// is replayed as before. Dropping it would strip thinking from every
/// same-model session whose items were synthesized rather than streamed; the
/// sampler's strip-and-retry is what covers a server that rejects it.
#[test]
fn thinking_without_a_recorded_origin_is_replayed() {
    let req = ConversationRequest::from_items(switched_model_conversation(None))
        .with_model("claude-opus-5");
    assert_eq!(thinking_blocks(&req).len(), 1);
}

/// The recovery the sampler applies when the server rejects a signature
/// anyway: the reasoning goes, the turn it belongs to stays.
#[test]
fn strip_reasoning_drops_only_the_reasoning_siblings() {
    let mut req = ConversationRequest::from_items(switched_model_conversation(Some("grok-4-fast")))
        .with_model("claude-opus-5");

    assert_eq!(req.strip_reasoning(), 1);
    assert_eq!(req.strip_reasoning(), 0, "nothing left to strip");
    assert!(thinking_blocks(&req).is_empty());
    assert_eq!(
        req.items.len(),
        3,
        "user, assistant and follow-up survive: {:?}",
        req.items
    );
}

/// Switching models mid-tool-loop: the turn the provider is being asked to
/// continue made tool calls, and its thinking went with the switch. A provider
/// validates that turn's thinking — thinking-on requires it to lead with one —
/// and it cannot be re-minted, so the request goes out with thinking off
/// rather than trading one 400 for another. The tool loop itself survives.
#[test]
fn a_tool_loop_that_lost_its_thinking_turns_thinking_off() {
	let mut req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "planning the call", Some("sig-from-the-first-model")),
		ConversationItem::Assistant(AssistantItem {
			content: String::new().into(),
			tool_calls: vec![ToolCall {
				id: "call_1".into(),
				name: "read_file".to_string(),
				arguments: r#"{"path":"src/main.rs"}"#.into(),
			}],
			model_id: Some("grok-4-fast".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::tool_result("call_1", "fn main() {}"),
	])
	.with_model("claude-opus-5");
	req.reasoning_effort = Some(crate::ReasoningEffort::High);

	let msgs = build_messages_request(&req);
	assert!(
		msgs.thinking.is_none(),
		"a tool loop the model cannot lead with a thinking block must go out with thinking off",
	);
	assert!(thinking_blocks(&req).is_empty());

	let json = serde_json::to_value(&msgs).unwrap();
	assert_eq!(
		json["messages"][1]["content"][0]["type"], "tool_use",
		"the tool call still has to reach the model: {json:#}",
	);
	assert_eq!(
		json["messages"][2]["content"][0]["type"], "tool_result",
		"and so does its result: {json:#}",
	);
	assert_eq!(
		json.pointer("/output_config/effort").and_then(|v| v.as_str()),
		Some("high"),
		"the caller's effort is untouched; only the thinking pairing stands down: {json:#}",
	);
}

/// The control: the same open tool loop, same model. Nothing was lost, so
/// thinking stays on and the block is replayed.
#[test]
fn a_tool_loop_that_kept_its_thinking_keeps_thinking_on() {
	let mut req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "planning the call", Some("sig-from-this-model")),
		ConversationItem::Assistant(AssistantItem {
			content: String::new().into(),
			tool_calls: vec![ToolCall {
				id: "call_1".into(),
				name: "read_file".to_string(),
				arguments: r#"{"path":"src/main.rs"}"#.into(),
			}],
			model_id: Some("claude-opus-5-20260101".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::tool_result("call_1", "fn main() {}"),
	])
	.with_model("claude-opus-5");
	req.reasoning_effort = Some(crate::ReasoningEffort::High);

	assert!(build_messages_request(&req).thinking.is_some());
	assert_eq!(thinking_blocks(&req).len(), 1);
}

/// A closed loop — its results answered and the user back with a follow-up —
/// is not the turn the model is being asked to continue, so only the foreign
/// thinking goes; thinking itself stays on for the new turn.
#[test]
fn a_closed_tool_loop_leaves_thinking_on() {
	let mut req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "planning the call", Some("sig-from-the-first-model")),
		ConversationItem::Assistant(AssistantItem {
			content: String::new().into(),
			tool_calls: vec![ToolCall {
				id: "call_1".into(),
				name: "read_file".to_string(),
				arguments: r#"{"path":"src/main.rs"}"#.into(),
			}],
			model_id: Some("grok-4-fast".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::tool_result("call_1", "fn main() {}"),
		ConversationItem::user("q2"),
	])
	.with_model("claude-opus-5");
	req.reasoning_effort = Some(crate::ReasoningEffort::High);

	assert!(build_messages_request(&req).thinking.is_some());
	assert!(thinking_blocks(&req).is_empty());
}

/// The Messages API is the one backend that rejects thinking blocks it was not
/// configured for, so the capture loop strips reasoning from its own turns
/// there too — and the tool_use / tool_result pair it built by hand still maps.
#[test]
fn todo_capture_loop_strips_reasoning_and_keeps_the_tool_pair() {
    let request = build_messages_request(
        &ConversationRequest::from_items(todo_capture_loop_items(true))
            .with_model("messages-compatible-model"),
    );
    let json = serde_json::to_value(&request).unwrap();
    let blocks: Vec<&serde_json::Value> = json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .collect();

    assert!(
        !blocks
            .iter()
            .any(|b| b["type"] == "thinking" || b["type"] == "redacted_thinking"),
        "no thinking blocks may reach the Messages API here: {json:#}"
    );
    assert!(
        blocks
            .iter()
            .any(|b| b["type"] == "tool_use" && b["id"] == "call_todo_1"),
        "the loop's own tool_use must survive: {json:#}"
    );
    assert!(
        blocks
            .iter()
            .any(|b| b["type"] == "tool_result" && b["tool_use_id"] == "call_todo_1"),
        "the tool result the loop fed back must survive: {json:#}"
    );
}

/// A switch between two models that reason in plain text keeps the thinking.
/// Only a signature is model-bound, and there is none on either side here, so
/// dropping the block would throw away context nothing was going to reject.
#[test]
fn unsigned_thinking_rides_a_switch_between_two_models_that_do_not_sign() {
	let req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "weighing the options", None),
		ConversationItem::Assistant(AssistantItem {
			content: "The answer.".into(),
			tool_calls: vec![],
			model_id: Some("grok-4-fast".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::user("q2"),
	])
	.with_model("grok-code-fast");

	assert_eq!(
		thinking_blocks(&req),
		vec![("weighing the options".to_string(), String::new())],
		"unsigned thinking has nothing to verify, so a switch must replay it",
	);
}

/// The other unsigned case: the model being called does sign its thinking, and
/// it rejects a block that arrives without a signature just as hard as one
/// signed by somebody else. What says so is the conversation itself — this
/// model already signed a block earlier in it.
#[test]
fn unsigned_thinking_is_dropped_at_a_model_that_signs_its_own() {
	let req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "the signing model's own thinking", Some("sig-1")),
		ConversationItem::Assistant(AssistantItem {
			content: "First answer.".into(),
			tool_calls: vec![],
			model_id: Some("claude-opus-5".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::user("q2"),
		reasoning_sibling("r2", "the other model's thinking", None),
		ConversationItem::Assistant(AssistantItem {
			content: "Second answer.".into(),
			tool_calls: vec![],
			model_id: Some("grok-4-fast".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::user("q3"),
	])
	.with_model("claude-opus-5");

	assert_eq!(
		thinking_blocks(&req),
		vec![(
			"the signing model's own thinking".to_string(),
			"sig-1".to_string()
		)],
		"the signing model keeps its own block and must not be handed an unsigned one",
	);
}

/// Mid-tool-loop, the same way round: the turn being continued kept its
/// thinking across the switch, so there is a block to lead with and thinking
/// stays on. Turning it off here would cost the loop its reasoning for no 400
/// that was ever going to happen.
#[test]
fn a_tool_loop_that_kept_its_unsigned_thinking_keeps_thinking_on() {
	let mut req = ConversationRequest::from_items(vec![
		ConversationItem::user("q1"),
		reasoning_sibling("r1", "planning the call", None),
		ConversationItem::Assistant(AssistantItem {
			content: String::new().into(),
			tool_calls: vec![ToolCall {
				id: "call_1".into(),
				name: "read_file".to_string(),
				arguments: r#"{"path":"src/main.rs"}"#.into(),
			}],
			model_id: Some("grok-4-fast".into()),
			model_fingerprint: None,
			reasoning_effort: None,
		}),
		ConversationItem::tool_result("call_1", "fn main() {}"),
	])
	.with_model("grok-code-fast");
	req.reasoning_effort = Some(crate::ReasoningEffort::High);

	let msgs = build_messages_request(&req);
	assert!(
		msgs.thinking.is_some(),
		"the loop still leads with a thinking block, so thinking stays on",
	);
	assert_eq!(
		thinking_blocks(&req),
		vec![("planning the call".to_string(), String::new())],
	);
}

fn legacy_dialect_request(model: &str) -> ConversationRequest {
    let mut req = ConversationRequest::from_items(vec![ConversationItem::user("go")])
        .with_model(model)
        .with_max_output_tokens(32_000);
    req.reasoning_effort = Some(crate::ReasoningEffort::High);
    req
}

/// The 400 this rule exists for: "Input tag 'adaptive' found using 'type' does
/// not match any of the expected tags: 'disabled', 'enabled'". A pre-4.6 Claude
/// sizes its thinking in tokens, and rejects the effort word alongside it.
#[test]
fn a_pre_4_6_claude_gets_a_token_budget_instead_of_adaptive_thinking() {
    let msgs = build_messages_request(&legacy_dialect_request("claude-haiku-4-5"));

    assert!(
        matches!(
            msgs.thinking,
            Some(crate::messages::ThinkingConfig::Enabled {
                budget_tokens: 16_384
            })
        ),
        "expected an enabled/budget_tokens thinking config, got {:?}",
        msgs.thinking,
    );
    assert!(
        msgs.output_config.is_none(),
        "output_config.effort is 4.6-and-later too, so nothing is left to send: {:?}",
        msgs.output_config,
    );
}

#[test]
fn a_4_6_model_keeps_adaptive_thinking_and_the_effort_word() {
    for model in ["claude-opus-4-6", "claude-sonnet-5", "claude-opus-5"] {
        let msgs = build_messages_request(&legacy_dialect_request(model));

        assert!(
            matches!(
                msgs.thinking,
                Some(crate::messages::ThinkingConfig::Adaptive { .. })
            ),
            "{model} speaks the adaptive dialect, got {:?}",
            msgs.thinking,
        );
        assert_eq!(
            msgs.output_config.and_then(|oc| oc.effort).as_deref(),
            Some("high"),
            "{model} takes the effort word",
        );
    }
}

/// Structured outputs are not what 4.6 changed, so the legacy dialect keeps
/// `output_config.format` while losing only the effort beside it.
#[test]
fn structured_output_survives_the_legacy_thinking_dialect() {
    let schema = serde_json::json!({ "type": "object" });
    let req = legacy_dialect_request("claude-haiku-4-5").with_json_schema(schema);

    let oc = build_messages_request(&req)
        .output_config
        .expect("format still goes out");
    assert!(oc.format.is_some());
    assert_eq!(oc.effort, None);
}

/// The budget has to clear the API's 1024 floor and stay under `max_tokens`.
/// A ceiling that cannot house both leaves thinking off rather than sending a
/// budget the API rejects.
#[test]
fn a_budget_that_cannot_clear_the_api_floor_turns_thinking_off() {
    let mut req = legacy_dialect_request("claude-haiku-4-5");
    req.max_output_tokens = Some(900);
    assert!(build_messages_request(&req).thinking.is_none());

    req.max_output_tokens = Some(2_000);
    assert!(
        matches!(
            build_messages_request(&req).thinking,
            Some(crate::messages::ThinkingConfig::Enabled {
                budget_tokens: 1_999
            })
        ),
        "the budget gives way to max_tokens, not the other way round",
    );
}

#[test]
fn the_thinking_dialect_is_read_off_every_spelling_of_a_model_id() {
    let speaks_adaptive = |model: &str| {
        matches!(
            build_messages_request(&legacy_dialect_request(model)).thinking,
            Some(crate::messages::ThinkingConfig::Adaptive { .. })
        )
    };

    for adaptive in [
        "claude-opus-4-6",
        "claude-opus-4-6-20260122",
        "anthropic/claude-sonnet-5",
        "claude-fable-5",
        "claude-opus-4-8",
        // Not a Claude at all: a gateway's own model keeps the request it has
        // always been sent.
        "grok-4-fast",
        "gemini-3-pro",
    ] {
        assert!(speaks_adaptive(adaptive), "{adaptive}");
    }

    for legacy in [
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "claude-sonnet-4-5",
        "claude-opus-4-5-20250929",
        "claude-opus-4-20250514",
        "claude-3-7-sonnet-20250219",
        "us.anthropic.claude-haiku-4-5-v1:0",
        "claude-haiku-4-5@20251001",
    ] {
        assert!(!speaks_adaptive(legacy), "{legacy}");
    }
}
