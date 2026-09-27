use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;
use crate::views::prompt_widget::StashedPrompt;

/// One rewindable prompt, as the agent's `x.ai/rewind/points` reply names it.
///
/// Every field reads under both the snake_case key this program names and the
/// camelCase key the agent writes; a reply carrying both spellings of one field
/// folds them rather than failing the whole reply on a duplicate field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "RewindPointInfoWire")]
pub struct RewindPointInfo {
    pub prompt_index: usize,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub num_file_snapshots: usize,
    #[serde(default)]
    pub prompt_preview: Option<String>,
    #[serde(default)]
    pub has_file_changes: bool,
}

impl RewindPointInfo {
    pub const PROMPT_INDEX_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("prompt_index", &["promptIndex"]);
    pub const CREATED_AT_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("created_at", &["createdAt"]);
    pub const NUM_FILE_SNAPSHOTS_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("num_file_snapshots", &["numFileSnapshots"]);
    pub const PROMPT_PREVIEW_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("prompt_preview", &["promptPreview"]);
    pub const HAS_FILE_CHANGES_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("has_file_changes", &["hasFileChanges"]);
}

/// `RewindPointInfo` with each key spelling as its own field.
#[derive(Debug, Default, serde::Deserialize)]
struct RewindPointInfoWire {
    #[serde(default)]
    prompt_index: Option<usize>,
    #[serde(default, rename = "promptIndex")]
    prompt_index_camel: Option<usize>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default, rename = "createdAt")]
    created_at_camel: Option<String>,
    #[serde(default)]
    num_file_snapshots: Option<usize>,
    #[serde(default, rename = "numFileSnapshots")]
    num_file_snapshots_camel: Option<usize>,
    #[serde(default)]
    prompt_preview: Option<String>,
    #[serde(default, rename = "promptPreview")]
    prompt_preview_camel: Option<String>,
    #[serde(default)]
    has_file_changes: Option<bool>,
    #[serde(default, rename = "hasFileChanges")]
    has_file_changes_camel: Option<bool>,
}

impl TryFrom<RewindPointInfoWire> for RewindPointInfo {
    type Error = RewindPayloadError;

    fn try_from(wire: RewindPointInfoWire) -> Result<Self, Self::Error> {
        Ok(Self {
            prompt_index: required(
                RewindPointInfo::PROMPT_INDEX_KEYS
                    .fold(vec![wire.prompt_index, wire.prompt_index_camel])?,
                "prompt_index",
            )?,
            created_at: RewindPointInfo::CREATED_AT_KEYS
                .fold(vec![wire.created_at, wire.created_at_camel])?
                .unwrap_or_default(),
            num_file_snapshots: RewindPointInfo::NUM_FILE_SNAPSHOTS_KEYS
                .fold(vec![wire.num_file_snapshots, wire.num_file_snapshots_camel])?
                .unwrap_or_default(),
            prompt_preview: RewindPointInfo::PROMPT_PREVIEW_KEYS
                .fold(vec![wire.prompt_preview, wire.prompt_preview_camel])?,
            has_file_changes: RewindPointInfo::HAS_FILE_CHANGES_KEYS
                .fold(vec![wire.has_file_changes, wire.has_file_changes_camel])?
                .unwrap_or(false),
        })
    }
}

/// The `x.ai/rewind/points` reply. See [`RewindPointInfo`]'s note on keys.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "RewindPointsResponseWire")]
pub struct RewindPointsResponse {
    pub rewind_points: Vec<RewindPointInfo>,
}

impl RewindPointsResponse {
    pub const REWIND_POINTS_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("rewind_points", &["rewindPoints"]);
}

/// `RewindPointsResponse` with each key spelling as its own field.
#[derive(Debug, serde::Deserialize)]
struct RewindPointsResponseWire {
    #[serde(default)]
    rewind_points: Option<Vec<RewindPointInfo>>,
    #[serde(default, rename = "rewindPoints")]
    rewind_points_camel: Option<Vec<RewindPointInfo>>,
}

impl TryFrom<RewindPointsResponseWire> for RewindPointsResponse {
    type Error = RewindPayloadError;

    fn try_from(wire: RewindPointsResponseWire) -> Result<Self, Self::Error> {
        Ok(Self {
            rewind_points: required(
                RewindPointsResponse::REWIND_POINTS_KEYS
                    .fold(vec![wire.rewind_points, wire.rewind_points_camel])?,
                "rewind_points",
            )?,
        })
    }
}

/// The `x.ai/rewind/execute` reply. See [`RewindPointInfo`]'s note on keys.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "RewindResponseWire")]
pub struct RewindResponse {
    pub success: bool,
    #[serde(default)]
    pub target_prompt_index: usize,
    #[serde(default)]
    pub reverted_files: Vec<String>,
    #[serde(default)]
    pub clean_files: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<RewindConflictInfo>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub prompt_text: Option<String>,
}

impl RewindResponse {
    pub const TARGET_PROMPT_INDEX_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("target_prompt_index", &["targetPromptIndex"]);
    pub const REVERTED_FILES_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("reverted_files", &["revertedFiles"]);
    pub const CLEAN_FILES_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("clean_files", &["cleanFiles"]);
    pub const PROMPT_TEXT_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("prompt_text", &["promptText"]);
}

