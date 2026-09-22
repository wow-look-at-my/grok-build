//! Ollama native `/api/chat` request building.

use super::*;
use crate::conversation::thinking_replay::{ThinkingDisposition, ThinkingReplayPlan};
use crate::ollama::{
    OllamaChatRequest, OllamaMessage, OllamaTool, OllamaToolCall, OllamaToolCallFunction,
    OllamaToolFunction,
};

/// `options.num_ctx` — the window the runner is loaded at.
pub const NUM_CTX_OPTION: &str = "num_ctx";
/// `options.num_predict` — Ollama's spelling of a max-output budget.
pub const NUM_PREDICT_OPTION: &str = "num_predict";

/// Build the body for `POST /api/chat`.
///
/// Three fields here exist nowhere on Ollama's OpenAI-compatible endpoint, and
/// each is a correctness matter rather than a tuning knob:
///
/// * `options.num_ctx` pins the window. Without it the runner picks one from
///   available VRAM and the catalog's number becomes a guess the harness
///   compacts against.
/// * `truncate: false` turns a prompt overflow into a server error. The
///   default drops the oldest messages in silence, which orphans tool calls.
/// * `keep_alive` keeps the model resident between turns.
pub fn build_ollama_chat_request(req: &ConversationRequest) -> OllamaChatRequest {
    let plan = ThinkingReplayPlan::new(req, false);
    let messages = build_ollama_messages(req, &plan);

    let tools = (!req.tools.is_empty()).then(|| {
        req.tools
            .iter()
            .map(|tool| OllamaTool {
                r#type: "function".to_owned(),
                function: OllamaToolFunction {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                },
            })
            .collect::<Vec<_>>()
    });

    let mut options = serde_json::Map::new();
    if let Some(temperature) = req.temperature {
        options.insert(
            "temperature".to_owned(),
            serde_json::json!(f64::from(temperature)),
        );
    }
    if let Some(top_p) = req.top_p {
        options.insert("top_p".to_owned(), serde_json::json!(f64::from(top_p)));
    }
    // The fitted output budget is `num_predict` here. `max_output_tokens` has
    // already been fitted against the window by `fit_output_budget`.
    if let Some(max_output) = req.max_output_tokens {
        options.insert(NUM_PREDICT_OPTION.to_owned(), serde_json::json!(max_output));
    }

    OllamaChatRequest {
        model: req.model.clone().unwrap_or_default(),
        messages,
        stream: true,
        tools,
        think: ollama_think_value(req),
        // Ollama's `format` takes a bare JSON schema, not the Chat Completions
        // `{type, json_schema: {schema}}` envelope.
        format: req.json_schema.clone(),
        // Residency and truncation are the caller's to set through
        // `extra_body`; a default here would overwrite what the user wrote.
        keep_alive: None,
        truncate: None,
        options,
    }
}

/// The `think` field for this request.
///
/// Ollama takes a bool or a model-defined level, and its own OpenAI-compat
/// layer maps `reasoning_effort` onto exactly this. `None` leaves the model's
/// own default alone, which is what an unset effort means everywhere else.
fn ollama_think_value(req: &ConversationRequest) -> Option<serde_json::Value> {
    let effort = wire_reasoning_effort(req.reasoning_mandatory, req.reasoning_effort)?;
    Some(match effort {
        // Ollama has no "off" level: a bool is how thinking is disabled.
        crate::ReasoningEffort::None => serde_json::Value::Bool(false),
        other => serde_json::Value::String(other.as_str().to_owned()),
    })
}

