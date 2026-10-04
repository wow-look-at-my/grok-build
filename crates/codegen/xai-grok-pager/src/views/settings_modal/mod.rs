//! Settings modal: opens via F2, `/settings`, command palette, and shortcuts-help.

mod input;
mod render;
mod state;

#[cfg(test)]
mod tests;

pub use input::{handle_settings_key, handle_settings_mouse, handle_settings_paste};
pub use render::{ResetConfirmOverlay, render_settings_modal};
#[allow(unused_imports)] // re-export for crate path; used by settings/registry tests
pub(crate) use state::MAX_PICKER_CHOICES;
pub use state::{MODAL_TITLE, RowEntry, SettingsKeyOutcome, SettingsModalMode, SettingsModalState};
