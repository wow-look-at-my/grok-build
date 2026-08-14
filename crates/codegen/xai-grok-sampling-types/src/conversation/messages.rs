//! Messages API wire format.

use super::*;

/// Marks the last block that can carry one, scanning back past `Thinking`,
/// which the API rejects a breakpoint on.
fn mark_message_cache_breakpoint(msg: &mut crate::messages::Message) -> bool {
    use crate::messages::{CacheControl, ContentBlock, MessageContent};

    match &mut msg.content {
        MessageContent::Blocks(blocks) => {
            for block in blocks.iter_mut().rev() {
                let cache_control = match block {
                    ContentBlock::Text { cache_control, .. }
                    | ContentBlock::ToolResult { cache_control, .. }
                    | ContentBlock::Image { cache_control, .. }
                    | ContentBlock::ToolUse { cache_control, .. } => cache_control,
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {
                        continue;
                    }
                };
                *cache_control = Some(CacheControl::ephemeral());
                return true;
            }
            false
        }
        // Plain text cannot carry a breakpoint, so promote it to block form.
        MessageContent::Text(text) => {
            let text = std::mem::take(text);
            msg.content = MessageContent::Blocks(vec![ContentBlock::Text {
                text,
                cache_control: Some(CacheControl::ephemeral()),
            }]);
            true
        }
    }
}

/// An entry is written only at a breakpoint, so marking the system prompt alone
/// leaves the transcript uncached. The third covers a turn that appends more
/// than the API's 20 block lookback. The fourth slot stays free: a gateway that
/// turns on automatic caching takes it, and five is rejected outright.
fn apply_cache_breakpoints(
    system_blocks: &mut [crate::messages::TextBlock],
    messages: &mut [crate::messages::Message],
) {
    use crate::messages::{CacheControl, MessageRole};

    if let Some(last) = system_blocks.last_mut() {
        last.cache_control = Some(CacheControl::ephemeral());
    }

    let tip = (0..messages.len())
        .rev()
        .find(|&i| mark_message_cache_breakpoint(&mut messages[i]));

    // Where the previous request ended. A turn can append several user messages
    // in a row, so skip the whole trailing run rather than a neighbour of the tip.
    if let Some(tip) = tip
        && let Some(prev) = messages[..tip]
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant))
            .and_then(|assistant| {
                messages[..assistant]
                    .iter()
                    .rposition(|m| matches!(m.role, MessageRole::User))
            })
    {
        mark_message_cache_breakpoint(&mut messages[prev]);
    }
}

