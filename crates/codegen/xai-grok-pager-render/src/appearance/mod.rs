//! Hot-reloadable appearance configuration.

pub mod cache;
mod config;
pub mod follow_up_behavior;
pub mod permission_cursor;
pub mod render_mermaid;
pub mod scroll_mode;
pub mod text_selection;
mod watcher;

pub use config::{
    AnimationConfig, AppearanceConfig, BlockBackground, BlocksConfig, EditBlockConfig,
    ExecuteHeaderStyle, FollowIndicator, LayoutConfig, PromptConfig, PromptViewConfig,
    RawAltScreenMode, RawAppearanceConfig, RawTerminalConfig, ScrollConfig, ScrollbackConfig,
    ScrollbarConfig, ToolBullet, ToolConfig, persist_respect_manual_folds,
};
pub use follow_up_behavior::FollowUpBehavior;
pub use render_mermaid::RenderMermaid;
pub use scroll_mode::ScrollMode;
pub use text_selection::TextSelection;
pub use watcher::ConfigWatcher;

// Atomic so MarkdownContent can read tab width without an AppearanceConfig handle.

use std::sync::atomic::{AtomicU8, Ordering};

static TAB_WIDTH: AtomicU8 = AtomicU8::new(4);

/// Current tab expansion width (number of spaces per `\t`).
pub fn tab_width() -> u8 {
    TAB_WIDTH.load(Ordering::Relaxed)
}

/// Update the global tab width (called when config is loaded/reloaded).
pub fn set_tab_width(w: u8) {
    TAB_WIDTH.store(w, Ordering::Relaxed);
}

/// Replaces tabs with spaces at [`tab_width`].
/// This sits beside the width because every caller that paints a tab has to agree on it.
/// ratatui drops a cluster holding a control character, so a tab left in the text is deleted rather than drawn.
pub fn expand_tabs(text: &str) -> std::borrow::Cow<'_, str> {
    let width = tab_width();
    if width == 0 || !text.contains('\t') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace('\t', &" ".repeat(usize::from(width))))
}