/// `RewindResponse` with each key spelling as its own field.
#[derive(Debug, Default, serde::Deserialize)]
struct RewindResponseWire {
    success: bool,
    #[serde(default)]
    target_prompt_index: Option<usize>,
    #[serde(default, rename = "targetPromptIndex")]
    target_prompt_index_camel: Option<usize>,
    #[serde(default)]
    reverted_files: Option<Vec<String>>,
    #[serde(default, rename = "revertedFiles")]
    reverted_files_camel: Option<Vec<String>>,
    #[serde(default)]
    clean_files: Option<Vec<String>>,
    #[serde(default, rename = "cleanFiles")]
    clean_files_camel: Option<Vec<String>>,
    #[serde(default)]
    conflicts: Vec<RewindConflictInfo>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    prompt_text: Option<String>,
    #[serde(default, rename = "promptText")]
    prompt_text_camel: Option<String>,
}

impl TryFrom<RewindResponseWire> for RewindResponse {
    type Error = RewindPayloadError;

    fn try_from(wire: RewindResponseWire) -> Result<Self, Self::Error> {
        Ok(Self {
            success: wire.success,
            target_prompt_index: required(
                RewindResponse::TARGET_PROMPT_INDEX_KEYS.fold(vec![
                    wire.target_prompt_index,
                    wire.target_prompt_index_camel,
                ])?,
                "target_prompt_index",
            )?,
            reverted_files: RewindResponse::REVERTED_FILES_KEYS
                .fold(vec![wire.reverted_files, wire.reverted_files_camel])?
                .unwrap_or_default(),
            clean_files: RewindResponse::CLEAN_FILES_KEYS
                .fold(vec![wire.clean_files, wire.clean_files_camel])?
                .unwrap_or_default(),
            conflicts: wire.conflicts,
            error: wire.error,
            mode: wire.mode,
            prompt_text: RewindResponse::PROMPT_TEXT_KEYS
                .fold(vec![wire.prompt_text, wire.prompt_text_camel])?,
        })
    }
}

/// One file the rewind could not apply. See [`RewindPointInfo`]'s note on keys.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(try_from = "RewindConflictInfoWire")]
pub struct RewindConflictInfo {
    pub path: String,
    pub conflict_type: String,
}

impl RewindConflictInfo {
    pub const CONFLICT_TYPE_KEYS: xai_tool_types::Aliases =
        xai_tool_types::Aliases::new("conflict_type", &["conflictType"]);
}

/// `RewindConflictInfo` with each key spelling as its own field.
#[derive(Debug, serde::Deserialize)]
struct RewindConflictInfoWire {
    path: String,
    #[serde(default)]
    conflict_type: Option<String>,
    #[serde(default, rename = "conflictType")]
    conflict_type_camel: Option<String>,
}

impl TryFrom<RewindConflictInfoWire> for RewindConflictInfo {
    type Error = RewindPayloadError;

    fn try_from(wire: RewindConflictInfoWire) -> Result<Self, Self::Error> {
        Ok(Self {
            path: wire.path,
            conflict_type: required(
                RewindConflictInfo::CONFLICT_TYPE_KEYS
                    .fold(vec![wire.conflict_type, wire.conflict_type_camel])?,
                "conflict_type",
            )?,
        })
    }
}

/// Why an agent rewind payload could not be read.
#[derive(Debug)]
enum RewindPayloadError {
    Alias(xai_tool_types::AliasConflict),
    /// A field the view cannot render without. It stayed required before the
    /// shadows existed and stays required after them.
    MissingField(&'static str),
}

impl std::fmt::Display for RewindPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alias(conflict) => conflict.fmt(f),
            Self::MissingField(field) => write!(f, "missing field `{field}`"),
        }
    }
}

impl std::error::Error for RewindPayloadError {}

impl From<xai_tool_types::AliasConflict> for RewindPayloadError {
    fn from(value: xai_tool_types::AliasConflict) -> Self {
        Self::Alias(value)
    }
}

