use super::test_support::*;
use super::*;
use assert_matches::assert_matches;

fn make_test_tool() -> ToolSpec {
    ToolSpec {
        name: "test_tool".to_string(),
        description: Some("A test tool".to_string()),
        parameters: serde_json::json!({}),
    }
}

#[test]
fn test_conversation_item_roundtrip() {
    let system = ConversationItem::system("You are a helpful assistant.");
    let chat_msg = conversation_item_to_chat_message(system.clone());
    let back: ConversationItem = chat_msg.into();
    assert_eq!(back.text_content(), "You are a helpful assistant.");

    let user = ConversationItem::user("Hello!");
    let chat_msg = conversation_item_to_chat_message(user);
    let back: ConversationItem = chat_msg.into();
    assert_eq!(back.text_content(), "Hello!");

    // Reasoning is a sibling item, so the single-item conversion leaves reasoning_content None
    // The `conversation_to_chat_messages` helper carries reasoning through and is tested separately
    let assistant = ConversationItem::assistant_with_model("Hi there!", "grok-3");
    let chat_msg = conversation_item_to_chat_message(assistant);
    assert_eq!(chat_msg.reasoning_content, None);
    let back: ConversationItem = chat_msg.into();
    assert_eq!(back.text_content(), "Hi there!");

    let tool_result = ConversationItem::tool_result("call_123", "Result data");
    let chat_msg = conversation_item_to_chat_message(tool_result);
    assert_eq!(chat_msg.tool_call_id, Some("call_123".to_string()));
}

/// `reasoning_content` is unverified text, so `Native` and `TextOnly` send the
/// same body. `Scrubbed` is the one level that changes it: the fold has
/// nothing to fold.
#[test]
fn the_replay_level_only_scrubs_a_chat_completions_request() {
    let items = || {
        vec![
            ConversationItem::user("q1"),
            reasoning_sibling("r1", "carry the one", Some("enc-blob-from-origin")),
            ConversationItem::assistant_with_model("The answer.", "grok-4"),
            ConversationItem::user("q2"),
        ]
    };
    let reasoning_content = |req: ConversationRequest| -> Option<String> {
        let chat: ChatCompletionRequest = req.into();
        chat.messages
            .into_iter()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| m.reasoning_content)
    };

    let native = ConversationRequest::from_items(items()).with_model("grok-3-mini");
    assert_eq!(
        reasoning_content(native).as_deref(),
        Some("carry the one"),
        "another model's words still ride as reasoning_content"
    );

    let mut text_only = ConversationRequest::from_items(items()).with_model("grok-3-mini");
    assert!(text_only.degrade_thinking_replay());
    assert_eq!(
        reasoning_content(text_only).as_deref(),
        Some("carry the one")
    );

    let mut scrubbed = ConversationRequest::from_items(items()).with_model("grok-3-mini");
    assert!(scrubbed.degrade_thinking_replay());
    assert!(scrubbed.degrade_thinking_replay());
    assert_eq!(
        reasoning_content(scrubbed),
        None,
        "scrubbed sends no reasoning"
    );
}

#[test]
fn test_conversation_request_to_chat_completion() {
    let req = ConversationRequest::from_items(vec![
        ConversationItem::system("System prompt"),
        ConversationItem::user("User message"),
    ])
    .with_model("grok-3")
    .with_temperature(0.7);

    let chat_req: ChatCompletionRequest = req.into();
    assert_eq!(chat_req.model, Some("grok-3".to_string()));
    assert_eq!(chat_req.temperature, Some(0.7));
    assert_eq!(chat_req.messages.len(), 2);
}

#[test]
fn test_user_with_image() {
    let mut user = ConversationItem::user("Check this image");
    user.add_image("https://example.com/image.png");

    let ConversationItem::User(u) = &user else {
        panic!("Expected User item");
    };
    assert_eq!(u.content.len(), 2);
    assert_matches!(
        u.content.get(1),
        Some(ContentPart::Image { url }) if url.as_ref() == "https://example.com/image.png"
    );

    // Convert to chat request and verify
    let chat_msg = conversation_item_to_chat_message(user);
    let blocks = chat_msg.content.blocks();
    assert_eq!(blocks.len(), 2);
    assert_matches!(
        blocks.get(1),
        Some(ChatContentBlock::ImageUrl { image_url }) if image_url.url == "https://example.com/image.png"
    );
}

