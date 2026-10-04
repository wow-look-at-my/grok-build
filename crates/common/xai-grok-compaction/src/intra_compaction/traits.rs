//! Trait abstractions for intra-compaction.

use async_trait::async_trait;

use super::trigger::IntraCompactionError;

/// Which segment of the conversation a single intra-compaction pass acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTarget {
    /// Compact the agent loop's accumulated step turns (assistant outputs, tool calls, tool results).
    Steps,
    /// Compact prior conversation-history turns (user/assistant exchanges from before the current agent loop).
    History,
    /// Replace the *whole* conversation (prior history + accumulated steps) with a single summary.
    FullReplace,
}

impl CompactionTarget {
    /// Stable metric label for this target.
    pub fn label(self) -> &'static str {
        match self {
            Self::Steps => "steps",
            Self::History => "history",
            Self::FullReplace => "full_replace",
        }
    }
}

/// Minimal interface the compaction orchestrator needs from the agent's stream processor. Implemented by Grok chat's `StreamProcessor` (`Item = Arc<GrokTurn>`). Read-views are exposed: - **Accumulated step turns**: items added since the agent loop started
///   — assistant outputs, tool calls, tool results, recovery turns. The
///   original conversation (system prompt, user messages, prior history)
///   is excluded. Used by step (fine-grained) compaction.
/// - **History turns**: items from prior user-query/assistant-response
///   exchanges, before the current agent loop began. Used by history
///   (coarse) compaction.
#[async_trait]
pub trait CompactionStreamProc: Send + Sync {
    /// The harness's conversation item type.
    type Item;

    /// Get the items accumulated across all completed steps, oldest first. Candidates for **steps** compaction.
    async fn get_accumulated_turns_for_compaction(&self) -> Vec<Self::Item>;

    /// Get the conversation-history items (prior user/assistant exchanges
    /// from before the current agent loop), oldest first.
    async fn get_history_turns_for_compaction(&self) -> Vec<Self::Item> {
        Vec::new()
    }

    /// Get the **whole** conversation — prior history followed by the
    /// accumulated step turns, oldest first. Candidates for **full-replace**
    /// (`CompactionTarget::FullReplace`) compaction.
    async fn get_all_turns_for_compaction(&self) -> Vec<Self::Item>
    where
        Self::Item: Send,
    {
        let mut all = self.get_history_turns_for_compaction().await;
        all.extend(self.get_accumulated_turns_for_compaction().await);
        all
    }

    /// Top-level intra-compaction mutator. Replaces the first
    /// `n_turns_to_remove` items in the read-view selected by `target` with
    /// the given `compaction_turn`.
    async fn replace_with_compaction(
        &self,
        target: CompactionTarget,
        n_turns_to_remove: usize,
        compaction_turn: Self::Item,
    ) -> Result<(), IntraCompactionError>;
}