/// The value a required field must carry; its absence is the error serde's
/// derived impl used to raise.
fn required<T>(folded: Option<T>, field: &'static str) -> Result<T, RewindPayloadError> {
    folded.ok_or(RewindPayloadError::MissingField(field))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewindPhase {
    Loading,
    Picker {
        points: Vec<RewindPointInfo>,
        selected: usize,
    },
    CancelOffer {
        active_idx: usize,
    },
    /// Confirm before executing a conversation-only rewind.
    Confirm {
        target_prompt_index: usize,
        active_idx: usize,
        prompt_preview: Option<String>,
    },
    Executing {
        target_prompt_index: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Debug)]
pub struct RewindState {
    pub phase: RewindPhase,
    pub anchor_entry_idx: usize,
    pub stashed_draft: Option<StashedPrompt>,
    pub selected_prompt_index: Option<usize>,
}

impl RewindState {
    pub fn new_cancel_offer(
        anchor: usize,
        draft: Option<StashedPrompt>,
        selected_prompt_index: Option<usize>,
    ) -> Self {
        Self {
            phase: RewindPhase::CancelOffer { active_idx: 0 },
            anchor_entry_idx: anchor,
            stashed_draft: draft,
            selected_prompt_index,
        }
    }
}

pub enum RewindInput {
    Dismissed,
    CancelTurnThenProceed,
    DismissError,
    Confirm(usize),
    /// Execute this rewind and turn off confirm-before-rewind.
    ConfirmNeverAsk(usize),
    PickerSelect(usize),
    MoveUp,
    MoveDown,
    ConfirmCursor,
    Consumed,
}

const CANCEL_OFFER_OPTIONS: usize = 2;
/// Yes / Yes, and don't ask again / No.
const CONFIRM_OPTIONS: usize = 3;

pub fn handle_rewind_key(state: &RewindState, key: &KeyEvent) -> RewindInput {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return RewindInput::Consumed;
    }
    match &state.phase {
        RewindPhase::Picker { points, selected } => match key.code {
            KeyCode::Char('j') | KeyCode::Down => RewindInput::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => RewindInput::MoveUp,
            KeyCode::Enter => {
                if let Some(p) = points.get(*selected) {
                    RewindInput::PickerSelect(p.prompt_index)
                } else {
                    RewindInput::Consumed
                }
            }
            KeyCode::Esc => RewindInput::Dismissed,
            _ => RewindInput::Consumed,
        },
        RewindPhase::CancelOffer { .. } => match key.code {
            KeyCode::Char('y') => RewindInput::CancelTurnThenProceed,
            KeyCode::Char('n') => RewindInput::Dismissed,
            KeyCode::Char('j') | KeyCode::Down => RewindInput::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => RewindInput::MoveUp,
            KeyCode::Enter => RewindInput::ConfirmCursor,
            KeyCode::Esc => RewindInput::Dismissed,
            _ => RewindInput::Consumed,
        },
        RewindPhase::Confirm {
            target_prompt_index,
            ..
        } => match key.code {
            KeyCode::Char('y') => RewindInput::Confirm(*target_prompt_index),
            KeyCode::Char('n') => RewindInput::Dismissed,
            KeyCode::Char('a') => RewindInput::ConfirmNeverAsk(*target_prompt_index),
            KeyCode::Char('j') | KeyCode::Down => RewindInput::MoveDown,
            KeyCode::Char('k') | KeyCode::Up => RewindInput::MoveUp,
            KeyCode::Enter => RewindInput::ConfirmCursor,
            KeyCode::Esc => RewindInput::Dismissed,
            _ => RewindInput::Consumed,
        },
        RewindPhase::Error { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => RewindInput::DismissError,
            _ => RewindInput::Consumed,
        },
        RewindPhase::Loading => match key.code {
            KeyCode::Esc => RewindInput::Dismissed,
            _ => RewindInput::Consumed,
        },
        RewindPhase::Executing { .. } => RewindInput::Consumed,
    }
}

pub fn move_cursor(phase: &mut RewindPhase, delta: i32) {
    match phase {
        RewindPhase::Picker { points, selected } => {
            if points.is_empty() {
                return;
            }
            let max = points.len() as i32 - 1;
            let new = (*selected as i32 + delta).clamp(0, max);
            *selected = new as usize;
        }
        RewindPhase::CancelOffer { active_idx } => {
            let new = (*active_idx as i32 + delta).clamp(0, CANCEL_OFFER_OPTIONS as i32 - 1);
            *active_idx = new as usize;
        }
        RewindPhase::Confirm { active_idx, .. } => {
            let new = (*active_idx as i32 + delta).clamp(0, CONFIRM_OPTIONS as i32 - 1);
            *active_idx = new as usize;
        }
        _ => {}
    }
}

pub fn confirm_cursor(phase: &RewindPhase) -> RewindInput {
    match phase {
        RewindPhase::CancelOffer { active_idx } => match active_idx {
            0 => RewindInput::CancelTurnThenProceed,
            _ => RewindInput::Dismissed,
        },
        RewindPhase::Confirm {
            target_prompt_index,
            active_idx,
            ..
        } => match active_idx {
            0 => RewindInput::Confirm(*target_prompt_index),
            1 => RewindInput::ConfirmNeverAsk(*target_prompt_index),
            _ => RewindInput::Dismissed,
        },
        _ => RewindInput::Consumed,
    }
}

/// Hit-test a screen position against the rewind overlay's clickable rows.
///
/// Returns the logical cursor index under `(col, row)` for the current
/// phase, or `None` if the position is not on a selectable row.
///
/// IMPORTANT: the row geometry here mirrors `render_rewind_overlay`. Keep
/// this, `render_rewind_overlay`, and `rewind_overlay_height` in sync when
/// changing layout.
pub fn rewind_row_at(phase: &RewindPhase, area: Rect, col: u16, row: u16) -> Option<usize> {
    if area.height == 0 || area.width < 10 {
        return None;
    }
    if col < area.x || col >= area.x + area.width {
        return None;
    }
    if row < area.y || row >= area.y + area.height {
        return None;
    }
    match phase {
        RewindPhase::Picker { points, selected } => crate::views::overlay_list::ListOverlay {
            len: points.len(),
            selected: *selected,
        }
        .row_at(area, col, row),
        RewindPhase::CancelOffer { .. } => match row.checked_sub(area.y + 3) {
            Some(0) => Some(0),
            Some(1) => Some(1),
            _ => None,
        },
        RewindPhase::Confirm { .. } => match row.checked_sub(area.y + 2) {
            Some(0) => Some(0),
            Some(1) => Some(1),
            Some(2) => Some(2),
            _ => None,
        },
        RewindPhase::Error { .. } => {
            if row == area.y + 3 {
                Some(0)
            } else {
                None
            }
        }
        RewindPhase::Loading | RewindPhase::Executing { .. } => None,
    }
}