#[test]
fn test_chat_response_message_to_conversation_item() {
    use crate::types::{ChatResponseMessage, Role, ToolCallFunction, ToolCallResponse};

    // Simple text response
    let response_msg = ChatResponseMessage {
        role: Role::Assistant,
        content: Some("Hello, world!".to_string()),
        reasoning_content: None,
        tool_calls: vec![],
        tool_call_id: None,
        citations: None,
    };

    let item: ConversationItem = response_msg.into();
    assert_eq!(item.text_content(), "Hello, world!");
    assert_eq!(item.role(), Role::Assistant);

    // Response with reasoning
    let response_with_reasoning = ChatResponseMessage {
        role: Role::Assistant,
        content: Some("The answer is 42.".to_string()),
        reasoning_content: Some("Let me think step by step...".to_string()),
        tool_calls: vec![],
        tool_call_id: None,
        citations: None,
    };

    let item: ConversationItem = response_with_reasoning.into();
    let ConversationItem::Assistant(a) = &item else {
        panic!("Expected Assistant item");
    };
    assert_eq!(a.content.as_ref(), "The answer is 42.");
    // Reasoning content is dropped on the single-item `From` path; see the doc comment on `From<ChatResponseMessage>`

    // Response with tool calls
    let response_with_tools = ChatResponseMessage {
        role: Role::Assistant,
        content: None,
        reasoning_content: None,
        tool_calls: vec![ToolCallResponse {
            id: "call_123".to_string(),
            kind: "function".to_string(),
            function: ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path": "/foo.txt"}"#.to_string(),
            },
            vendor: Default::default(),
        }],
        tool_call_id: None,
        citations: None,
    };

    let item: ConversationItem = response_with_tools.into();
    let ConversationItem::Assistant(a) = &item else {
        panic!("Expected Assistant item");
    };
    let [tc] = a.tool_calls.as_slice() else {
        panic!("expected one tool call: {:?}", a.tool_calls);
    };
    assert_eq!(tc.id.as_ref(), "call_123");
    assert_eq!(tc.name, "read_file");
}

#[test]
fn test_tool_calls_roundtrip_to_chat_request() {
    let tool_call = ToolCall {
        id: "call_abc123".into(),
        name: "read_file".to_string(),
        arguments: r#"{"path": "/foo.txt", "limit": 100}"#.into(),
        vendor: Default::default(),
    };

    let item = ConversationItem::assistant_tool_calls(vec![tool_call.clone()]);

    let chat_msg = conversation_item_to_chat_message(item.clone());
    let [ctc] = chat_msg.tool_calls.as_slice() else {
        panic!("expected one chat tool call: {:?}", chat_msg.tool_calls);
    };
    assert_eq!(ctc.id, Some("call_abc123".to_string()));
    assert_eq!(ctc.function.name, "read_file");
    assert_eq!(
        ctc.function.arguments,
        r#"{"path": "/foo.txt", "limit": 100}"#
    );

    let back: ConversationItem = chat_msg.into();
    let ConversationItem::Assistant(a) = back else {
        panic!("Expected Assistant item");
    };
    let [tc] = a.tool_calls.as_slice() else {
        panic!("expected one tool call: {:?}", a.tool_calls);
    };
    assert_eq!(tc.id.as_ref(), "call_abc123");
    assert_eq!(tc.name, "read_file");
    assert_eq!(
        tc.arguments.as_ref(),
        r#"{"path": "/foo.txt", "limit": 100}"#
    );
}

#[test]
fn test_multiple_tool_calls_roundtrip() {
    let tool_calls = vec![
        ToolCall {
            id: "call_1".into(),
            name: "read_file".to_string(),
            arguments: r#"{"path": "/a.txt"}"#.into(),
            vendor: Default::default(),
        },
        ToolCall {
            id: "call_2".into(),
            name: "bash".to_string(),
            arguments: r#"{"command": "ls -la"}"#.into(),
            vendor: Default::default(),
        },
        ToolCall {
            id: "call_3".into(),
            name: "grep".to_string(),
            arguments: r#"{"pattern": "TODO", "path": "."}"#.into(),
            vendor: Default::default(),
        },
    ];

    let item = ConversationItem::assistant_tool_calls(tool_calls);

    let chat_msg = conversation_item_to_chat_message(item);
    let [c0, c1, c2] = chat_msg.tool_calls.as_slice() else {
        panic!("expected three chat tool calls: {:?}", chat_msg.tool_calls);
    };
    assert_eq!(c0.function.name, "read_file");
    assert_eq!(c1.function.name, "bash");
    assert_eq!(c2.function.name, "grep");

    // Back to ConversationItem
    let back: ConversationItem = chat_msg.into();
    let ConversationItem::Assistant(a) = back else {
        panic!("Expected Assistant item");
    };
    let [t0, t1, t2] = a.tool_calls.as_slice() else {
        panic!("expected three tool calls: {:?}", a.tool_calls);
    };
    assert_eq!(t0.name, "read_file");
    assert_eq!(t1.name, "bash");
    assert_eq!(t2.name, "grep");
}

#[test]
fn test_assistant_with_content_and_tool_calls() {
    // Assistant can have both text content and tool calls
    let assistant = AssistantItem {
        content: "Let me help you with that.".into(),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "read_file".to_string(),
            arguments: r#"{"path": "/test.txt"}"#.into(),
            vendor: Default::default(),
        }],
        model_id: Some("grok-3".to_string()),
        model_fingerprint: None,
        reasoning_effort: None,
    };

    let item = ConversationItem::Assistant(assistant.clone());
    let chat_msg = conversation_item_to_chat_message(item);

    assert_eq!(chat_msg.text_content(), "Let me help you with that.");
    assert_eq!(chat_msg.tool_calls.len(), 1);
    assert_eq!(chat_msg.model_id, Some("grok-3".to_string()));
}

