use super::*;

/// Marks the last block that can carry one, scanning back past `Thinking`, which the API rejects a breakpoint on.
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

/// An entry is written only at a breakpoint, so marking the system prompt alone leaves the transcript uncached.
/// The third covers a turn that appends more than the API's 20 block lookback.
/// The fourth slot stays free: a gateway that turns on automatic caching takes it, and five is rejected outright.
fn apply_cache_breakpoints(
    system_blocks: &mut [crate::messages::TextBlock],
    messages: &mut [crate::messages::Message],
) {
    use crate::messages::{CacheControl, MessageRole};

    if let Some(last) = system_blocks.last_mut() {
        last.cache_control = Some(CacheControl::ephemeral());
    }

    let tip = (0..messages.len()).rev().find(|&i| {
        messages
            .get_mut(i)
            .is_some_and(mark_message_cache_breakpoint)
    });

    // Where the previous request ended
    // A turn can append several user messages in a row, so skip the whole trailing run rather than a neighbour of the tip
    if let Some(tip) = tip
        && let Some(before_tip) = messages.get(..tip)
        && let Some(prev) = before_tip
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant))
            .and_then(|assistant| {
                before_tip.get(..assistant).and_then(|before_asst| {
                    before_asst
                        .iter()
                        .rposition(|m| matches!(m.role, MessageRole::User))
                })
            })
        && let Some(msg) = messages.get_mut(prev)
    {
        mark_message_cache_breakpoint(msg);
    }
}

/// The replay plan for this request. A Claude id signs its thinking whatever
/// the conversation shows, so that knowledge goes in beside the evidence.
fn replay_plan(req: &ConversationRequest) -> ThinkingReplayPlan {
    let target_is_claude = req
        .model
        .as_deref()
        .is_some_and(|m| claude_version(m).is_some());
    ThinkingReplayPlan::new(req, target_is_claude)
}

/// Whether the conversation ends mid-tool-loop on a turn whose thinking block
/// the plan leaves behind. A provider validates the thinking of a tool-calling
/// turn it is being asked to continue ("thinking blocks cannot be modified",
/// and thinking-on requires that turn to lead with one), and the block is
/// exactly what a model switch takes away. Neither half is recoverable, so the
/// whole request goes out with thinking off; the next turn is this model's own
/// and pairs normally.
fn open_tool_loop_lost_its_thinking(req: &ConversationRequest, plan: &ThinkingReplayPlan) -> bool {
    if req.model.is_none() {
        return false;
    }
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

    req.items[..open]
        .iter()
        .enumerate()
        .rev()
        .take_while(|(_, item)| {
            matches!(
                item,
                ConversationItem::Reasoning(_) | ConversationItem::BackendToolCall(_)
            )
        })
        .any(|(idx, item)| match item {
            ConversationItem::Reasoning(r) => plan.is_foreign(&req.items, idx, r),
            _ => false,
        })
}

/// The Claude generation a model id names, as `(major, minor)`. Both spellings
/// the family has used are read: `claude-haiku-4-5` and `claude-3-7-sonnet`,
/// each optionally behind a gateway prefix and ahead of a dated snapshot. The
/// first one- or two-digit component is the major, which is what keeps a
/// snapshot stamp from being read as a version.
fn claude_version(model: &str) -> Option<(u32, u32)> {
    let tail = model.to_ascii_lowercase();
    let tail = tail.split("claude").nth(1)?;
    let mut parts = tail
        .split(['-', '.', '_', '@', ':'])
        .filter(|p| !p.is_empty())
        .skip_while(|p| !matches!(p.len(), 1 | 2) || !p.bytes().all(|b| b.is_ascii_digit()));

    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()
        .filter(|p| matches!(p.len(), 1 | 2))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    Some((major, minor))
}

/// Which `thinking` dialect a model speaks. Claude 4.6 replaced
/// `{"type":"enabled","budget_tokens":N}` with `{"type":"adaptive"}` plus
/// `output_config.effort`, and each generation rejects the other's spelling
/// outright ("Input tag 'adaptive' ... does not match any of the expected
/// tags"). A name that is not a Claude at all is a gateway's own model, which
/// this cannot speak for: it keeps the request it has always been sent.
fn speaks_adaptive_thinking(model: &str) -> bool {
    claude_version(model).is_none_or(|version| version >= (4, 6))
}

