//! Data abstraction — the `CompactionItem` seam.

/// Harness-agnostic role of a single conversation item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionRole {
    /// System prompt.
    System,
    /// Developer prompt (Grok chat) — maps to System on harnesses without a distinct developer role.
    Developer,
    /// A user message.
    User,
    /// An assistant output (may carry tool requests).
    Assistant,
    /// A tool result.
    Tool,
}

/// A file attached to a user item, as seen by the shared user-query
/// extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionFileRef {
    /// Stable unique id of the attachment source.
    pub id: String,
    /// Human-readable file name.
    pub name: String,
}

/// Contract: one turn/item in a conversation, as seen by the shared
/// compaction algorithms.
///
/// Implementors:
/// - Grok chat: `GrokTurn`
/// - grok-build: `ConversationItem`
pub trait CompactionItem {
    /// The harness-agnostic role of this item.
    fn role(&self) -> CompactionRole;

    /// The item's text content, if any. Tool results and assistant tool-only turns may have no text.
    fn text(&self) -> Option<String>;

    /// Whether this item is a tool result.
    fn is_tool_result(&self) -> bool {
        matches!(self.role(), CompactionRole::Tool)
    }

    /// Whether this (assistant) item carries at least one tool request. `false` for all non-assistant items.
    fn has_tool_requests(&self) -> bool;

    /// Whether this item carries a *prior compaction summary*.
    fn is_compaction_summary(&self) -> bool;

    /// File attachments on a (user) item, for the `<grok_file>` lines in the `<grok_user_queries>` preamble.
    fn attachment_refs(&self) -> Vec<CompactionFileRef>;
}

/// Constructive extension of [`CompactionItem`] for algorithms that rebuild
/// items (history filtering and summary-carrier construction).
///
/// Not object-safe (`compaction_summary_item` has no receiver) — always used
/// through generics, never as `dyn`.
pub trait CompactionItemBuilder: CompactionItem + Clone {
    /// Construct the item that carries a compaction summary back into the conversation.
    fn compaction_summary_item(text: String) -> Self;

    /// Rebuild this item keeping only user-visible content.
    fn strip_tool_content(&self) -> Option<Self>;

    /// Truncate this item's payload for **summarizer input** to roughly
    /// `max_tokens`.
    fn truncate_payload_for_compaction(&self, max_tokens: u32) -> Self {
        let _ = max_tokens;
        self.clone()
    }
}

/// Write seam for the full-replace **assembler**
/// ([`crate::code_compaction::assemble::assemble_compacted_history`]):
/// constructs the typed harness items that make up grok-build's rebuilt
/// history.
pub trait CompactionItemFactory: Sized {
    /// A real user message (used for the last user query).
    fn new_user(text: String) -> Self;
    /// A synthetic user message carrying compaction metadata (user-info prefix, summary carrier).
    fn new_user_meta(text: String) -> Self;
    /// A user message carrying project instructions (AGENTS.md).
    fn new_project_instructions(text: String) -> Self;
    /// A synthetic user message carrying a `<system-reminder>` block.
    fn new_system_reminder(text: String) -> Self;
}

/// Forward [`CompactionItem`] through shared references so the algorithms can
/// operate over `&[Arc<T>]` (Grok chat stores turns as `Arc<GrokTurn>`).
impl<T: CompactionItem + ?Sized> CompactionItem for std::sync::Arc<T> {
    fn role(&self) -> CompactionRole {
        (**self).role()
    }
    fn text(&self) -> Option<String> {
        (**self).text()
    }
    fn is_tool_result(&self) -> bool {
        (**self).is_tool_result()
    }
    fn has_tool_requests(&self) -> bool {
        (**self).has_tool_requests()
    }
    fn is_compaction_summary(&self) -> bool {
        (**self).is_compaction_summary()
    }
    fn attachment_refs(&self) -> Vec<CompactionFileRef> {
        (**self).attachment_refs()
    }
}

/// Forward [`CompactionItemBuilder`] through `Arc` — rebuilt items are
/// wrapped in a fresh `Arc`, untouched items are *not* deep-cloned (the
/// shared filters clone the `Arc` pointer directly).
impl<T: CompactionItemBuilder> CompactionItemBuilder for std::sync::Arc<T> {
    fn compaction_summary_item(text: String) -> Self {
        std::sync::Arc::new(T::compaction_summary_item(text))
    }
    fn strip_tool_content(&self) -> Option<Self> {
        (**self).strip_tool_content().map(std::sync::Arc::new)
    }
    fn truncate_payload_for_compaction(&self, max_tokens: u32) -> Self {
        std::sync::Arc::new((**self).truncate_payload_for_compaction(max_tokens))
    }
}