#[test]
fn test_conversation_request_with_tools_to_chat_completion() {
    let tools = vec![
        ToolSpec {
            name: "read_file".to_string(),
            description: Some("Read a file from disk".to_string()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "bash".to_string(),
            description: Some("Run a bash command".to_string()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        },
    ];

    let req =
        ConversationRequest::from_items(vec![ConversationItem::user("Help me")]).with_tools(tools);

    let chat_req: ChatCompletionRequest = req.into();
    assert!(chat_req.tools.is_some());
    let tools = chat_req.tools.unwrap();
    let [t0, t1] = tools.as_slice() else {
        panic!("expected two tools: {tools:?}");
    };
    assert_eq!(t0.function.name, "read_file");
    assert_eq!(t1.function.name, "bash");
}

#[test]
fn tool_choice_presets_map_to_wire_strings() {
    for (choice, wire) in [
        (ConversationToolChoice::Auto, "auto"),
        (ConversationToolChoice::None, "none"),
        (ConversationToolChoice::Required, "required"),
    ] {
        let req = ConversationRequest::from_items(vec![ConversationItem::user("test")])
            .with_tools(vec![make_test_tool()])
            .with_tool_choice(choice);

        let chat_req: ChatCompletionRequest = req.into();
        let Some(ToolChoice::Preset(preset)) = chat_req.tool_choice else {
            panic!("expected a preset tool choice for {wire}");
        };
        assert_eq!(preset, wire);
    }
}

#[test]
fn test_tool_choice_function_to_chat_completion() {
    let req = ConversationRequest::from_items(vec![ConversationItem::user("test")])
        .with_tools(vec![make_test_tool()])
        .with_tool_choice(ConversationToolChoice::Function("read_file".to_string()));

    let chat_req: ChatCompletionRequest = req.into();
    let ToolChoice::Function { function, .. } = chat_req.tool_choice.unwrap() else {
        panic!("Expected Function tool choice");
    };
    assert_eq!(function.name, "read_file");
}

#[test]
fn test_tool_choice_dropped_when_no_tools_chat_completions() {
    // Chat Completions API rejects tool_choice without tools
    let req = ConversationRequest::from_items(vec![ConversationItem::user("test")])
        .with_tool_choice(ConversationToolChoice::Auto);
    let chat_req: ChatCompletionRequest = req.into();
    assert!(chat_req.tool_choice.is_none());
    assert!(chat_req.tools.is_none());
}

#[test]
fn test_user_with_multiple_images() {
    let parts = vec![
        ContentPart::Text {
            text: "Compare these images:".into(),
        },
        ContentPart::Image {
            url: "https://example.com/img1.png".into(),
        },
        ContentPart::Image {
            url: "https://example.com/img2.png".into(),
        },
        ContentPart::Image {
            url: "data:image/png;base64,iVBORw0KGgo=".into(),
        },
    ];

    let user = ConversationItem::user_with_parts(parts);

    let chat_msg = conversation_item_to_chat_message(user);
    let blocks = chat_msg.content.blocks();
    let [b0, b1, b2, b3] = blocks.as_slice() else {
        panic!("expected four blocks: {blocks:?}");
    };
    assert_matches!(b0, ChatContentBlock::Text { text } if text == "Compare these images:");
    assert_matches!(b1, ChatContentBlock::ImageUrl { .. });
    assert_matches!(b2, ChatContentBlock::ImageUrl { .. });
    assert_matches!(b3, ChatContentBlock::ImageUrl { .. });
}

#[test]
fn test_malformed_tool_arguments_sanitized_to_empty_object_in_chat_request() {
    // Exactly the broken string from the real incident: the missing `"` before `new_string` makes the JSON parse fail at char 80
    let bad_args = r#"{"file_path": "/testbed/cxx_polynomial/include/emsr/remez.h", "old_string": "", new_string": "x"}"#;
    assert!(
        serde_json::from_str::<serde_json::Value>(bad_args).is_err(),
        "pre-condition: bad_args must be invalid JSON"
    );

    let tool_call = ToolCall {
        id: "functions.search_replace:10".into(),
        name: "search_replace".to_string(),
        arguments: bad_args.into(),
        vendor: Default::default(),
    };

    let item = ConversationItem::assistant_tool_calls(vec![tool_call]);
    let chat_msg = conversation_item_to_chat_message(item);

    let Some(sanitized) = chat_msg.tool_calls.first().map(|c| &c.function.arguments) else {
        panic!("expected a tool call: {:?}", chat_msg.tool_calls);
    };
    assert_eq!(
        sanitized, "{}",
        "malformed arguments must be replaced with {{}}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(sanitized).is_ok(),
        "sanitized arguments must be valid JSON"
    );
}

#[test]
fn test_valid_tool_arguments_pass_through_unchanged_in_chat_request() {
    let valid_args = r#"{"file_path": "/foo.rs", "old_string": "a", "new_string": "b"}"#;
    let tool_call = ToolCall {
        id: "call_1".into(),
        name: "search_replace".to_string(),
        arguments: valid_args.into(),
        vendor: Default::default(),
    };

    let item = ConversationItem::assistant_tool_calls(vec![tool_call]);
    let chat_msg = conversation_item_to_chat_message(item);
    assert_eq!(
        chat_msg
            .tool_calls
            .first()
            .map(|c| c.function.arguments.as_str()),
        Some(valid_args),
        "valid arguments must not be modified"
    );
}

#[test]
fn test_chat_completion_request_carries_reasoning_effort_top_level() {
    for (variant, expected) in [
        (crate::ReasoningEffort::None, "none"),
        (crate::ReasoningEffort::Minimal, "minimal"),
        (crate::ReasoningEffort::Low, "low"),
        (crate::ReasoningEffort::Medium, "medium"),
        (crate::ReasoningEffort::High, "high"),
        (crate::ReasoningEffort::Xhigh, "xhigh"),
        (crate::ReasoningEffort::Max, "max"),
    ] {
        let req =
            ConversationRequest::from_items(vec![ConversationItem::user("hi")]).with_model("test");
        let req = ConversationRequest {
            reasoning_effort: Some(variant),
            ..req
        };
        let chat: ChatCompletionRequest = req.into();
        let json = serde_json::to_value(&chat).unwrap();
        assert_eq!(
            json.pointer("/reasoning_effort").and_then(|v| v.as_str()),
            Some(expected),
            "{variant:?} should serialize as top-level reasoning_effort={expected:?}; got: {json:#}",
        );
    }
}

#[test]
fn test_chat_completion_request_omits_reasoning_effort_when_unset() {
    let req =
        ConversationRequest::from_items(vec![ConversationItem::user("hi")]).with_model("test");
    let chat: ChatCompletionRequest = req.into();
    let json = serde_json::to_value(&chat).unwrap();
    assert!(
        json.get("reasoning_effort").is_none(),
        "reasoning_effort must be absent when unset; got: {json:#}",
    );
}

#[test]
fn wire_reasoning_effort_remaps_only_mandatory_disabled_tiers() {
    use crate::{ReasoningEffort, wire_reasoning_effort};
    // For a mandatory target, unset/None/Minimal all lift to the lowest
    // enabled tier; supported efforts pass through unchanged.
    assert_eq!(
        wire_reasoning_effort(true, None),
        Some(ReasoningEffort::Low),
    );
    assert_eq!(
        wire_reasoning_effort(true, Some(ReasoningEffort::None)),
        Some(ReasoningEffort::Low),
    );
    assert_eq!(
        wire_reasoning_effort(true, Some(ReasoningEffort::Minimal)),
        Some(ReasoningEffort::Low),
    );
    for supported in [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Xhigh,
        ReasoningEffort::Max,
    ] {
        assert_eq!(
            wire_reasoning_effort(true, Some(supported)),
            Some(supported)
        );
    }
    // A non-mandatory target is a strict no-op, including the disabled tiers.
    assert_eq!(wire_reasoning_effort(false, None), None);
    assert_eq!(
        wire_reasoning_effort(false, Some(ReasoningEffort::None)),
        Some(ReasoningEffort::None),
    );
    assert_eq!(
        wire_reasoning_effort(false, Some(ReasoningEffort::Minimal)),
        Some(ReasoningEffort::Minimal),
    );
    assert_eq!(
        wire_reasoning_effort(false, Some(ReasoningEffort::High)),
        Some(ReasoningEffort::High),
    );
}

/// A reasoning-mandatory target must never be sent a chat-completions body
/// that disables reasoning: any requested effort resolving to `None` (or to a
/// `None`/`Minimal`/unset request) is remapped to the lowest enabled tier
/// (`low`) on the wire, and the disabled `none` signal is never present.
#[test]
fn mandatory_target_never_disables_reasoning_on_chat_completions_wire() {
    // These requests all resolve to a disabled/omitted effort and must be
    // remapped for a mandatory target.
    for requested in [
        Some(crate::ReasoningEffort::None),
        Some(crate::ReasoningEffort::Minimal),
        None,
    ] {
        let req = ConversationRequest {
            reasoning_effort: requested,
            reasoning_mandatory: true,
            ..ConversationRequest::from_items(vec![ConversationItem::user("hi")]).with_model("test")
        };
        let chat: ChatCompletionRequest = req.into();
        let json = serde_json::to_value(&chat).unwrap();
        assert_eq!(
            json.pointer("/reasoning_effort").and_then(|v| v.as_str()),
            Some("low"),
            "requested {requested:?} on a reasoning-mandatory target must be remapped to \
             the lowest enabled effort; got: {json:#}",
        );
    }
}

/// A non-mandatory target is unchanged: requesting `None` serializes `none`,
/// and an unset effort stays absent — no behavior change for models that do
/// not mandate reasoning.
#[test]
fn non_mandatory_target_is_unchanged_on_chat_completions_wire() {
    let req = ConversationRequest {
        reasoning_effort: Some(crate::ReasoningEffort::None),
        reasoning_mandatory: false,
        ..ConversationRequest::from_items(vec![ConversationItem::user("hi")]).with_model("test")
    };
    let chat: ChatCompletionRequest = req.into();
    let json = serde_json::to_value(&chat).unwrap();
    assert_eq!(
        json.pointer("/reasoning_effort").and_then(|v| v.as_str()),
        Some("none"),
        "non-mandatory target requesting None must keep the none signal; got: {json:#}",
    );

    let unset = ConversationRequest {
        reasoning_mandatory: false,
        ..ConversationRequest::from_items(vec![ConversationItem::user("hi")]).with_model("test")
    };
    let chat: ChatCompletionRequest = unset.into();
    let json = serde_json::to_value(&chat).unwrap();
    assert!(
        json.get("reasoning_effort").is_none(),
        "non-mandatory target with unset effort must stay absent; got: {json:#}",
    );
}

#[test]
fn btw_cross_api_chat_completions_no_regressions() {
    let items = btw_prepare_items(btw_mid_turn_conversation());
    let req = ConversationRequest::from_items(items);
    let chat: ChatCompletionRequest = req.into();
    let json = serde_json::to_value(&chat).unwrap();

    let messages = json.get("messages").unwrap().as_array().unwrap();

    // Last assistant must not have orphaned tool_calls.
    let last_assistant = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .expect("should have an assistant message");
    let has_tool_calls = last_assistant
        .get("tool_calls")
        .and_then(|tc| tc.as_array())
        .is_some_and(|a| !a.is_empty());
    if has_tool_calls {
        let last_asst_idx = messages
            .iter()
            .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
            .unwrap();
        let has_following_tool = messages
            .get(last_asst_idx + 1..)
            .into_iter()
            .flatten()
            .any(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"));
        assert!(
            has_following_tool,
            "last assistant with tool_calls must have a following tool message"
        );
    }

    assert!(
        json.get("temperature").is_none()
            || json.pointer("/temperature").is_some_and(|v| v.is_null()),
        "temperature must be absent; got: {json:#}",
    );

    // The completed tool pair (call_1) must survive.
    let has_call_1 = messages.iter().any(|m| {
        m.get("tool_calls")
            .and_then(|tc| tc.as_array())
            .is_some_and(|calls| {
                calls
                    .iter()
                    .any(|c| c.get("id").and_then(|id| id.as_str()) == Some("call_1"))
            })
    });
    assert!(has_call_1, "completed tool_call call_1 must survive");

    let has_tool_result_1 = messages.iter().any(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("tool")
            && m.get("tool_call_id").and_then(|id| id.as_str()) == Some("call_1")
    });
    assert!(
        has_tool_result_1,
        "completed tool result for call_1 must survive"
    );
}

#[test]
fn test_sanitize_non_ascii_args_preview_does_not_panic() {
    // Build a string where the 200-byte boundary lands inside a CJK char.
    // Each '文' is 3 bytes, so 67 × 3 = 201 bytes; byte 200 is inside the 67th char
    let filler = "文".repeat(70);
    let bad_args = format!("{{\"old_string\": \"{filler}\"}}");
    // The outer JSON is valid but contains non-ASCII; force the warning path by making the JSON invalid
    let malformed = format!("{{\"old_string\": \"{filler}\" missing_key}}");

    let tool_call = ToolCall {
        id: "call_1".into(),
        name: "search_replace".to_string(),
        arguments: malformed.clone().into(),
        vendor: Default::default(),
    };
    // Must not panic.
    let item = ConversationItem::assistant_tool_calls(vec![tool_call]);
    let chat_msg = conversation_item_to_chat_message(item);
    assert_eq!(
        chat_msg
            .tool_calls
            .first()
            .map(|c| c.function.arguments.as_str()),
        Some("{}"),
        "malformed non-ASCII arguments must be sanitized to {{}}"
    );
    // Also confirm valid non-ASCII passes through unchanged.
    let tool_call_valid = ToolCall {
        id: "call_2".into(),
        name: "search_replace".to_string(),
        arguments: bad_args.clone().into(),
        vendor: Default::default(),
    };
    let item_valid = ConversationItem::assistant_tool_calls(vec![tool_call_valid]);
    let chat_msg_valid = conversation_item_to_chat_message(item_valid);
    assert_eq!(
        chat_msg_valid
            .tool_calls
            .first()
            .map(|c| c.function.arguments.as_str()),
        Some(bad_args.as_str()),
        "valid non-ASCII arguments must pass through unchanged"
    );
}

#[test]
fn test_tool_result_with_images_to_chat_completions() {
    let item = ConversationItem::tool_result_with_images(
        "call_1",
        "Read image file: photo.png",
        vec![ContentPart::Image {
            url: "data:image/png;base64,iVBOR".into(),
        }],
    );

    let msg = conversation_item_to_chat_message(item);
    assert_eq!(msg.role, Role::Tool);
    assert_eq!(msg.tool_call_id, Some("call_1".to_string()));

    let MessageContent::Blocks(blocks) = &msg.content else {
        panic!(
            "Expected Blocks content for image tool result, got {:?}",
            msg.content
        );
    };
    let [b0, b1] = blocks.as_slice() else {
        panic!("expected two blocks: {blocks:?}");
    };
    assert!(matches!(b0, ChatContentBlock::Text { text } if text == "Read image file: photo.png"));
    assert!(
        matches!(b1, ChatContentBlock::ImageUrl { image_url } if image_url.url == "data:image/png;base64,iVBOR")
    );
}

#[test]
fn conversation_to_chat_messages_drops_reasoning_when_user_intervenes() {
    // Reasoning only folds onto the *immediately* following assistant
    // A non-assistant item in between (here a User) clears pending reasoning
    // `conversation_to_chat_messages_drops_trailing_reasoning` covers the trailing case
    let items = vec![
        reasoning_sibling("r1", "stale thinking", None),
        ConversationItem::user("actually, new question"),
        ConversationItem::assistant("answer"),
    ];

    let msgs = conversation_to_chat_messages(items);

    let [user, assistant] = msgs.as_slice() else {
        panic!("expected user + assistant: {msgs:?}");
    };
    assert_eq!(user.role, Role::User);
    assert_eq!(assistant.role, Role::Assistant);
    assert_eq!(assistant.text_content(), "answer");
    assert_eq!(
        assistant.reasoning_content.as_deref(),
        None,
        "reasoning separated from the assistant by a user message is dropped"
    );
}

#[test]
fn upgrade_then_fold_through_conversation_to_chat_messages() {
    // End-to-end: lift legacy `reasoning` to a sibling, then run the chat-completions wire path
    // This mirrors what the real load-then-replay flow does for a legacy session
    let raw = serde_json::json!({
        "type": "assistant",
        "content": "the answer",
        "reasoning": {"text": "step-by-step", "id": "rs_x"}
    });
    let mut seen = std::collections::HashSet::new();
    let mut siblings = upgrade_legacy_reasoning(&raw, &mut seen);
    // Append the assistant by re-deserializing the same raw value; AssistantItem silently ignores `reasoning`
    let assistant: ConversationItem = serde_json::from_value(raw).unwrap();
    siblings.push(assistant);

    let msgs = conversation_to_chat_messages(siblings);
    let [msg] = msgs.as_slice() else {
        panic!("expected one message: {msgs:?}");
    };
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(
        msg.reasoning_content.as_deref(),
        Some("step-by-step"),
        "reconstructed sibling folded onto assistant.reasoning_content"
    );
}

/// Chat Completions is the widest provider surface (every OpenAI-compatible
/// gateway), so the loop's hand-built turn has to land as an assistant message
/// carrying the call plus a matching `tool` message.
#[test]
fn todo_capture_loop_maps_to_assistant_call_and_tool_message() {
    let request: ChatCompletionRequest =
        ConversationRequest::from_items(todo_capture_loop_items(false))
            .with_model("chat-completions-model")
            .into();
    let json = serde_json::to_value(&request).unwrap();
    let messages = json["messages"].as_array().unwrap();

    let call_at = messages
        .iter()
        .position(|m| {
            m["tool_calls"]
                .as_array()
                .is_some_and(|calls| calls.iter().any(|c| c["id"] == "call_todo_1"))
        })
        .unwrap_or_else(|| panic!("the loop's own tool call must survive: {json:#}"));
    let result_at = messages
        .iter()
        .position(|m| m["role"] == "tool" && m["tool_call_id"] == "call_todo_1")
        .unwrap_or_else(|| panic!("the tool result the loop fed back must survive: {json:#}"));
    assert!(
        call_at < result_at,
        "the call must precede its result; got {call_at} / {result_at}"
    );
}

/// Gemini 3 rejects a replayed function call whose thought signature is
/// missing, and the signature only ever reaches an OpenAI-shaped client on the
/// call itself. Whatever spelling it arrived in has to go back out unchanged.
#[test]
fn a_tool_calls_provider_fields_survive_the_round_trip() {
    for key in ["extra_content", "provider_specific_fields"] {
        let wire = serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "get_weather", "arguments": "{\"city\":\"Tokyo\"}" },
                key: { "google": { "thought_signature": "CpcHAdHtim9" } },
            }],
        });
        let msg: crate::types::ChatResponseMessage = serde_json::from_value(wire).unwrap();
        let item = ConversationItem::from(msg);

        let replayed = serde_json::to_value(conversation_item_to_chat_message(item)).unwrap();
        assert_eq!(
            replayed["tool_calls"][0][key],
            serde_json::json!({ "google": { "thought_signature": "CpcHAdHtim9" } }),
            "{key} must ride the replay: {replayed:#}",
        );
    }
}