/// Whether a thinking block minted by `origin` can be replayed to `target`.
/// The signature is verified against the model that produced it, so anything
/// else is a 400. An alias and the dated snapshot it answers as are one model
/// (`claude-opus-5` / `claude-opus-5-20260101`), and a gateway's routing
/// prefix (`anthropic/claude-opus-5`) is not part of the name.
fn same_model(origin: &str, target: &str) -> bool {
    fn normalize(model: &str) -> &str {
        let model = model.rsplit('/').next().unwrap_or(model);
        model.strip_suffix("-latest").unwrap_or(model)
    }

    let (origin, target) = (normalize(origin), normalize(target));
    if origin.eq_ignore_ascii_case(target) {
        return true;
    }
    let (long, short) = if origin.len() > target.len() {
        (origin, target)
    } else {
        (target, origin)
    };
    let Some(prefix) = long.as_bytes().get(..short.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(short.as_bytes()) {
        return false;
    }
    match long.as_bytes()[short.len()..].split_first() {
        Some((b'-', date)) => !date.is_empty() && date.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// The model behind a reasoning item: the streaming layer emits each
/// `Reasoning` as the sibling of the `Assistant` item that follows it. A
/// `User` or `System` item closes the turn, so reasoning with no assistant
/// behind it has no recorded origin.
fn reasoning_origin_model(items: &[ConversationItem], reasoning_idx: usize) -> Option<&str> {
    items[reasoning_idx + 1..]
        .iter()
        .find_map(|item| match item {
            ConversationItem::Assistant(a) => Some(a.model_id.as_deref()),
            ConversationItem::User(_) | ConversationItem::System(_) => Some(None),
            _ => None,
        })
        .flatten()
}

/// Whether the conversation ends mid-tool-loop on a turn whose thinking block
/// another model minted. A provider validates the thinking of a tool-calling
/// turn it is being asked to continue ("thinking blocks cannot be modified",
/// and thinking-on requires that turn to lead with one), and the block is
/// exactly what a model switch takes away. Neither half is recoverable, so the
/// whole request goes out with thinking off; the next turn is this model's own
/// and pairs normally.
fn open_tool_loop_lost_its_thinking(req: &ConversationRequest) -> bool {
    let Some(target) = req.model.as_deref() else {
        return false;
    };
    // The last tool-calling assistant with nothing but its results behind it.
    let mut open = None;
    for (idx, item) in req.items.iter().enumerate() {
        match item {
            ConversationItem::Assistant(a) if !a.tool_calls.is_empty() => open = Some(idx),
            ConversationItem::Assistant(_)
            | ConversationItem::User(_)
            | ConversationItem::System(_) => open = None,
            _ => {}
        }
    }
    let Some(open) = open else { return false };
    let ConversationItem::Assistant(assistant) = &req.items[open] else {
        return false;
    };
    let Some(origin) = assistant.model_id.as_deref() else {
        return false;
    };

    !same_model(origin, target)
        && req.items[..open]
            .iter()
            .rev()
            .take_while(|item| {
                matches!(
                    item,
                    ConversationItem::Reasoning(_) | ConversationItem::BackendToolCall(_)
                )
            })
            .any(|item| matches!(item, ConversationItem::Reasoning(_)))
}

/// Convert a `ConversationRequest` into a Messages API request.
pub fn build_messages_request(req: &ConversationRequest) -> crate::messages::MessagesRequest {
    use crate::messages::{
        ContentBlock, ImageSource, Message, MessageContent, MessageRole, MessagesRequest,
        OutputConfig, SystemParam, TextBlock, ToolChoiceParam, ToolParam, ToolResultContent,
    };

    let mut system_blocks: Vec<TextBlock> = Vec::new();
    let mut messages: Vec<Message> = Vec::new();
    let mut pending_assistant: Vec<ContentBlock> = Vec::new();
    let mut pending_tool_results: Vec<ContentBlock> = Vec::new();

    let sanitize_tool_call_id = |id: &str| -> String {
        id.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };

    let content_parts_to_anthropic_blocks = |parts: &[ContentPart]| -> Vec<ContentBlock> {
        parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => ContentBlock::Text {
                    text: text.as_ref().to_owned(),
                    cache_control: None,
                },
                ContentPart::Image { url } => {
                    if url.starts_with("data:") {
                        if let Some((header, data)) = url.split_once(',') {
                            let media_type = header
                                .strip_prefix("data:")
                                .and_then(|h| h.strip_suffix(";base64"))
                                .unwrap_or("image/png")
                                .to_string();
                            ContentBlock::Image {
                                source: ImageSource::Base64 {
                                    media_type,
                                    data: data.to_string(),
                                },
                                cache_control: None,
                            }
                        } else {
                            // Malformed data URI, treat as text
                            ContentBlock::Text {
                                text: format!("[invalid image: {}]", url),
                                cache_control: None,
                            }
                        }
                    } else if url.starts_with("http://") || url.starts_with("https://") {
                        ContentBlock::Image {
                            source: ImageSource::Url {
                                url: url.as_ref().to_owned(),
                            },
                            cache_control: None,
                        }
                    } else {
                        // Unknown format, treat as text
                        ContentBlock::Text {
                            text: format!("[image: {}]", url),
                            cache_control: None,
                        }
                    }
                }
            })
            .collect()
    };

    let flush_assistant = |pending: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
        if !pending.is_empty() {
            msgs.push(Message {
                role: MessageRole::Assistant,
                content: MessageContent::Blocks(pending.clone()),
            });
            pending.clear();
        }
    };

    let flush_tool_results = |pending: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
        if !pending.is_empty() {
            msgs.push(Message {
                role: MessageRole::User,
                content: MessageContent::Blocks(pending.clone()),
            });
            pending.clear();
        }
    };

    let mut dropped_foreign_thinking = 0usize;
    // Thinking off for this request takes its blocks with it: a block sent
    // without the top-level config is rejected in turn.
    let thinking_off = open_tool_loop_lost_its_thinking(req);

    for (idx, item) in req.items.iter().enumerate() {
        match item {
            ConversationItem::System(s) => {
                flush_assistant(&mut pending_assistant, &mut messages);
                flush_tool_results(&mut pending_tool_results, &mut messages);
                system_blocks.push(TextBlock {
                    r#type: "text".to_string(),
                    text: s.content.as_ref().to_owned(),
                    cache_control: None,
                });
            }
            ConversationItem::User(u) => {
                flush_assistant(&mut pending_assistant, &mut messages);
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let blocks = content_parts_to_anthropic_blocks(&u.content);
                messages.push(Message {
                    role: MessageRole::User,
                    content: MessageContent::Blocks(blocks),
                });
            }
            ConversationItem::Assistant(a) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);

                if !a.content.is_empty() {
                    pending_assistant.push(ContentBlock::Text {
                        text: a.content.as_ref().to_owned(),
                        cache_control: None,
                    });
                }

                for tc in &a.tool_calls {
                    let input =
                        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
                    pending_assistant.push(ContentBlock::ToolUse {
                        id: sanitize_tool_call_id(&tc.id),
                        name: tc.name.clone(),
                        input,
                        cache_control: None,
                    });
                }
            }
            ConversationItem::ToolResult(t) => {
                flush_assistant(&mut pending_assistant, &mut messages);
                let content = if t.images.is_empty() {
                    ToolResultContent::Text(t.content.as_ref().to_owned())
                } else {
                    let mut blocks = vec![ContentBlock::Text {
                        text: t.content.as_ref().to_owned(),
                        cache_control: None,
                    }];
                    for img in &t.images {
                        if let ContentPart::Image { url } = img {
                            let source = if let Some(rest) = url.strip_prefix("data:") {
                                if let Some((media_type, data)) = rest.split_once(";base64,") {
                                    ImageSource::Base64 {
                                        media_type: media_type.to_string(),
                                        data: data.to_string(),
                                    }
                                } else {
                                    ImageSource::Url {
                                        url: url.as_ref().to_owned(),
                                    }
                                }
                            } else {
                                ImageSource::Url {
                                    url: url.as_ref().to_owned(),
                                }
                            };
                            blocks.push(ContentBlock::Image {
                                source,
                                cache_control: None,
                            });
                        }
                    }
                    ToolResultContent::Blocks(blocks)
                };
                pending_tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: sanitize_tool_call_id(&t.tool_call_id),
                    content,
                    cache_control: None,
                });
            }
            // No native equivalent, so emit synthetic text to retain context.
            ConversationItem::BackendToolCall(b) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                pending_assistant.push(ContentBlock::Text {
                    text: b.text_summary(),
                    cache_control: None,
                });
            }
            // `tco_*` blobs carry only `signature`; real reasoning sets
            // `thinking`.
            ConversationItem::Reasoning(r) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let thinking = reasoning_item_text(r);
                let signature = r
                    .encrypted_content
                    .as_deref()
                    .map(str::to_owned)
                    .unwrap_or_default();
                if thinking.is_empty() && signature.is_empty() {
                    continue;
                }
                // A signature is only valid for the model that minted it, so a
                // conversation carried onto another model has to leave its
                // thinking behind rather than have every turn 400. Provenance
                // the history never recorded is replayed as before; the
                // strip-and-retry in the sampler covers a server that
                // disagrees.
                let foreign = matches!(
                    (
                        req.model.as_deref(),
                        reasoning_origin_model(&req.items, idx),
                    ),
                    (Some(target), Some(origin)) if !same_model(origin, target)
                );
                if foreign || thinking_off {
                    dropped_foreign_thinking += 1;
                    continue;
                }
                pending_assistant.push(ContentBlock::Thinking {
                    thinking,
                    signature,
                });
            }
        }
    }

    flush_assistant(&mut pending_assistant, &mut messages);
    flush_tool_results(&mut pending_tool_results, &mut messages);

    if dropped_foreign_thinking > 0 {
        tracing::warn!(
            dropped = dropped_foreign_thinking,
            model = req.model.as_deref().unwrap_or_default(),
            thinking_off,
            "dropped thinking block(s) minted by another model; this model cannot verify their signatures"
        );
    }

    apply_cache_breakpoints(&mut system_blocks, &mut messages);

    let system: Option<SystemParam> = if system_blocks.is_empty() {
        None
    } else if system_blocks.len() == 1 && system_blocks[0].cache_control.is_none() {
        // Single block without cache_control - can use text form
        Some(SystemParam::Text(system_blocks[0].text.clone()))
    } else {
        Some(SystemParam::Blocks(system_blocks))
    };

    let tools: Option<Vec<ToolParam>> = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| ToolParam {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect(),
        )
    };

    let tool_choice: Option<ToolChoiceParam> = req.tool_choice.as_ref().map(|tc| match tc {
        ConversationToolChoice::Auto => ToolChoiceParam::Auto,
        ConversationToolChoice::Required => ToolChoiceParam::Any,
        ConversationToolChoice::Function(name) => ToolChoiceParam::Tool { name: name.clone() },
        ConversationToolChoice::None => ToolChoiceParam::Auto, // default
    });

    let effort = req
        .reasoning_effort
        .and_then(|e| e.to_messages_api())
        .map(|s| s.to_string());

    // A wire schema here suppresses tool calls, so the agent routes
    // structured output through the StructuredOutput tool instead.
    let format = req
        .json_schema
        .as_ref()
        .map(|schema| crate::messages::OutputFormat::JsonSchema {
            schema: schema.clone(),
        });

    // thinking is driven by reasoning_effort only, not by json_schema.
    let thinking = effort
        .as_ref()
        .filter(|_| !thinking_off)
        .map(|_| crate::messages::ThinkingConfig::Adaptive {
            display: Some(crate::messages::ThinkingDisplay::Summarized),
        });

    let output_config = if effort.is_some() || format.is_some() {
        Some(OutputConfig { effort, format })
    } else {
        None
    };

    MessagesRequest {
        model: req.model.clone().unwrap_or_default(),
        messages,
        max_tokens: req.max_output_tokens.unwrap_or(0),
        system,
        tools,
        tool_choice,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        stream: None, // Set by caller
        stop_sequences: None,
        thinking,
        output_config,
        metadata: None,
    }
}

/// `Thinking` is dropped because this `From` returns a single item; the
/// streaming consumer emits the sibling `Reasoning` item instead.
impl From<crate::messages::MessagesResponse> for ConversationItem {
    fn from(resp: crate::messages::MessagesResponse) -> Self {
        use crate::messages::ContentBlock;

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for block in resp.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&text);
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tool_calls.push(ToolCall {
                        id: Arc::<str>::from(id),
                        name,
                        arguments: Arc::<str>::from(
                            serde_json::to_string(&input).unwrap_or_default(),
                        ),
                    });
                }
                // Thinking dropped — see doc comment above.
                ContentBlock::Thinking { .. } => {}
                _ => {} // Image, ToolResult not expected in assistant responses
            }
        }

        ConversationItem::Assistant(AssistantItem {
            content: Arc::<str>::from(content),
            tool_calls,
            model_id: Some(resp.model),
            model_fingerprint: None,
            reasoning_effort: None,
        })
    }
}