/// Move the overlay cursor/selection to `idx` (used by mouse hover/click).
/// Returns `true` if the stored cursor changed.
pub fn set_rewind_cursor(phase: &mut RewindPhase, idx: usize) -> bool {
    match phase {
        RewindPhase::Picker { points, selected } => {
            if points.is_empty() {
                return false;
            }
            let new = idx.min(points.len() - 1);
            if *selected != new {
                *selected = new;
                true
            } else {
                false
            }
        }
        RewindPhase::CancelOffer { active_idx } => {
            let new = idx.min(CANCEL_OFFER_OPTIONS - 1);
            if *active_idx != new {
                *active_idx = new;
                true
            } else {
                false
            }
        }
        RewindPhase::Confirm { active_idx, .. } => {
            let new = idx.min(CONFIRM_OPTIONS - 1);
            if *active_idx != new {
                *active_idx = new;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// The activation input for the current cursor position — equivalent to
/// pressing Enter on the focused row. Used by mouse-click handling.
pub fn rewind_activate(phase: &RewindPhase) -> RewindInput {
    match phase {
        RewindPhase::Picker { points, selected } => points
            .get(*selected)
            .map(|p| RewindInput::PickerSelect(p.prompt_index))
            .unwrap_or(RewindInput::Consumed),
        RewindPhase::Error { .. } => RewindInput::DismissError,
        other => confirm_cursor(other),
    }
}

pub fn rewind_overlay_height(phase: &RewindPhase, screen_h: u16) -> u16 {
    let content = match phase {
        RewindPhase::Loading => 2,
        RewindPhase::Picker { points, selected } => {
            return crate::views::overlay_list::ListOverlay {
                len: points.len(),
                selected: *selected,
            }
            .height(screen_h);
        }
        RewindPhase::CancelOffer { .. } => 5,
        RewindPhase::Executing { .. } => 2,
        RewindPhase::Confirm { .. } => 5,
        RewindPhase::Error { .. } => 4,
    };
    content + 1
}

pub fn render_rewind_overlay(buf: &mut Buffer, area: Rect, phase: &RewindPhase, focused: bool) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let theme = Theme::current();
    let bg = theme.bg_light;

    buf.set_style(area, Style::default().bg(bg));

    let accent_style = Style::default().fg(theme.accent_user);
    for row in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, row)) {
            cell.set_symbol(crate::glyphs::accent_bar());
            cell.set_style(accent_style);
        }
    }

    let content_x = area.x + 3;
    let content_w = area.width.saturating_sub(5);

    let title_style = Style::default()
        .fg(theme.accent_user)
        .add_modifier(Modifier::BOLD);

    match phase {
        RewindPhase::Loading => {
            let y = area.y + 1;
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(
                    "Loading rewind points...",
                    Style::default().fg(theme.gray),
                )),
                content_w,
            );
        }
        RewindPhase::Picker { points, selected } => {
            // Shared list-overlay chrome + row geometry (also used by /jump).
            // It applies the unfocus dim itself, so return before the shared
            // blend at the bottom of this function.
            crate::views::overlay_list::ListOverlay {
                len: points.len(),
                selected: *selected,
            }
            .render(buf, area, "Rewind to which turn?", focused, |i, ctx| {
                let point = &points[i];
                let dot_style = Style::default().fg(theme.gray).bg(ctx.row_bg);
                let preview: String = crate::render::line_utils::truncate_str(
                    point.prompt_preview.as_deref().unwrap_or("(no preview)"),
                    ctx.content_width.saturating_sub(8) as usize,
                );
                let text_style = Style::default()
                    .fg(theme.text_primary)
                    .bg(ctx.row_bg)
                    .add_modifier(if ctx.is_cursor {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    });

                Line::from(vec![
                    Span::styled("\u{00B7} ", dot_style),
                    Span::styled(preview, text_style),
                ])
            });
            return;
        }
        RewindPhase::CancelOffer { active_idx } => {
            let mut y = area.y + 1;
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled("A turn is currently running.", title_style)),
                content_w,
            );
            y += 1;
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(
                    "Would you like to cancel it before rewinding?",
                    Style::default().fg(theme.gray),
                )),
                content_w,
            );
            y += 1;
            render_radio_row(
                buf,
                content_x,
                y,
                content_w,
                'y',
                "Cancel turn and rewind",
                *active_idx == 0,
                focused,
                &theme,
            );
            y += 1;
            render_radio_row(
                buf,
                content_x,
                y,
                content_w,
                'n',
                "Let it finish",
                *active_idx == 1,
                focused,
                &theme,
            );
        }
        RewindPhase::Executing { .. } => {
            let y = area.y + 1;
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(
                    "Rewinding...",
                    Style::default().fg(theme.gray),
                )),
                content_w,
            );
        }
        RewindPhase::Confirm {
            active_idx,
            prompt_preview,
            ..
        } => {
            let mut y = area.y + 1;
            let preview_text = prompt_preview.as_deref().unwrap_or("this turn");
            let prefix = "Rewind conversation to \u{201C}";
            let suffix = "\u{201D}?";
            let chrome = prefix.chars().count() + suffix.chars().count();
            let max_preview = (content_w as usize).saturating_sub(chrome + 1);
            let preview_trunc: String = if preview_text.chars().count() > max_preview {
                let truncated: String = preview_text
                    .chars()
                    .take(max_preview.saturating_sub(1))
                    .collect();
                format!("{truncated}\u{2026}")
            } else {
                preview_text.to_string()
            };
            let title = format!("{prefix}{preview_trunc}{suffix}");
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(title, title_style)),
                content_w,
            );
            y += 1;
            render_radio_row(
                buf,
                content_x,
                y,
                content_w,
                'y',
                "Yes",
                *active_idx == 0,
                focused,
                &theme,
            );
            y += 1;
            render_radio_row(
                buf,
                content_x,
                y,
                content_w,
                'a',
                "Yes, and don't ask again",
                *active_idx == 1,
                focused,
                &theme,
            );
            y += 1;
            render_radio_row(
                buf,
                content_x,
                y,
                content_w,
                'n',
                "No",
                *active_idx == 2,
                focused,
                &theme,
            );
        }
        RewindPhase::Error { message } => {
            let mut y = area.y + 1;
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(
                    "Rewind failed",
                    Style::default()
                        .fg(theme.accent_error)
                        .add_modifier(Modifier::BOLD),
                )),
                content_w,
            );
            y += 1;
            let truncated: String = message.chars().take(content_w as usize).collect();
            buf.set_line(
                content_x,
                y,
                &Line::from(Span::styled(
                    truncated,
                    Style::default().fg(theme.text_primary),
                )),
                content_w,
            );
            y += 1;
            render_radio_row(
                buf, content_x, y, content_w, '\x1b', "Dismiss", true, focused, &theme,
            );
        }
    }

    // Unfocus dim: when the prompt area is unfocused (e.g. user moved
    // to scrollback), blend foregrounds toward `bg_light` so the panel
    // visually recedes. Mirrors the unfocused prompt widget pattern
    // (`prompt_widget.rs:1948`).
    if !focused {
        crate::render::color::blend_area(buf, area, Some((bg, 0.66)), None);
    }
}