/// The passthrough is only for the keys a provider reads back. Response-shaped
/// bookkeeping is not one of them, and a call that arrived with nothing extra
/// must serialize exactly as it did before any of this existed.
#[test]
fn a_tool_call_without_provider_fields_replays_unchanged() {
    let wire = serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": "call_1",
            "type": "function",
            "index": 0,
            "function": { "name": "get_weather", "arguments": "{}" },
        }],
    });
    let msg: crate::types::ChatResponseMessage = serde_json::from_value(wire).unwrap();
    let item = ConversationItem::from(msg);

    let replayed = serde_json::to_value(conversation_item_to_chat_message(item)).unwrap();
    assert_eq!(
        replayed["tool_calls"][0],
        serde_json::json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "get_weather", "arguments": "{}" },
        }),
        "an untouched call must go out untouched: {replayed:#}",
    );
}

// ============================================================================
// Strict-schema message profiles (Cerebras `wrong_api_format`)
// ============================================================================
//
// Cerebras validates its Chat Completions message schema strictly: an
// unrecognized property on any message is a hard 400, so a replayed
// assistant message carrying `model_id` (which this crate writes into stored
// history) bricks the conversation from turn 2 onward. These tests drive the
// real serialized body — the observable the provider actually sees — and
// assert that a strict target receives no unsupported property while a
// tolerant target's body is byte-for-byte unchanged.