fn build_ollama_messages(
    req: &ConversationRequest,
    plan: &ThinkingReplayPlan,
) -> Vec<OllamaMessage> {
    let mut messages: Vec<OllamaMessage> = Vec::with_capacity(req.items.len());

    for (idx, item) in req.items.iter().enumerate() {
        match item {
            ConversationItem::System(system) => messages.push(OllamaMessage {
                role: "system".to_owned(),
                content: system.content.to_string(),
                ..Default::default()
            }),

            ConversationItem::User(user) => {
                let mut text = String::new();
                let mut images = Vec::new();
                for part in &user.content {
                    match part {
                        ContentPart::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        // Ollama takes bare base64, never a data URI and never
                        // a URL: it has no fetcher, so a remote image would
                        // reach the model as nothing at all.
                        ContentPart::Image { url } => {
                            if let Some(base64) = base64_payload(url) {
                                images.push(base64);
                            }
                        }
                    }
                }
                messages.push(OllamaMessage {
                    role: "user".to_owned(),
                    content: text,
                    images,
                    ..Default::default()
                });
            }

            ConversationItem::Assistant(assistant) => {
                let tool_calls = assistant
                    .tool_calls
                    .iter()
                    .map(|call| OllamaToolCall {
                        // Ollama's own calls carry no id; one it never minted
                        // is not a key it can match a result against.
                        id: (!call.id.is_empty()).then(|| call.id.to_string()),
                        function: OllamaToolCallFunction {
                            name: call.name.clone(),
                            arguments: arguments_as_object(&call.arguments),
                        },
                    })
                    .collect::<Vec<_>>();

                messages.push(OllamaMessage {
                    role: "assistant".to_owned(),
                    content: assistant.content.to_string(),
                    tool_calls,
                    ..Default::default()
                });
            }

            ConversationItem::ToolResult(result) => messages.push(OllamaMessage {
                role: "tool".to_owned(),
                content: result.content.to_string(),
                tool_call_id: (!result.tool_call_id.is_empty())
                    .then(|| result.tool_call_id.clone()),
                ..Default::default()
            }),

            ConversationItem::Reasoning(reasoning) => {
                match plan.disposition(&req.items, idx, reasoning) {
                    // Ollama's thinking is unsigned plain text, so a replay
                    // rides on the assistant message it belongs to.
                    ThinkingDisposition::Native => {
                        let text = crate::conversation::reasoning_item_text(reasoning);
                        if !text.trim().is_empty() {
                            messages.push(OllamaMessage {
                                role: "assistant".to_owned(),
                                content: String::new(),
                                thinking: Some(text),
                                ..Default::default()
                            });
                        }
                    }
                    ThinkingDisposition::Text => messages.push(OllamaMessage {
                        role: "assistant".to_owned(),
                        content: crate::conversation::thinking_replay::thinking_as_text(reasoning),
                        ..Default::default()
                    }),
                    ThinkingDisposition::Drop => {}
                }
            }

            // Server-side calls belong to a backend Ollama does not have.
            ConversationItem::BackendToolCall(_) => {}
        }
    }

    merge_thinking_into_following_assistant(&mut messages);
    messages
}

/// Fold a thinking-only assistant message into the assistant message it
/// precedes.
///
/// A reasoning item and the assistant message it belongs to are two items
/// here and one message on Ollama's wire. Leaving them apart sends two
/// consecutive assistant turns, which renders as two separate replies in the
/// model's own template.
fn merge_thinking_into_following_assistant(messages: &mut Vec<OllamaMessage>) {
    let mut idx = 0;
    while idx + 1 < messages.len() {
        let is_thinking_only = messages[idx].role == "assistant"
            && messages[idx].thinking.is_some()
            && messages[idx].content.is_empty()
            && messages[idx].tool_calls.is_empty();
        let next_is_assistant =
            messages[idx + 1].role == "assistant" && messages[idx + 1].thinking.is_none();
        if is_thinking_only && next_is_assistant {
            let thinking = messages.remove(idx).thinking;
            messages[idx].thinking = thinking;
        }
        idx += 1;
    }
}

/// Ollama takes tool-call arguments as an object; every other backend here
/// carries them as a JSON-encoded string. Arguments that do not parse go out
/// as an object with the raw text under `input`, because dropping them sends
/// the model a call it never made.
fn arguments_as_object(arguments: &str) -> serde_json::Value {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value @ serde_json::Value::Object(_)) => value,
        _ => serde_json::json!({ "input": trimmed }),
    }
}

