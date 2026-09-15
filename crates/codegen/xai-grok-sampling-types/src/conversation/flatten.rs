//! Flatten a conversation to plain text so any model can ingest it.
//!
//! A history carries state that belongs to the provider that produced it: a
//! reasoning item's `encrypted_content`, a thinking block's signature, a tool
//! call's id and its vendor fields. Replay that to another model and the
//! request is rejected. The conversation is then unusable on that model.
//!
//! This turns every one of those into ordinary text. The record of what
//! happened survives. Nothing opaque to the target model is left in it.

use std::sync::Arc;

use super::{
    AssistantItem, ContentPart, ConversationItem, SyntheticReason, ToolResultItem, UserItem,
    reasoning_item_text,
};

/// What one flattening changed. The caller logs it, so a session that was
/// rewritten says by how much rather than only that it happened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlattenReport {
    pub items_before: usize,
    pub items_after: usize,
    /// Reasoning items rendered as assistant text.
    pub reasoning_to_text: usize,
    /// Reasoning items dropped: they carried an encrypted blob and no text,
    /// so there was nothing to render.
    pub reasoning_dropped: usize,
    /// Tool calls rendered into their assistant message's text.
    pub tool_calls_to_text: usize,
    /// Tool results rendered as user text.
    pub tool_results_to_text: usize,
    /// Server-side tool calls rendered as assistant text.
    pub backend_calls_to_text: usize,
}

impl FlattenReport {
    /// Whether the flattening rewrote anything at all. A conversation of
    /// plain messages flattens to itself.
    pub fn changed_anything(&self) -> bool {
        self.reasoning_to_text > 0
            || self.reasoning_dropped > 0
            || self.tool_calls_to_text > 0
            || self.tool_results_to_text > 0
            || self.backend_calls_to_text > 0
    }
}

/// Whether `items` holds anything a different model can refuse to ingest.
///
/// True for reasoning siblings, tool calls, tool results and server-side tool
/// calls. A conversation of plain user and assistant messages is portable as
/// it stands, and flattening one buys nothing.
pub fn needs_flattening(items: &[ConversationItem]) -> bool {
    items.iter().any(|item| match item {
        ConversationItem::Reasoning(_) | ConversationItem::BackendToolCall(_) => true,
        ConversationItem::Assistant(a) => !a.tool_calls.is_empty(),
        ConversationItem::ToolResult(_) => true,
        ConversationItem::System(_) | ConversationItem::User(_) => false,
    })
}

/// Rewrite `items` so every part of it is plain text.
///
/// Reasoning becomes a `<thinking>` assistant message and loses its encrypted
/// blob. An assistant message's tool calls become `<tool_call>` blocks in its
/// own text. A tool result becomes a `<tool_result>` user message, because no
/// call is left to pair it with. A server-side call becomes assistant text.
/// System and user messages pass through: neither carries provider state.
///
/// Every assistant message this touches loses its `model_id`. That origin is
/// what the thinking-signature rules read, and nothing here came from it.
pub fn flatten_conversation(
    items: Vec<ConversationItem>,
) -> (Vec<ConversationItem>, FlattenReport) {
    let mut report = FlattenReport {
        items_before: items.len(),
        ..FlattenReport::default()
    };
    let mut out: Vec<ConversationItem> = Vec::with_capacity(items.len());

    for item in items {
        match item {
            ConversationItem::System(s) => out.push(ConversationItem::System(s)),
            ConversationItem::User(u) => out.push(ConversationItem::User(u)),
            ConversationItem::Reasoning(r) => {
                let text = reasoning_item_text(&r);
                if text.trim().is_empty() {
                    // Encrypted-only: the text was never in the history, so
                    // nothing carries across. This is the whole loss, counted.
                    report.reasoning_dropped += 1;
                    continue;
                }
                report.reasoning_to_text += 1;
                out.push(assistant_text(format!("<thinking>\n{text}\n</thinking>")));
            }
            ConversationItem::BackendToolCall(b) => {
                report.backend_calls_to_text += 1;
                let summary = b.text_summary();
                out.push(assistant_text(format!(
                    "<backend_tool_call>\n{summary}\n</backend_tool_call>"
                )));
            }
            ConversationItem::Assistant(a) => {
                let AssistantItem {
                    content,
                    tool_calls,
                    model_id,
                    model_fingerprint,
                    reasoning_effort,
                } = a;
                if tool_calls.is_empty() {
                    out.push(ConversationItem::Assistant(AssistantItem {
                        content,
                        tool_calls,
                        model_id,
                        model_fingerprint,
                        reasoning_effort,
                    }));
                    continue;
                }
                let mut rendered = content.as_ref().to_owned();
                for tc in &tool_calls {
                    report.tool_calls_to_text += 1;
                    if !rendered.is_empty() {
                        rendered.push('\n');
                    }
                    rendered.push_str(&render_tool_call(&tc.id, &tc.name, &tc.arguments));
                }
                out.push(assistant_text(rendered));
            }
            ConversationItem::ToolResult(t) => {
                report.tool_results_to_text += 1;
                out.push(tool_result_as_user(t));
            }
        }
    }

    report.items_after = out.len();
    (out, report)
}

