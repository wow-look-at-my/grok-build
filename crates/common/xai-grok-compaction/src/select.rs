//! Turn selection for compaction.

use crate::item::CompactionItem;

/// Output of [`select_turns_to_compact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPlan {
    /// Compact items at indices `0..split_idx`. Keep `split_idx..total`.
    pub split_idx: usize,
    /// Sum of `item_token_counts[..split_idx]`.
    pub tokens_to_compact: u32,
}

/// Decide where to split the items for compaction. Walk backward from the newest item, accumulating "keep" tokens. The candidate split index is the first one where adding more would
///    exceed `target_tokens`.
/// 3. **Snap forward** to a safe boundary: if the split would orphan tool
///    results, walk forward until past the matching tool-result items.
///    below `min_compactable` — not worth running the LLM.
/// # Tool-pair boundary safety `items` is the agent's running state. A typical sequence: ```text [Assistant(tool_request_A, tool_request_B),
///  Tool(A_result),
///  Tool(B_result),
///  Assistant(response_text),
///  Assistant(tool_request_C),
///  Tool(C_result),
///  ...]
/// ``` A safe split point is one where everything **before** the split is self-contained (no dangling tool requests waiting for results that live after the split).
pub fn select_turns_to_compact<T: CompactionItem>(
    item_token_counts: &[u32],
    items: &[T],
    target_tokens: u32,
    min_compactable: u32,
) -> Option<SplitPlan> {
    debug_assert_eq!(
        item_token_counts.len(),
        items.len(),
        "token counts and items must have the same length"
    );

    let total = items.len();
    if total == 0 {
        return None;
    }

    let mut kept = 0u32;
    let mut split_idx = total; // start with "compact nothing", will move down
    for i in (0..total).rev() {
        let count = item_token_counts[i];
        if kept.saturating_add(count) > target_tokens {
            // Adding this item would exceed the budget — split here.
            split_idx = i + 1;
            break;
        }
        kept = kept.saturating_add(count);
        split_idx = i;
    }

    // If the whole list fits within the budget, nothing to compact.
    if split_idx == 0 {
        return None;
    }

    let safe_split_idx = snap_to_safe_boundary(items, split_idx);

    // After snapping forward we might have eaten everything.
    if safe_split_idx >= total {
        return None;
    }

    let tokens_to_compact: u32 = item_token_counts[..safe_split_idx]
        .iter()
        .copied()
        .fold(0u32, u32::saturating_add);

    if tokens_to_compact < min_compactable {
        return None;
    }

    Some(SplitPlan {
        split_idx: safe_split_idx,
        tokens_to_compact,
    })
}

/// If `candidate` lands on a tool-result item, advance forward past all
/// tool-result items in the same tool-pair run. The "run" is delimited by the
/// assistant item (with tool requests) and the next non-tool item.
fn snap_to_safe_boundary<T: CompactionItem>(items: &[T], candidate: usize) -> usize {
    let total = items.len();
    if candidate >= total {
        return total;
    }

    // If candidate is not a tool-result item, no snap needed.
    if !items[candidate].is_tool_result() {
        return candidate;
    }

    // Candidate is a tool-result item.
    let mut idx = candidate;
    while idx < total && items[idx].is_tool_result() {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::CompactionRole;

    /// Minimal mock implementing [`CompactionItem`] for selection tests.
    struct MockItem {
        role: CompactionRole,
    }

    impl MockItem {
        fn user() -> Self {
            Self {
                role: CompactionRole::User,
            }
        }
        fn assistant() -> Self {
            Self {
                role: CompactionRole::Assistant,
            }
        }
        fn tool() -> Self {
            Self {
                role: CompactionRole::Tool,
            }
        }
    }

    impl CompactionItem for MockItem {
        fn role(&self) -> CompactionRole {
            self.role
        }
        fn text(&self) -> Option<String> {
            None
        }
        fn has_tool_requests(&self) -> bool {
            false
        }
        fn is_compaction_summary(&self) -> bool {
            false
        }
        fn attachment_refs(&self) -> Vec<crate::item::CompactionFileRef> {
            Vec::new()
        }
    }

    #[test]
    fn empty_returns_none() {
        let items: Vec<MockItem> = vec![];
        assert!(select_turns_to_compact(&[], &items, 100, 10).is_none());
    }

    #[test]
    fn all_fits_in_budget_returns_none() {
        let items = vec![MockItem::user(), MockItem::assistant()];
        let counts = vec![10, 20];
        assert!(select_turns_to_compact(&counts, &items, 1000, 5).is_none());
    }

    #[test]
    fn splits_at_correct_index() {
        let items = vec![
            MockItem::user(),
            MockItem::assistant(),
            MockItem::user(),
            MockItem::assistant(),
        ];
        let counts = vec![40, 30, 20, 10];
        let plan = select_turns_to_compact(&counts, &items, 30, 5).expect("should split");
        assert_eq!(plan.split_idx, 2);
        assert_eq!(plan.tokens_to_compact, 70);
    }

    #[test]
    fn below_min_compactable_returns_none() {
        let items = vec![MockItem::user(), MockItem::assistant()];
        let counts = vec![5, 100];
        assert!(select_turns_to_compact(&counts, &items, 50, 10).is_none());
    }

    #[test]
    fn snaps_past_tool_results() {
        // Layout: [User, Assistant-text, Assistant-with-tools, Tool, Tool, Assistant-text]
        // If the naïve split lands on a Tool, snap forward past all Tools.
        let items = vec![
            MockItem::user(),
            MockItem::assistant(),
            MockItem::assistant(), // pretend this had tool_requests
            MockItem::tool(),
            MockItem::tool(),
            MockItem::assistant(),
        ];
        let counts = vec![10, 10, 10, 50, 50, 10];

        let plan = select_turns_to_compact(&counts, &items, 60, 5).expect("should split");
        assert_eq!(plan.split_idx, 5);
        assert_eq!(plan.tokens_to_compact, 10 + 10 + 10 + 50 + 50);
    }

    #[test]
    fn snap_does_not_advance_when_already_safe() {
        let items = vec![
            MockItem::user(),
            MockItem::assistant(),
            MockItem::user(), // safe split here
            MockItem::assistant(),
        ];
        let counts = vec![50, 50, 10, 10];
        let plan = select_turns_to_compact(&counts, &items, 30, 5).expect("should split");
        assert_eq!(plan.split_idx, 2);
    }

    #[test]
    fn snap_walks_to_end_returns_none() {
        // Pathological: split would need to snap past all items.
        let items = vec![MockItem::assistant(), MockItem::tool(), MockItem::tool()];
        let counts = vec![10, 50, 50];
        assert!(select_turns_to_compact(&counts, &items, 0, 5).is_none());
    }
}