/// A two-turn conversation whose assistant items carry both a recorded
/// `model_id` and a replayed reasoning sibling — exactly the history shape
/// that produced the Cerebras 400 on `messages.6.assistant`.
fn history_with_model_id_and_reasoning() -> Vec<ConversationItem> {
    vec![
        ConversationItem::system("You are helpful."),
        ConversationItem::user("q1"),
        reasoning_sibling("rs_1", "thinking about q1", None),
        ConversationItem::Assistant(AssistantItem {
            content: "a1".into(),
            tool_calls: vec![],
            model_id: Some("qwen-3.8-27b".into()),
            model_fingerprint: None,
            reasoning_effort: None,
        }),
        ConversationItem::user("q2"),
    ]
}

/// Serialize the request exactly as the sampler sends it, so the assertions
/// read the wire body rather than an intermediate struct.
fn wire_body(items: Vec<ConversationItem>, profile: ChatMessageProfile) -> serde_json::Value {
    let mut req = ConversationRequest::from_items(items);
    req.chat_message_profile = profile;
    let wire: crate::types::ChatCompletionRequest = req.into();
    serde_json::to_value(&wire).unwrap()
}

/// The decisive check: for a strict-schema target, the serialized assistant
/// messages carry neither `model_id` nor `reasoning_content`.
///
/// This drives the real `From<ConversationRequest> for ChatCompletionRequest`
/// conversion (the same one `SamplingClient::conversation_stream` uses), then
/// inspects the JSON the provider would receive. Both properties are absent,
/// not null or empty — a strict schema rejects on presence.
#[test]
fn strict_profile_omits_model_id_and_reasoning_content_from_wire_body() {
    let body = wire_body(
        history_with_model_id_and_reasoning(),
        ChatMessageProfile::STRICT,
    );
    let messages = body["messages"].as_array().expect("messages array");

    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message present");
    assert!(
        assistant.get("model_id").is_none(),
        "strict target must not receive model_id: {assistant:#}"
    );
    assert!(
        assistant.get("reasoning_content").is_none(),
        "strict target must not receive reasoning_content: {assistant:#}"
    );

    // No message of any role may carry an unsupported property.
    for m in messages {
        assert!(
            m.get("model_id").is_none(),
            "no message may carry model_id for a strict target: {m:#}"
        );
        assert!(
            m.get("reasoning_content").is_none(),
            "no message may carry reasoning_content for a strict target: {m:#}"
        );
    }

    // The conversation itself must survive: dropping the two properties must
    // not drop content or structure. The `Reasoning` sibling folds into the
    // assistant, so it contributes no message of its own: system, user,
    // assistant, user.
    assert_eq!(assistant["content"], serde_json::json!("a1"));
    assert_eq!(messages.len(), 4, "structure preserved: {messages:#?}");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], serde_json::json!("q1"));
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[3]["content"], serde_json::json!("q2"));
}