/// An assistant message carrying text and nothing else. No tool calls, and no
/// origin model: a flattened record is not attributable to the model that
/// produced it, and claiming otherwise re-arms the signature rules.
fn assistant_text(content: String) -> ConversationItem {
    ConversationItem::Assistant(AssistantItem {
        content: Arc::<str>::from(content),
        tool_calls: Vec::new(),
        model_id: None,
        model_fingerprint: None,
        reasoning_effort: None,
    })
}

fn render_tool_call(id: &str, name: &str, arguments: &str) -> String {
    format!("<tool_call id=\"{id}\" name=\"{name}\">\n{arguments}\n</tool_call>")
}

/// A tool result, as a user message. Images ride along: they are content the
/// target model reads directly, not provider state.
fn tool_result_as_user(t: ToolResultItem) -> ConversationItem {
    let ToolResultItem {
        tool_call_id,
        content,
        images,
    } = t;
    let text = format!(
        "<tool_result tool_call_id=\"{tool_call_id}\">\n{}\n</tool_result>",
        content.as_ref()
    );
    let mut parts = vec![ContentPart::Text {
        text: Arc::<str>::from(text),
    }];
    parts.extend(images);
    ConversationItem::User(UserItem {
        content: parts,
        synthetic_reason: Some(SyntheticReason::HistoryFlattened),
        ..UserItem::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ToolCall, synthesized_reasoning_item};
    use std::collections::BTreeMap;

    fn user(text: &str) -> ConversationItem {
        ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from(text),
            }],
            ..UserItem::default()
        })
    }

    fn assistant_with_call(text: &str, id: &str, name: &str, args: &str) -> ConversationItem {
        ConversationItem::Assistant(AssistantItem {
            content: Arc::<str>::from(text),
            tool_calls: vec![ToolCall {
                id: Arc::<str>::from(id),
                name: name.to_owned(),
                arguments: Arc::<str>::from(args),
                vendor: BTreeMap::new(),
            }],
            model_id: Some("grok-4".to_owned()),
            model_fingerprint: None,
            reasoning_effort: None,
        })
    }

    fn tool_result(id: &str, content: &str) -> ConversationItem {
        ConversationItem::ToolResult(ToolResultItem {
            tool_call_id: id.to_owned(),
            content: Arc::<str>::from(content),
            images: Vec::new(),
        })
    }

    fn text_of(item: &ConversationItem) -> String {
        match item {
            ConversationItem::Assistant(a) => a.content.as_ref().to_owned(),
            ConversationItem::User(u) => u
                .content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_ref().to_owned()),
                    ContentPart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
            ConversationItem::System(s) => s.content.as_ref().to_owned(),
            _ => String::new(),
        }
    }

    #[test]
    fn a_plain_conversation_is_left_alone() {
        let items = vec![user("hello"), assistant_text("hi".to_owned())];
        assert!(!needs_flattening(&items));
        let (out, report) = flatten_conversation(items);
        assert_eq!(out.len(), 2);
        assert!(!report.changed_anything());
    }

    #[test]
    fn nothing_opaque_survives_a_flattening() {
        let mut encrypted = synthesized_reasoning_item("weighing the options");
        encrypted.encrypted_content = Some("enc_blob".to_owned());
        let items = vec![
            user("do the thing"),
            ConversationItem::Reasoning(encrypted),
            assistant_with_call("on it", "call_1", "read_file", r#"{"path":"/a"}"#),
            tool_result("call_1", "file contents"),
        ];
        assert!(needs_flattening(&items));

        let (out, report) = flatten_conversation(items);

        assert!(
            out.iter().all(|i| matches!(
                i,
                ConversationItem::User(_) | ConversationItem::Assistant(_)
            )),
            "only user and assistant messages may survive"
        );
        assert!(
            out.iter().all(|i| match i {
                ConversationItem::Assistant(a) => a.tool_calls.is_empty(),
                _ => true,
            }),
            "no tool call may survive"
        );
        assert_eq!(report.reasoning_to_text, 1);
        assert_eq!(report.tool_calls_to_text, 1);
        assert_eq!(report.tool_results_to_text, 1);
        assert!(report.changed_anything());
    }

    #[test]
    fn the_thinking_text_survives_as_an_assistant_message() {
        let items = vec![ConversationItem::Reasoning(synthesized_reasoning_item(
            "weighing the options",
        ))];
        let (out, _) = flatten_conversation(items);
        assert_eq!(out.len(), 1);
        assert_eq!(
            text_of(&out[0]),
            "<thinking>\nweighing the options\n</thinking>"
        );
    }

    #[test]
    fn an_encrypted_only_reasoning_item_is_dropped_and_counted() {
        let mut encrypted = synthesized_reasoning_item("");
        encrypted.summary.clear();
        encrypted.encrypted_content = Some("enc_blob".to_owned());
        let (out, report) = flatten_conversation(vec![ConversationItem::Reasoning(encrypted)]);
        assert!(out.is_empty(), "there was no text to carry across");
        assert_eq!(report.reasoning_dropped, 1);
        assert_eq!(report.reasoning_to_text, 0);
    }

    #[test]
    fn a_tool_call_keeps_its_name_and_arguments() {
        let items = vec![assistant_with_call(
            "reading it",
            "call_1",
            "read_file",
            r#"{"path":"/a"}"#,
        )];
        let (out, _) = flatten_conversation(items);
        let rendered = text_of(&out[0]);
        assert!(rendered.contains("reading it"), "{rendered}");
        assert!(rendered.contains("name=\"read_file\""), "{rendered}");
        assert!(rendered.contains(r#"{"path":"/a"}"#), "{rendered}");
    }

    #[test]
    fn a_flattened_assistant_message_claims_no_origin_model() {
        let items = vec![assistant_with_call("x", "call_1", "read_file", "{}")];
        let (out, _) = flatten_conversation(items);
        match &out[0] {
            ConversationItem::Assistant(a) => assert!(
                a.model_id.is_none(),
                "an origin would re-arm the signature rules"
            ),
            other => panic!("expected an assistant message, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_result_becomes_a_user_message_that_keeps_its_text() {
        let (out, _) = flatten_conversation(vec![tool_result("call_1", "file contents")]);
        match &out[0] {
            ConversationItem::User(u) => {
                assert_eq!(
                    u.synthetic_reason,
                    Some(SyntheticReason::HistoryFlattened),
                    "a flattened result is not something the user typed"
                );
            }
            other => panic!("expected a user message, got {other:?}"),
        }
        let rendered = text_of(&out[0]);
        assert!(rendered.contains("file contents"), "{rendered}");
        assert!(rendered.contains("tool_call_id=\"call_1\""), "{rendered}");
    }

    #[test]
    fn a_tool_result_keeps_the_images_it_returned() {
        let item = ConversationItem::ToolResult(ToolResultItem {
            tool_call_id: "call_1".to_owned(),
            content: Arc::<str>::from("a screenshot"),
            images: vec![ContentPart::Image {
                url: Arc::<str>::from("data:image/png;base64,AAAA"),
            }],
        });
        let (out, _) = flatten_conversation(vec![item]);
        match &out[0] {
            ConversationItem::User(u) => assert!(
                u.content
                    .iter()
                    .any(|p| matches!(p, ContentPart::Image { .. })),
                "an image is content the target reads, not provider state"
            ),
            other => panic!("expected a user message, got {other:?}"),
        }
    }

    #[test]
    fn the_system_prompt_is_untouched() {
        let items = vec![
            ConversationItem::System(crate::conversation::SystemItem {
                content: Arc::<str>::from("you are an agent"),
            }),
            tool_result("call_1", "x"),
        ];
        let (out, _) = flatten_conversation(items);
        assert!(matches!(out[0], ConversationItem::System(_)));
        assert_eq!(text_of(&out[0]), "you are an agent");
    }

    #[test]
    fn flattening_twice_changes_nothing_the_second_time() {
        let items = vec![
            user("go"),
            assistant_with_call("on it", "call_1", "read_file", "{}"),
            tool_result("call_1", "contents"),
        ];
        let (once, _) = flatten_conversation(items);
        assert!(!needs_flattening(&once));
        let (twice, report) = flatten_conversation(once.clone());
        assert!(!report.changed_anything());
        assert_eq!(once.len(), twice.len());
    }
}
