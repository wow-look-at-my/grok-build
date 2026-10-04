use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnStatus {
    #[default]
    Running,
    Completed,
    Failed,
    /// Cancelled by the user, not by the agent or an error.
    Cancelled,
}

/// Navigation direction for block selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NavDirection {
    /// Moving down (j key): the top of the block shows first.
    #[default]
    Down,
    /// Moving up (k key): the bottom of the block shows first.
    Up,
}

/// A turn in the conversation (the user prompt and every response until the next prompt).
#[derive(Debug, Clone)]
pub struct Turn {
    /// Index of the UserPrompt entry that starts this turn.
    pub prompt_index: usize,
    /// Index past the last entry (exclusive, like Range).
    pub end_index: usize,
    pub status: TurnStatus,
}

impl Turn {
    pub fn range(&self) -> Range<usize> {
        self.prompt_index..self.end_index
    }

    pub fn len(&self) -> usize {
        self.end_index - self.prompt_index
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Show all turns in a single timeline.
    #[default]
    AllTurns,
    SingleTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportSnapshot {
    pub(crate) scroll_offset: usize,
    pub(crate) follow_mode: bool,
    pub(crate) follow_preserve_scroll: bool,
    pub(crate) follow_preserve_content_generation: u64,
    pub(crate) viewport_height: u16,
    pub(crate) last_width: u16,
    pub(crate) selected: Option<usize>,
    pub(crate) current_turn: Option<usize>,
    pub(crate) view_mode: ViewMode,
    pub(crate) total_height: usize,
}

/// Maximum truncated header height for AllTurns sticky headers.
pub(super) const MAX_TRUNCATED_HEADER_HEIGHT: u16 = 6;

/// Duration (ms) an entry's accent stays bright after finishing.
pub const FINISH_FLASH_DURATION_MS: u64 = 400;

/// Extra entries measured EXACTLY beyond the visible viewport edge when settling lazy heights.
pub(super) const MEASURE_MARGIN_ENTRIES: usize = 8;

/// Entries kept (not swept) on each side of the measurement window when evicting off-screen render caches.
pub(super) const EVICT_KEEP_MARGIN_ENTRIES: usize = 128;

/// On a bottom-pinned full rebuild (resume) we eagerly measure this many pages of entries ABOVE the viewport.
pub(super) const RESUME_WARM_PAGES: u16 = 3;

/// Per-entry layout info, cached for rendering and navigation.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntryLayoutInfo {
    /// Rendered height at current width.
    pub height: u16,
    pub gap_after: u16,
    /// When non-zero, this entry renders as a group header instead of its normal block content.
    pub group_header_count: u16,
    /// When true, this entry renders as an expanded-group collapse header.
    pub group_collapse_header: bool,
    /// When true, this entry heads a verb-group run.
    pub verb_group_header: bool,
}

impl EntryLayoutInfo {
    /// Whether this entry renders as any kind of group header (N-more
    /// truncation, expanded-group collapse, or verb) in place.
    pub fn is_group_header(&self) -> bool {
        self.group_header_count > 0 || self.group_collapse_header
    }

    pub fn is_expanded_verb_header(&self) -> bool {
        self.verb_group_header && self.group_collapse_header
    }

    /// Add the synthetic verb-run header row to a member's own measured height.
    pub fn with_verb_header_row(&self, member_height: u16) -> u16 {
        member_height.saturating_add(u16::from(self.is_expanded_verb_header()))
    }
}