/// The regression guard: a tolerant provider's body is unchanged. Same
/// conversation, permissive profile — both properties are still present.
#[test]
fn permissive_profile_still_sends_model_id_and_reasoning_content() {
    let body = wire_body(
        history_with_model_id_and_reasoning(),
        ChatMessageProfile::PERMISSIVE,
    );
    let messages = body["messages"].as_array().expect("messages array");

    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message present");
    assert_eq!(
        assistant["model_id"],
        serde_json::json!("qwen-3.8-27b"),
        "tolerant target keeps model_id: {assistant:#}"
    );
    assert_eq!(
        assistant["reasoning_content"],
        serde_json::json!("thinking about q1"),
        "tolerant target keeps replayed reasoning: {assistant:#}"
    );
}

/// The default profile must be today's behavior, or every existing provider
/// would silently change shape.
#[test]
fn default_profile_is_permissive() {
    assert_eq!(
        ChatMessageProfile::default(),
        ChatMessageProfile::PERMISSIVE
    );
    assert!(ChatMessageProfile::default().is_permissive());
    assert!(!ChatMessageProfile::STRICT.is_permissive());

    // A request built the ordinary way (no profile set) sends both fields.
    let mut req = ConversationRequest::from_items(history_with_model_id_and_reasoning());
    assert_eq!(
        req.chat_message_profile,
        ChatMessageProfile::PERMISSIVE,
        "ConversationRequest::from_items must default to permissive"
    );
    req.model = Some("m".into());
    let wire: crate::types::ChatCompletionRequest = req.into();
    let body = serde_json::to_value(&wire).unwrap();
    let assistant = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(assistant["model_id"], serde_json::json!("qwen-3.8-27b"));
    assert_eq!(
        assistant["reasoning_content"],
        serde_json::json!("thinking about q1")
    );
}