/// The base64 payload of a data URI, or `None` for a URL Ollama cannot fetch.
fn base64_payload(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (_, payload) = rest.split_once("base64,")?;
        return Some(payload.to_owned());
    }
    // A bare base64 blob with no envelope is already what the wire wants.
    (!url.starts_with("http://") && !url.starts_with("https://")).then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request_with(items: Vec<ConversationItem>) -> ConversationRequest {
        let mut req = ConversationRequest::new();
        req.items = items;
        req.model = Some("qwen3-coder:30b".to_owned());
        req
    }

    #[test]
    fn a_tool_result_pairs_by_id_and_arguments_become_an_object() {
        let req = request_with(vec![
            ConversationItem::system("be useful"),
            ConversationItem::user("list the files"),
            ConversationItem::Assistant(AssistantItem {
                content: Arc::from(""),
                tool_calls: vec![ToolCall {
                    id: Arc::from("call_1"),
                    name: "bash".to_owned(),
                    arguments: Arc::from(r#"{"cmd":"ls"}"#),
                    vendor: Default::default(),
                }],
                model_id: None,
                model_fingerprint: None,
                reasoning_effort: None,
            }),
            ConversationItem::ToolResult(ToolResultItem {
                tool_call_id: "call_1".to_owned(),
                content: Arc::from("a.rs b.rs"),
                images: Vec::new(),
            }),
        ]);

        let built = build_ollama_chat_request(&req);

        assert_eq!(built.messages.len(), 4);
        assert_eq!(built.messages[2].role, "assistant");
        let call = &built.messages[2].tool_calls[0];
        assert_eq!(call.id.as_deref(), Some("call_1"));
        assert_eq!(
            call.function.arguments,
            serde_json::json!({"cmd": "ls"}),
            "Ollama takes arguments as an object, not as an encoded string"
        );
        assert_eq!(built.messages[3].role, "tool");
        assert_eq!(built.messages[3].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn unparseable_arguments_are_carried_rather_than_dropped() {
        assert_eq!(
            arguments_as_object("not json at all"),
            serde_json::json!({"input": "not json at all"})
        );
        assert_eq!(
            arguments_as_object(""),
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn thinking_rides_on_the_assistant_message_it_belongs_to() {
        let reasoning = rs::ReasoningItem {
            id: String::new(),
            summary: vec![rs::SummaryPart::SummaryText(rs::SummaryTextContent {
                text: "first I look".to_owned(),
            })],
            content: None,
            encrypted_content: None,
            status: None,
        };
        let req = request_with(vec![
            ConversationItem::user("hi"),
            ConversationItem::Reasoning(reasoning),
            ConversationItem::Assistant(AssistantItem {
                content: Arc::from("hello"),
                tool_calls: Vec::new(),
                model_id: None,
                model_fingerprint: None,
                reasoning_effort: None,
            }),
        ]);

        let built = build_ollama_chat_request(&req);

        assert_eq!(
            built.messages.len(),
            2,
            "a reasoning item and its assistant message are one message on this wire"
        );
        assert_eq!(built.messages[1].content, "hello");
        assert_eq!(built.messages[1].thinking.as_deref(), Some("first I look"));
    }

    #[test]
    fn an_http_image_is_dropped_because_ollama_fetches_nothing() {
        assert_eq!(base64_payload("https://example.test/cat.png"), None);
        assert_eq!(
            base64_payload("data:image/png;base64,AAAA").as_deref(),
            Some("AAAA")
        );
    }

    #[test]
    fn the_output_budget_rides_as_num_predict() {
        let mut req = request_with(vec![ConversationItem::user("hi")]);
        req.max_output_tokens = Some(4096);

        let built = build_ollama_chat_request(&req);

        assert_eq!(
            built.options.get(NUM_PREDICT_OPTION),
            Some(&serde_json::json!(4096))
        );
        assert!(
            built.options.get(NUM_CTX_OPTION).is_none(),
            "the window is the caller's to set through extra_body, not a default here"
        );
    }
}
