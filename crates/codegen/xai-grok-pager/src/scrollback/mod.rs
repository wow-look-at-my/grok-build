//! Scrollback: conversation display with blocks, scroll, selection, turns.

pub mod block;
pub mod blocks;
pub mod entry;
pub mod export;
pub mod layout;
pub mod link_map;
pub mod render;
pub mod scrollback_pane;
pub mod search;
pub mod selection;
pub mod state;
pub mod sticky;
pub mod table_geometry;
pub mod text_selection;
pub mod types;
pub mod wrappers;

// Re-exports for convenience
pub use block::{BlockContent, RenderBlock};
pub use blocks::{
    AgentMessageBlock, SystemMessageBlock, ThinkingBlock, ToolCallBlock, UserPromptBlock,
};
pub use entry::{EntryId, ScrollbackEntry};
pub use layout::HorizontalLayout;
pub use link_map::{VisibleLink, VisibleLinkMap};
pub use render::ScratchBuffer;
pub use scrollback_pane::ScrollbackPane;
pub use search::{ScrollbackMatch, ScrollbackSearchIndex, ScrollbackSearchState};
pub use selection::{RenderOutput, SelectionBox};
pub use state::{EntryLayoutInfo, ScrollbackState};
pub use text_selection::*;
pub use types::*;