/// Visible label for sentinel-encoded keys (`Esc`, `Bksp`).
fn key_label(key: char) -> String {
    match key {
        '\x1b' => "Esc".into(),
        '\x08' => "Bksp".into(),
        other => other.to_string(),
    }
}

fn render_radio_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    w: u16,
    key: char,
    label: &str,
    is_cursor: bool,
    panel_focused: bool,
    theme: &Theme,
) {
    let bg = if is_cursor && panel_focused {
        theme.bg_visual
    } else {
        theme.bg_light
    };

    let row_rect = Rect {
        x: x.saturating_sub(1),
        y,
        width: w + 2,
        height: 1,
    };
    buf.set_style(row_rect, Style::default().bg(bg));

    let marker = if is_cursor {
        crate::glyphs::filled_dot()
    } else {
        "\u{25CB}"
    };
    let key_display = key_label(key);

    let num_style = Style::default().fg(theme.accent_user).bg(bg);
    let marker_style = if is_cursor {
        Style::default().fg(theme.accent_user).bg(bg)
    } else {
        Style::default().fg(theme.gray).bg(bg)
    };
    let label_style = Style::default()
        .fg(theme.text_primary)
        .bg(bg)
        .add_modifier(if is_cursor {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });

    let line = Line::from(vec![
        Span::styled(format!("{key_display:<4}"), num_style),
        Span::styled(format!("({marker}) "), marker_style),
        Span::styled(label.to_string(), label_style),
    ]);
    buf.set_line(x, y, &line, w);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyModifiers};

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        }
    }

    fn point(prompt_index: usize) -> RewindPointInfo {
        RewindPointInfo {
            prompt_index,
            created_at: String::new(),
            num_file_snapshots: 0,
            prompt_preview: Some(format!("turn {prompt_index}")),
            has_file_changes: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        }
    }

    fn confirm_state() -> RewindState {
        RewindState {
            phase: RewindPhase::Confirm {
                target_prompt_index: 3,
                active_idx: 0,
                prompt_preview: None,
            },
            anchor_entry_idx: 0,
            stashed_draft: None,
            selected_prompt_index: Some(3),
        }
    }

    #[test]
    fn picker_row_hit_test_maps_to_point_index() {
        let phase = RewindPhase::Picker {
            points: vec![point(0), point(1), point(2)],
            selected: 0,
        };
        // Title is at y+1; rows start at y+2.
        assert_eq!(rewind_row_at(&phase, area(), 5, 1), None);
        assert_eq!(rewind_row_at(&phase, area(), 5, 2), Some(0));
        assert_eq!(rewind_row_at(&phase, area(), 5, 4), Some(2));
        // Past the last point.
        assert_eq!(rewind_row_at(&phase, area(), 5, 5), None);
        // Outside the overlay horizontally.
        assert_eq!(rewind_row_at(&phase, area(), 99, 2), None);
    }

    #[test]
    fn cancel_offer_rows() {
        let phase = RewindPhase::CancelOffer { active_idx: 0 };
        assert_eq!(rewind_row_at(&phase, area(), 5, 3), Some(0));
        assert_eq!(rewind_row_at(&phase, area(), 5, 4), Some(1));
        assert_eq!(rewind_row_at(&phase, area(), 5, 5), None);
    }

    #[test]
    fn confirm_rows() {
        let phase = RewindPhase::Confirm {
            target_prompt_index: 0,
            active_idx: 0,
            prompt_preview: None,
        };
        assert_eq!(rewind_row_at(&phase, area(), 5, 2), Some(0));
        assert_eq!(rewind_row_at(&phase, area(), 5, 3), Some(1));
        assert_eq!(rewind_row_at(&phase, area(), 5, 4), Some(2));
        assert_eq!(rewind_row_at(&phase, area(), 5, 5), None);
    }

    #[test]
    fn error_dismiss_row() {
        let phase = RewindPhase::Error {
            message: "boom".into(),
        };
        assert_eq!(rewind_row_at(&phase, area(), 5, 3), Some(0));
        assert_eq!(rewind_row_at(&phase, area(), 5, 2), None);
    }

    #[test]
    fn non_interactive_phases_have_no_rows() {
        for phase in [
            RewindPhase::Loading,
            RewindPhase::Executing {
                target_prompt_index: 0,
            },
        ] {
            for row in 0..10 {
                assert_eq!(rewind_row_at(&phase, area(), 5, row), None);
            }
        }
    }

    #[test]
    fn set_cursor_moves_and_clamps() {
        let mut phase = RewindPhase::Picker {
            points: vec![point(0), point(1)],
            selected: 0,
        };
        assert!(set_rewind_cursor(&mut phase, 1));
        assert!(!set_rewind_cursor(&mut phase, 1)); // no change
        // Clamp out-of-range to last point (already at last → no change).
        assert!(!set_rewind_cursor(&mut phase, 99));
        if let RewindPhase::Picker { selected, .. } = phase {
            assert_eq!(selected, 1);
        } else {
            panic!("expected picker");
        }

        let mut confirm = RewindPhase::Confirm {
            target_prompt_index: 0,
            active_idx: 0,
            prompt_preview: None,
        };
        set_rewind_cursor(&mut confirm, 2);
        if let RewindPhase::Confirm { active_idx, .. } = confirm {
            assert_eq!(active_idx, 2);
        } else {
            panic!("expected confirm");
        }
        set_rewind_cursor(&mut confirm, 99);
        if let RewindPhase::Confirm { active_idx, .. } = confirm {
            assert_eq!(active_idx, 2);
        } else {
            panic!("expected confirm");
        }
    }

    #[test]
    fn activate_matches_enter_semantics() {
        let picker = RewindPhase::Picker {
            points: vec![point(10), point(20)],
            selected: 1,
        };
        assert!(matches!(
            rewind_activate(&picker),
            RewindInput::PickerSelect(20)
        ));

        let error = RewindPhase::Error {
            message: "x".into(),
        };
        assert!(matches!(rewind_activate(&error), RewindInput::DismissError));

        let confirm_go = RewindPhase::Confirm {
            target_prompt_index: 4,
            active_idx: 0,
            prompt_preview: None,
        };
        assert!(matches!(
            rewind_activate(&confirm_go),
            RewindInput::Confirm(4)
        ));

        let confirm_never = RewindPhase::Confirm {
            target_prompt_index: 4,
            active_idx: 1,
            prompt_preview: None,
        };
        assert!(matches!(
            rewind_activate(&confirm_never),
            RewindInput::ConfirmNeverAsk(4)
        ));

        let confirm_no = RewindPhase::Confirm {
            target_prompt_index: 4,
            active_idx: 2,
            prompt_preview: None,
        };
        assert!(matches!(
            rewind_activate(&confirm_no),
            RewindInput::Dismissed
        ));
    }

    #[test]
    fn confirm_letter_keys() {
        let state = confirm_state();
        assert!(matches!(
            handle_rewind_key(&state, &key(KeyCode::Char('y'))),
            RewindInput::Confirm(3)
        ));
        assert!(matches!(
            handle_rewind_key(&state, &key(KeyCode::Char('n'))),
            RewindInput::Dismissed
        ));
        assert!(matches!(
            handle_rewind_key(&state, &key(KeyCode::Char('a'))),
            RewindInput::ConfirmNeverAsk(3)
        ));
    }

    #[test]
    fn esc_dismisses_from_confirm() {
        let state = confirm_state();
        assert!(matches!(
            handle_rewind_key(&state, &key(KeyCode::Esc)),
            RewindInput::Dismissed
        ));
    }

    #[test]
    fn backspace_ignored_on_confirm() {
        let state = confirm_state();
        assert!(matches!(
            handle_rewind_key(&state, &key(KeyCode::Backspace)),
            RewindInput::Consumed
        ));
    }

    #[test]
    fn esc_dismisses_from_picker_and_other_phases() {
        let s = RewindState {
            phase: RewindPhase::Picker {
                points: vec![],
                selected: 0,
            },
            anchor_entry_idx: 0,
            stashed_draft: None,
            selected_prompt_index: None,
        };
        assert!(matches!(
            handle_rewind_key(&s, &key(KeyCode::Esc)),
            RewindInput::Dismissed
        ));

        let s = RewindState::new_cancel_offer(0, None, None);
        assert!(matches!(
            handle_rewind_key(&s, &key(KeyCode::Esc)),
            RewindInput::Dismissed
        ));

        let s = RewindState {
            phase: RewindPhase::Loading,
            anchor_entry_idx: 0,
            stashed_draft: None,
            selected_prompt_index: None,
        };
        assert!(matches!(
            handle_rewind_key(&s, &key(KeyCode::Esc)),
            RewindInput::Dismissed
        ));
    }

    #[test]
    fn key_label_renders_special_sentinels() {
        assert_eq!(key_label('\x1b'), "Esc");
        assert_eq!(key_label('\x08'), "Bksp");
        assert_eq!(key_label('y'), "y");
        assert_eq!(key_label('a'), "a");
    }
}