/// The `budget_tokens` a pre-4.6 Claude sizes its thinking with, standing in
/// for the effort word its dialect has no room for. The API's floor is 1024 and
/// the budget must leave the answer room under `max_tokens`, so a ceiling that
/// cannot house the floor yields no thinking rather than a 400.
fn thinking_budget(effort: crate::ReasoningEffort, max_tokens: u32) -> Option<u32> {
    use crate::ReasoningEffort as Effort;

    let want = match effort {
        Effort::None | Effort::Minimal => return None,
        Effort::Low => 4_096,
        Effort::Medium => 8_192,
        Effort::High => 16_384,
        Effort::Xhigh => 24_576,
        Effort::Max => 32_768,
    };
    let budget = want.min(max_tokens.saturating_sub(1));
    (budget >= 1_024).then_some(budget)
}

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
    let mut converted_foreign_thinking = 0usize;
    let plan = replay_plan(req);
    // Thinking off for this request takes its blocks with it: a block sent
    // without the top-level config is rejected in turn.
    let thinking_off = open_tool_loop_lost_its_thinking(req, &plan);

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
            // `tco_*` blobs carry only `signature`; real reasoning sets `thinking`
            ConversationItem::Reasoning(r) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let thinking = reasoning_item_text(r);
                let signature = r
                    .encrypted_content
                    .as_deref()
                    .map(str::to_owned)
                    .unwrap_or_default();
                let mut disposition = plan.disposition(&req.items, idx, r);
                // Thinking off for this request means no typed block at all.
                // The words still ride, as an ordinary assistant message.
                if thinking_off && disposition == ThinkingDisposition::Native {
                    disposition = if thinking.is_empty() {
                        ThinkingDisposition::Drop
                    } else {
                        ThinkingDisposition::Text
                    };
                }
                match disposition {
                    ThinkingDisposition::Drop => {
                        if !thinking.is_empty() || !signature.is_empty() {
                            dropped_foreign_thinking += 1;
                        }
                    }
                    ThinkingDisposition::Text => {
                        converted_foreign_thinking += 1;
                        pending_assistant.push(ContentBlock::Text {
                            text: thinking_as_text(r),
                            cache_control: None,
                        });
                    }
                    ThinkingDisposition::Native => {
                        pending_assistant.push(ContentBlock::Thinking {
                            thinking,
                            signature,
                        });
                    }
                }
            }
        }
    }

    flush_assistant(&mut pending_assistant, &mut messages);
    flush_tool_results(&mut pending_tool_results, &mut messages);

    if dropped_foreign_thinking > 0 || converted_foreign_thinking > 0 {
        tracing::warn!(
            dropped = dropped_foreign_thinking,
            converted = converted_foreign_thinking,
            model = req.model.as_deref().unwrap_or_default(),
            thinking_off,
            level = plan.level().as_str(),
            "thinking block(s) this model cannot take: the ones carrying text went as plain assistant messages, the rest went"
        );
    }

    apply_cache_breakpoints(&mut system_blocks, &mut messages);

    let system: Option<SystemParam> = if system_blocks.is_empty() {
        None
    } else if let [block] = system_blocks.as_slice()
        && block.cache_control.is_none()
    {
        Some(SystemParam::Text(block.text.clone()))
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
                    eager_input_streaming: crate::messages::True,
                })
                .collect(),
        )
    };

    let tool_choice: Option<ToolChoiceParam> = req.tool_choice.as_ref().map(|tc| match tc {
        ConversationToolChoice::Auto => ToolChoiceParam::Auto,
        ConversationToolChoice::Required => ToolChoiceParam::Any,
        ConversationToolChoice::Function(name) => ToolChoiceParam::Tool { name: name.clone() },
        ConversationToolChoice::None => ToolChoiceParam::Auto, // ToolChoiceParam has no none variant, so fall back to the default
    });

    // The mandatory-reasoning remap happens BEFORE the Messages mapping: for
    // a reasoning-mandatory target, `None`/`Minimal` must not be omitted from
    // the wire, so they are lifted to the lowest supported non-disabled
    // effort first. A request for such a target always carries
    // `output_config.effort` (and its auto-paired `thinking`).
    let effective_effort = wire_reasoning_effort(req.reasoning_mandatory, req.reasoning_effort);
    let effort = effective_effort
        .map(|e| e.to_messages_api())
        .flatten()
        .map(|s| s.to_string());

    // A wire schema here suppresses tool calls, so the agent routes structured output through the StructuredOutput tool instead
    let format = req
        .json_schema
        .as_ref()
        .map(|schema| crate::messages::OutputFormat::JsonSchema {
            schema: schema.clone(),
        });

    let max_tokens = req.max_output_tokens.unwrap_or(0);
    let adaptive = req.model.as_deref().is_none_or(speaks_adaptive_thinking);

    // thinking is driven by reasoning_effort only, not by json_schema.
    let thinking = if thinking_off {
        None
    } else if adaptive {
        effort
            .as_ref()
            .map(|_| crate::messages::ThinkingConfig::Adaptive {
                display: Some(crate::messages::ThinkingDisplay::Summarized),
            })
    } else {
        let budget = req
            .reasoning_effort
            .and_then(|e| thinking_budget(e, max_tokens));
        if budget.is_none() && effort.is_some() {
            tracing::warn!(
                model = req.model.as_deref().unwrap_or_default(),
                max_tokens,
                "thinking off: this model sizes it in tokens and max_tokens leaves no room for the API's 1024 floor"
            );
        }
        budget.map(|budget_tokens| crate::messages::ThinkingConfig::Enabled { budget_tokens })
    };

    // `output_config.effort` is 4.6-and-later too; an older Claude 400s on it,
    // and its `thinking` budget already carries the same intent.
    let effort = effort.filter(|_| adaptive);

    let output_config = if effort.is_some() || format.is_some() {
        Some(OutputConfig { effort, format })
    } else {
        None
    };

    MessagesRequest {
        model: req.model.clone().unwrap_or_default(),
        messages,
        max_tokens,
        system,
        tools,
        tool_choice,
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        stream: None, // The caller sets this
        stop_sequences: None,
        thinking,
        output_config,
        metadata: None,
    }
}

/// `Thinking` is dropped because this `From` returns a single item; the streaming consumer emits the sibling `Reasoning` item instead.
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
                        vendor: Default::default(),
                    });
                }
                // Thinking is dropped; see the doc comment above
                ContentBlock::Thinking { .. } => {}
                _ => {} // Image and ToolResult are not expected in assistant responses
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