/// Suppressing a property must not disturb fields the provider *does* accept:
/// tool calls, their vendor passthrough, and tool results all ride unchanged.
#[test]
fn strict_profile_preserves_tool_calls_and_results() {
    let items = vec![
        ConversationItem::user("run it"),
        ConversationItem::Assistant(AssistantItem {
            content: String::new().into(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "get_weather".to_string(),
                arguments: r#"{"city":"SF"}"#.into(),
                vendor: Default::default(),
            }],
            model_id: Some("qwen-3.8-27b".into()),
            model_fingerprint: None,
            reasoning_effort: None,
        }),
        ConversationItem::tool_result("call_1", "sunny"),
    ];

    let body = wire_body(items.clone(), ChatMessageProfile::STRICT);
    let messages = body["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant present");
    assert!(assistant.get("model_id").is_none());
    assert_eq!(
        assistant["tool_calls"][0]["function"]["name"],
        serde_json::json!("get_weather"),
        "tool calls survive the strip: {assistant:#}"
    );
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        serde_json::json!(r#"{"city":"SF"}"#)
    );
    let tool_msg = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("tool result present");
    assert_eq!(tool_msg["tool_call_id"], serde_json::json!("call_1"));
    assert_eq!(tool_msg["content"], serde_json::json!("sunny"));
}