#[cfg(test)]
mod payload_alias_tests {
    use super::{RewindConflictInfo, RewindPointInfo, RewindPointsResponse, RewindResponse};

    /// One rewind point, written with either key spelling — the agent replies in
    /// camelCase, and this program's own naming is snake_case.
    fn point_body(camel: bool) -> String {
        fn key<'a>(camel: bool, snake: &'a str, camel_case: &'a str) -> &'a str {
            if camel { camel_case } else { snake }
        }
        format!(
            concat!(
                "{{\"{}\":3,\"{}\":\"2026-01-01\",\"{}\":2,",
                "\"{}\":\"hello\",\"{}\":true}}"
            ),
            key(camel, "prompt_index", "promptIndex"),
            key(camel, "created_at", "createdAt"),
            key(camel, "num_file_snapshots", "numFileSnapshots"),
            key(camel, "prompt_preview", "promptPreview"),
            key(camel, "has_file_changes", "hasFileChanges"),
        )
    }

    /// The same point with every field named twice, each pair carrying one
    /// value — what a peer that renames on the way through produces.
    fn point_body_both_ways() -> String {
        let camel =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&point_body(true))
                .unwrap();
        let snake =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&point_body(false))
                .unwrap();
        let mut merged = camel;
        for (k, v) in snake {
            merged.insert(k, v);
        }
        serde_json::to_string(&merged).unwrap()
    }

    #[test]
    fn a_rewind_point_reads_from_either_key_spelling() {
        let from_camel: RewindPointInfo = serde_json::from_str(&point_body(true))
            .expect("the agent's camelCase shape must parse");
        let from_snake: RewindPointInfo =
            serde_json::from_str(&point_body(false)).expect("the snake_case shape must parse too");
        assert_eq!(
            (
                from_camel.prompt_index,
                from_camel.created_at.as_str(),
                from_camel.num_file_snapshots,
                from_camel.prompt_preview.as_deref(),
                from_camel.has_file_changes,
            ),
            (3, "2026-01-01", 2, Some("hello"), true)
        );
        assert_eq!(from_camel, from_snake);
    }

    #[test]
    fn a_rewind_point_naming_every_key_both_ways_parses_once() {
        let both: RewindPointInfo = serde_json::from_str(&point_body_both_ways())
            .expect("one value per field, under both keys, must not be a duplicate");
        assert_eq!(both.prompt_index, 3);
        assert_eq!(both.created_at, "2026-01-01");
        assert_eq!(both.num_file_snapshots, 2);
        assert_eq!(both.prompt_preview.as_deref(), Some("hello"));
        assert!(both.has_file_changes);
    }

    #[test]
    fn a_rewind_point_whose_two_spellings_disagree_is_an_error_naming_the_field() {
        for (canonical, camel, left, right) in [
            ("prompt_index", "promptIndex", "3", "4"),
            ("created_at", "createdAt", r#""a""#, r#""b""#),
            ("num_file_snapshots", "numFileSnapshots", "1", "2"),
            ("prompt_preview", "promptPreview", r#""a""#, r#""b""#),
            ("has_file_changes", "hasFileChanges", "true", "false"),
        ] {
            // `prompt_index` stays present for the other four cases: it is
            // required, and its own absence must not be the error under test.
            let json = if canonical == "prompt_index" {
                format!(r#"{{"{canonical}":{left},"{camel}":{right}}}"#)
            } else {
                format!(
                    r#"{{"prompt_index":3,"{canonical}":{left},"{camel}":{right}}}"#
                )
            };
            let err = serde_json::from_str::<RewindPointInfo>(&json)
                .expect_err("{json} names one field twice with different values");
            let message = err.to_string();
            assert!(message.contains(canonical), "{message}");
            assert!(message.contains(camel), "{message}");
        }
    }

    /// A required field the payload omits entirely stays required: the shadow
    /// must not turn a missing index into index 0.
    #[test]
    fn a_rewind_point_with_no_prompt_index_at_all_is_still_an_error() {
        for json in [r#"{"createdAt":"x"}"#, r#"{"created_at":"x"}"#] {
            let err = serde_json::from_str::<RewindPointInfo>(json)
                .expect_err("prompt_index has no default");
            assert!(err.to_string().contains("prompt_index"), "{err}");
        }
    }

    /// These four types read a reply and never write one, so the key they never
    /// emit is asserted from the wire side instead: nothing here round-trips.
    #[test]
    fn the_points_list_reads_from_either_key_spelling() {
        let camel: RewindPointsResponse = serde_json::from_str(r#"{"rewindPoints":[]}"#).unwrap();
        let snake: RewindPointsResponse = serde_json::from_str(r#"{"rewind_points":[]}"#).unwrap();
        assert!(camel.rewind_points.is_empty() && snake.rewind_points.is_empty());

        let both: RewindPointsResponse =
            serde_json::from_str(r#"{"rewind_points":[],"rewindPoints":[]}"#)
                .expect("one empty list named twice is one empty list");
        assert!(both.rewind_points.is_empty());

        let points = |i: usize| format!(r#"[{{"prompt_index":{i}}}]"#);
        let json = format!(
            r#"{{"rewind_points":{},"rewindPoints":{}}}"#,
            points(1),
            points(2)
        );
        let err = serde_json::from_str::<RewindPointsResponse>(&json)
            .expect_err("two different lists must not resolve silently");
        assert!(err.to_string().contains("rewind_points"), "{err}");
    }

    #[test]
    fn the_execute_reply_reads_from_either_key_spelling() {
        let camel: RewindResponse = serde_json::from_str(concat!(
            r#"{"success":true,"targetPromptIndex":7,"revertedFiles":["a"],"#,
            r#""cleanFiles":["b"],"promptText":"go"}"#,
        ))
        .unwrap();
        let snake: RewindResponse = serde_json::from_str(concat!(
            r#"{"success":true,"target_prompt_index":7,"reverted_files":["a"],"#,
            r#""clean_files":["b"],"prompt_text":"go"}"#,
        ))
        .unwrap();
        assert_eq!(camel.target_prompt_index, 7);
        assert_eq!(camel.reverted_files, vec!["a".to_owned()]);
        assert_eq!(camel.clean_files, vec!["b".to_owned()]);
        assert_eq!(camel.prompt_text.as_deref(), Some("go"));
        assert_eq!(snake.target_prompt_index, camel.target_prompt_index);
        assert_eq!(snake.prompt_text, camel.prompt_text);

        let both: RewindResponse = serde_json::from_str(concat!(
            r#"{"success":true,"target_prompt_index":7,"targetPromptIndex":7,"#,
            r#""reverted_files":["a"],"revertedFiles":["a"],"#,
            r#""clean_files":["b"],"cleanFiles":["b"],"#,
            r#""prompt_text":"go","promptText":"go"}"#,
        ))
        .expect("one value per field, under both keys");
        assert_eq!(both.target_prompt_index, 7);
        assert_eq!(both.reverted_files, vec!["a".to_owned()]);
        assert_eq!(both.clean_files, vec!["b".to_owned()]);
        assert_eq!(both.prompt_text.as_deref(), Some("go"));
    }

    #[test]
    fn the_execute_reply_whose_two_spellings_disagree_is_an_error_naming_the_field() {
        for (canonical, camel, left, right) in [
            ("target_prompt_index", "targetPromptIndex", "1", "2"),
            ("reverted_files", "revertedFiles", r#"["a"]"#, r#"["b"]"#),
            ("clean_files", "cleanFiles", r#"["a"]"#, r#"["b"]"#),
            ("prompt_text", "promptText", r#""a""#, r#""b""#),
        ] {
            // `success` and `target_prompt_index` stay present for the other
            // three cases: both are required, and their absence must not be the
            // error under test.
            let json = if canonical == "target_prompt_index" {
                format!(r#"{{"success":true,"{canonical}":{left},"{camel}":{right}}}"#)
            } else {
                format!(
                    r#"{{"success":true,"target_prompt_index":1,"{canonical}":{left},"{camel}":{right}}}"#
                )
            };
            let err = serde_json::from_str::<RewindResponse>(&json)
                .expect_err("{json} names one field twice with different values");
            assert!(err.to_string().contains(canonical), "{err}");
        }
    }

    #[test]
    fn a_conflict_reads_from_either_key_spelling_and_errors_on_a_conflict() {
        let camel: RewindConflictInfo =
            serde_json::from_str(r#"{"path":"a.rs","conflictType":"modified"}"#).unwrap();
        let snake: RewindConflictInfo =
            serde_json::from_str(r#"{"path":"a.rs","conflict_type":"modified"}"#).unwrap();
        assert_eq!(camel.conflict_type, "modified");
        assert_eq!(snake.conflict_type, camel.conflict_type);

        let both: RewindConflictInfo = serde_json::from_str(
            r#"{"path":"a.rs","conflict_type":"modified","conflictType":"modified"}"#,
        )
        .expect("one conflict, two spellings");
        assert_eq!(both.conflict_type, "modified");

        let err = serde_json::from_str::<RewindConflictInfo>(
            r#"{"path":"a.rs","conflict_type":"modified","conflictType":"deleted"}"#,
        )
        .expect_err("two kinds for one file must not resolve silently");
        assert!(err.to_string().contains("conflict_type"), "{err}");
    }
}