/// Only the *serialized body* is narrowed. The stored conversation keeps both
/// values, which is what lets the Messages backend resolve thinking
/// signatures and lets a later turn on a tolerant provider still send
/// reasoning.
#[test]
fn strict_profile_leaves_stored_history_untouched() {
    let items = history_with_model_id_and_reasoning();
    let before = items.clone();

    let mut req = ConversationRequest::from_items(items);
    req.chat_message_profile = ChatMessageProfile::STRICT;
    let _wire: crate::types::ChatCompletionRequest = req.into();

    // `req.items` is moved by the conversion; assert on a freshly built
    // request instead, so the check is on stored state, not the wire.
    let mut stored = ConversationRequest::from_items(before);
    stored.chat_message_profile = ChatMessageProfile::STRICT;

    let assistant = stored
        .items
        .iter()
        .find_map(|i| match i {
            ConversationItem::Assistant(a) => Some(a),
            _ => None,
        })
        .expect("assistant item");
    assert_eq!(
        assistant.model_id.as_deref(),
        Some("qwen-3.8-27b"),
        "stored model_id must survive a strict-profile conversion"
    );
    assert!(
        stored
            .items
            .iter()
            .any(|i| matches!(i, ConversationItem::Reasoning(_))),
        "reasoning siblings must survive in stored history"
    );
}

/// `narrowed_by` can only narrow: a caller cannot re-widen a strict model,
/// and two permissive sides stay permissive.
#[test]
fn profile_narrowing_is_monotonic() {
    let p = ChatMessageProfile::PERMISSIVE;
    let s = ChatMessageProfile::STRICT;

    assert_eq!(p.narrowed_by(p), p);
    assert_eq!(
        p.narrowed_by(s),
        s,
        "permissive narrowed by strict is strict"
    );
    assert_eq!(s.narrowed_by(p), s, "strict cannot be widened");
    assert_eq!(s.narrowed_by(s), s);
}

/// Partial profiles: a provider may accept one property and reject the other.
#[test]
fn partial_profiles_suppress_independently() {
    let only_model_id = ChatMessageProfile {
        accepts_model_id: true,
        accepts_reasoning_content: false,
    };
    let body = wire_body(history_with_model_id_and_reasoning(), only_model_id);
    let messages = body["messages"].as_array().unwrap();
    let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(
        assistant["model_id"],
        serde_json::json!("qwen-3.8-27b"),
        "model_id accepted by this profile must be sent"
    );
    assert!(
        assistant.get("reasoning_content").is_none(),
        "reasoning_content rejected by this profile must be omitted"
    );

    let only_reasoning = ChatMessageProfile {
        accepts_model_id: false,
        accepts_reasoning_content: true,
    };
    let body = wire_body(history_with_model_id_and_reasoning(), only_reasoning);
    let messages = body["messages"].as_array().unwrap();
    let assistant = messages.iter().find(|m| m["role"] == "assistant").unwrap();
    assert!(
        assistant.get("model_id").is_none(),
        "model_id rejected by this profile must be omitted"
    );
    assert_eq!(
        assistant["reasoning_content"],
        serde_json::json!("thinking about q1"),
        "reasoning_content accepted by this profile must be sent"
    );
}

/// `strip_unsupported_message_properties` drops exactly what the provider
/// named, and reports whether it changed anything so the retry loop can tell a
/// productive strip from a no-op.
#[test]
fn strip_unsupported_message_properties_narrows_named_fields_only() {
    let mut req = ConversationRequest::from_items(history_with_model_id_and_reasoning());
    assert!(req.chat_message_profile.is_permissive());

    // Provider named only `model_id`.
    assert!(req.strip_unsupported_message_properties(true, false));
    assert!(!req.chat_message_profile.accepts_model_id);
    assert!(
        req.chat_message_profile.accepts_reasoning_content,
        "reasoning_content was not named, so it stays"
    );

    // A second strip for the other named property still makes progress.
    assert!(req.strip_unsupported_message_properties(false, true));
    assert!(!req.chat_message_profile.accepts_reasoning_content);

    // Now fully narrowed: further strips are no-ops, so the caller can stop.
    assert!(
        !req.strip_unsupported_message_properties(true, true),
        "an already-narrow profile must report no change"
    );
}

/// An unsupported-property error that names nothing still narrows both — that
/// error class exists only for targets whose schema takes neither.
#[test]
fn strip_with_no_named_property_narrows_both() {
    let mut req = ConversationRequest::from_items(history_with_model_id_and_reasoning());
    assert!(req.strip_unsupported_message_properties(false, false));
    assert_eq!(req.chat_message_profile, ChatMessageProfile::STRICT);
}

/// The recovery must reach the wire: after a strip, the serialized body for
/// the same stored history carries no unsupported property — which is what
/// un-bricks a session whose history predates the fix.
#[test]
fn strip_then_serialize_omits_unsupported_properties() {
    let mut req = ConversationRequest::from_items(history_with_model_id_and_reasoning());
    req.model = Some("qwen-3.8-27b".into());

    // Before recovery, the body carries the properties the provider rejects.
    let before: crate::types::ChatCompletionRequest = req.clone().into();
    let before = serde_json::to_value(&before).unwrap();
    assert!(before["messages"][2].get("model_id").is_some());

    // The provider's 400 names both; strip, then serialize again.
    assert!(req.strip_unsupported_message_properties(true, true));
    let after: crate::types::ChatCompletionRequest = req.into();
    let after = serde_json::to_value(&after).unwrap();
    for m in after["messages"].as_array().unwrap() {
        assert!(
            m.get("model_id").is_none() && m.get("reasoning_content").is_none(),
            "recovered body must carry no unsupported property: {m:#}"
        );
    }
}
