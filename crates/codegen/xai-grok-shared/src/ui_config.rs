use serde::{Deserialize, Serialize};
use xai_grok_config::DisplayRefreshSettings;

use xai_grok_status_line::StatusLineConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub max_thoughts_width: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// The `[models]` harness model slots the user set, keyed by slot id (`xai_grok_models::HARNESS_MODEL_SLOTS`).
    #[serde(skip)]
    pub harness_models: std::collections::BTreeMap<String, String>,
    /// YOLO mode. Read by `util::config`, declared here for `serde_ignored`.
    #[serde(default)]
    pub yolo: bool,
    /// UI theme alias. Read by `util::config`, declared here for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_theme: Option<String>,
    /// Read by pager, declared here for `serde_ignored`.
    #[serde(default)]
    pub compact_mode: bool,
    /// Read by pager, declared here for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simple_mode: Option<bool>,
    /// Read by `load_permission_mode()`. Declared for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Legacy name for `permission_mode`. Declared for `serde_ignored`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    /// Which permission option the cursor preselects on the first permission prompt of a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_selected_permission: Option<String>,
    /// Written by the pager's appearance persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_timestamps: Option<bool>,
    /// Timeline sidebar (per-turn tick rail in place of the scrollbar). `None` means off (client default; opt-in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_timeline: Option<bool>,
    /// The dashboard preview includes the selected session's reply panel. Unset means on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_preview: Option<bool>,
    /// Snap a just-sent prompt to the viewport top. `None` means on (default). Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_flip_on_send: Option<bool>,
    /// Ask before rewinding conversation history. `None` means on (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_before_rewind: Option<bool>,
    /// Gate the model's turn end on unfinished todos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_gate_unfinished_todos: Option<bool>,
    /// Gate the model's turn end on red CI for the branch it pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_gate_ci_failing: Option<bool>,
    /// Reissue a model call whose output rate stays under this many tokens per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_output_tokens_per_sec: Option<u32>,
    /// How long the rate must stay under the floor before the request is reissued. `None` = 10 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_rate_sustained_secs: Option<u32>,
    /// Trailing window the output rate is averaged over. `None` = 10 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_rate_window_secs: Option<u32>,
    /// How many times one model call is reissued for slow output before the response is accepted.
    #[serde(
        default,
        with = "xai_grok_config_types::retry_budget",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_rate_max_retries: Option<u32>,
    /// Reissue a model call that has produced no output this many seconds after the request was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_timeout_secs: Option<u32>,
    /// The most model requests this process sends at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel_requests: Option<u32>,
    /// Theme to use when the OS is in dark mode. Written by the pager's theme persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_dark_theme: Option<String>,
    /// Theme to use when the OS is in light mode. Written by the pager's theme persist module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_light_theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_speed: Option<u8>,
    /// Force scroll input classification (`auto` | `wheel` | `trackpad`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_mode: Option<String>,
    /// Invert vertical scroll direction ("natural" scrolling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_scroll: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_lines: Option<u8>,
    /// Vim-style scrollback navigation (hjkl, gg/G, /).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vim_mode: Option<bool>,
    /// How ` ```mermaid ` code blocks are rendered (`auto` | `on` | `off`). Written by the pager's settings modal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_mermaid: Option<String>,
    /// Hunk-tracker mode the pager advertises to the agent (`agent_only` | `all_dirty` | `off`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunk_tracker_mode: Option<String>,
    /// Voice capture chord behavior: `toggle` or `hold` (hold-to-talk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_capture_mode: Option<String>,
    /// Speech-to-text language preference for voice dictation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_stt_language: Option<String>,
    /// Whether the Ctrl+Space / F8 voice-dictation shortcut is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_keybind_enabled: Option<bool>,
    /// When `true`, registers `Ctrl+R` (while scrollback is focused) to toggle terminal mouse reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_reporting_toggle: Option<bool>,
    /// When cancelling a parent turn with running subagents: `always_stop` stops them without prompting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_subagents_on_turn_cancel: Option<String>,
    /// User knob for the `remember_tool_approvals` gate: per-tool "Always allow …" prompt options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_tool_approvals: Option<bool>,
    /// In-app drag selection highlight: `flash` | `hold` (legacy bool accepted).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_keep_text_selection"
    )]
    pub keep_text_selection: Option<String>,
    /// Legacy TTL ms; only `Some(0)` counts when `keep_text_selection` is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_highlight_duration_ms: Option<u64>,
    /// Show agent thinking/reasoning blocks in the TUI scrollback. `None` means on (client default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_thinking_blocks: Option<bool>,
    /// Summarize each thinking block and show the summary under its collapsed header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_summaries: Option<bool>,
    /// Fold runs of consecutive non-destructive tool calls (reads, searches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_tool_verbs: Option<bool>,
    /// Show Edit tool calls as a collapsed one-line `+N/-M` diffstat summary by default (expand for the diff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_edit_blocks: Option<bool>,
    /// Next-prompt suggestions (tab autocomplete ghost text) after each turn. `None` means on (client default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<bool>,
    /// Startup cursor style: `None` (default) inherits the terminal's own style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_blink: Option<bool>,
    /// `"fullscreen"` | `"minimal"`; unset uses the product default, fullscreen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_mode: Option<String>,
    /// Retired hidden opt-in for terminal-like double/triple-click word/line selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_click_action: Option<String>,
    /// Per-tip contextual-hint opt-outs (`[ui.contextual_hints]`).
    #[serde(default, skip_serializing_if = "ContextualHints::is_default")]
    pub contextual_hints: ContextualHints,
    /// Combine consecutive queued follow-ups into one turn. `None` means off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combine_queued_prompts: Option<bool>,
    /// Mid-turn follow-up routing: `"queue"` (default) or `"steer"`. `None` behaves as queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_behavior: Option<String>,
    /// Display-refresh probe and auto-cadence (`[ui.display_refresh]`).
    #[serde(default, skip_serializing_if = "DisplayRefreshSettings::is_default")]
    pub display_refresh: DisplayRefreshSettings,
    /// `[ui.status_line]`. Disabled by default.
    #[serde(default, skip_serializing_if = "status_line_should_not_be_saved")]
    pub status_line: StatusLineConfig,
}

fn status_line_should_not_be_saved(status_line: &StatusLineConfig) -> bool {
    status_line.is_default() || status_line.problem().is_some()
}

/// User-config opt-outs for the per-tip contextual hints, serialized as `[ui.contextual_hints]`.
/// Per-field `None` means "inherit remote/default"; `Some(bool)` is a user-explicit choice (needed so the resolver can let it beat the remote tier).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextualHints {
    /// Undo tip (Ctrl+Z after a substantial draft wipe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<bool>,
    /// Plan-mode nudge (typing a planning keyword).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<bool>,
    /// Clipboard-image input tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_input: Option<bool>,
    /// Send-now tip after queuing a mid-turn follow-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_now: Option<bool>,
    /// Small-screen tip (`/compact-mode` hint on smallish terminals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_screen: Option<bool>,
    /// Word-select tip after double-clicking scrollback while Text selection is still fold/nav (`flash` / `hold`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_select: Option<bool>,
    /// Export/copy tip after nearby drag-copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_copy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_wrap: Option<bool>,
}

impl ContextualHints {
    /// True when no tip has a user-explicit value (all inherit).
    /// Lets the section stay absent from `config.toml` until the user toggles a tip.
    pub fn is_default(&self) -> bool {
        self.undo.is_none()
            && self.plan_mode.is_none()
            && self.image_input.is_none()
            && self.send_now.is_none()
            && self.small_screen.is_none()
            && self.word_select.is_none()
            && self.export_copy.is_none()
            && self.ssh_wrap.is_none()
    }
}

const DEFAULT_MAX_THOUGHTS_WIDTH: u16 = 120;

fn deserialize_keep_text_selection<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Str(String),
    }

    Ok(
        Option::<Raw>::deserialize(deserializer)?.map(|raw| match raw {
            Raw::Bool(true) => "hold".to_string(),
            Raw::Bool(false) => "flash".to_string(),
            Raw::Str(s) => s,
        }),
    )
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            max_thoughts_width: DEFAULT_MAX_THOUGHTS_WIDTH,
            theme: None,
            harness_models: std::collections::BTreeMap::new(),
            yolo: false,
            ui_theme: None,
            compact_mode: false,
            simple_mode: None,
            permission_mode: None,
            approval_mode: None,
            default_selected_permission: None,
            show_timestamps: None,
            show_timeline: None,
            dashboard_preview: None,
            page_flip_on_send: None,
            confirm_before_rewind: None,
            stop_gate_unfinished_todos: None,
            stop_gate_ci_failing: None,
            min_output_tokens_per_sec: None,
            output_rate_sustained_secs: None,
            output_rate_window_secs: None,
            output_rate_max_retries: None,
            ttft_timeout_secs: None,
            max_parallel_requests: None,
            auto_dark_theme: None,
            auto_light_theme: None,
            scroll_speed: None,
            scroll_mode: None,
            invert_scroll: None,
            scroll_lines: None,
            vim_mode: None,
            render_mermaid: None,
            hunk_tracker_mode: None,
            voice_capture_mode: None,
            voice_stt_language: None,
            voice_keybind_enabled: None,
            mouse_reporting_toggle: None,
            remember_tool_approvals: None,
            cancel_subagents_on_turn_cancel: None,
            keep_text_selection: None,
            selection_highlight_duration_ms: None,
            show_thinking_blocks: None,
            thinking_summaries: None,
            group_tool_verbs: None,
            collapsed_edit_blocks: None,
            prompt_suggestions: None,
            cursor_blink: None,
            screen_mode: None,
            double_click_action: None,
            contextual_hints: ContextualHints::default(),
            combine_queued_prompts: None,
            follow_up_behavior: None,
            display_refresh: DisplayRefreshSettings::default(),
            status_line: StatusLineConfig::default(),
        }
    }
}

impl UiConfig {
    pub fn dashboard_preview_enabled(&self) -> bool {
        self.dashboard_preview.unwrap_or(true)
    }

    /// The source of truth for the timeline-sidebar default (opt-in).
    pub const SHOW_TIMELINE_DEFAULT: bool = false;

    /// Resolved timeline-sidebar setting: the configured value, or
    /// [`Self::SHOW_TIMELINE_DEFAULT`] when unset.
    pub fn show_timeline_enabled(&self) -> bool {
        self.show_timeline.unwrap_or(Self::SHOW_TIMELINE_DEFAULT)
    }

    /// Default for [`Self::page_flip_on_send`] when unset.
    pub const PAGE_FLIP_ON_SEND_DEFAULT: bool = true;

    pub fn page_flip_on_send_enabled(&self) -> bool {
        self.page_flip_on_send
            .unwrap_or(Self::PAGE_FLIP_ON_SEND_DEFAULT)
    }

    /// Default for [`Self::confirm_before_rewind`] when unset.
    pub const CONFIRM_BEFORE_REWIND_DEFAULT: bool = true;

    pub fn confirm_before_rewind_enabled(&self) -> bool {
        self.confirm_before_rewind
            .unwrap_or(Self::CONFIRM_BEFORE_REWIND_DEFAULT)
    }

    /// Default for [`Self::stop_gate_unfinished_todos`] when unset.
    pub const STOP_GATE_UNFINISHED_TODOS_DEFAULT: bool = true;

    pub fn stop_gate_unfinished_todos_enabled(&self) -> bool {
        self.stop_gate_unfinished_todos
            .unwrap_or(Self::STOP_GATE_UNFINISHED_TODOS_DEFAULT)
    }

    /// Default for [`Self::stop_gate_ci_failing`] when unset.
    pub const STOP_GATE_CI_FAILING_DEFAULT: bool = true;

    pub fn stop_gate_ci_failing_enabled(&self) -> bool {
        self.stop_gate_ci_failing
            .unwrap_or(Self::STOP_GATE_CI_FAILING_DEFAULT)
    }

    /// Default for [`Self::thinking_summaries`] when unset.
    pub const THINKING_SUMMARIES_DEFAULT: bool = true;

    pub fn thinking_summaries_enabled(&self) -> bool {
        self.thinking_summaries
            .unwrap_or(Self::THINKING_SUMMARIES_DEFAULT)
    }

    /// Default for [`Self::min_output_tokens_per_sec`] when unset.
    pub const MIN_OUTPUT_TOKENS_PER_SEC_DEFAULT: u32 = 15;

    /// Default for [`Self::output_rate_sustained_secs`] when unset.
    pub const OUTPUT_RATE_SUSTAINED_SECS_DEFAULT: u32 = 10;

    pub fn min_output_tokens_per_sec_value(&self) -> u32 {
        self.min_output_tokens_per_sec
            .unwrap_or(Self::MIN_OUTPUT_TOKENS_PER_SEC_DEFAULT)
    }

    pub fn output_rate_sustained_secs_value(&self) -> u32 {
        self.output_rate_sustained_secs
            .unwrap_or(Self::OUTPUT_RATE_SUSTAINED_SECS_DEFAULT)
    }

    /// Default for [`Self::output_rate_window_secs`] when unset.
    pub const OUTPUT_RATE_WINDOW_SECS_DEFAULT: u32 = 10;

    /// Default for [`Self::output_rate_max_retries`] when unset.
    pub const OUTPUT_RATE_MAX_RETRIES_DEFAULT: u32 = 2;

    pub fn output_rate_window_secs_value(&self) -> u32 {
        self.output_rate_window_secs
            .unwrap_or(Self::OUTPUT_RATE_WINDOW_SECS_DEFAULT)
    }

    pub fn output_rate_max_retries_value(&self) -> u32 {
        self.output_rate_max_retries
            .unwrap_or(Self::OUTPUT_RATE_MAX_RETRIES_DEFAULT)
    }

    /// Default for [`Self::ttft_timeout_secs`] when unset.
    pub const TTFT_TIMEOUT_SECS_DEFAULT: u32 = 120;

    pub fn ttft_timeout_secs_value(&self) -> u32 {
        self.ttft_timeout_secs
            .unwrap_or(Self::TTFT_TIMEOUT_SECS_DEFAULT)
    }

    /// Default for [`Self::max_parallel_requests`] when unset.
    pub const MAX_PARALLEL_REQUESTS_DEFAULT: u32 = 7;

    pub fn max_parallel_requests_value(&self) -> u32 {
        self.max_parallel_requests
            .unwrap_or(Self::MAX_PARALLEL_REQUESTS_DEFAULT)
    }

    /// Fill the `[ui]` window and retry budget from a legacy
    /// `[output_rate_floor]` table. A `[ui]` value wins where both are set.
    pub fn adopt_legacy_output_rate_floor(
        &mut self,
        window_secs: Option<u64>,
        max_retries: Option<u32>,
    ) {
        if self.output_rate_window_secs.is_none() {
            self.output_rate_window_secs =
                window_secs.map(|w| u32::try_from(w).unwrap_or(u32::MAX));
        }
        if self.output_rate_max_retries.is_none() {
            self.output_rate_max_retries = max_retries;
        }
    }

    /// Canonical default for `[ui].follow_up_behavior`.
    pub const FOLLOW_UP_BEHAVIOR_DEFAULT: &'static str = "queue";

    /// Resolved follow-up behavior: `"queue"` or `"steer"`.
    /// Unknown values fall back to queue.
    pub fn follow_up_behavior(&self) -> &'static str {
        match self.follow_up_behavior.as_deref() {
            Some("steer") => "steer",
            _ => Self::FOLLOW_UP_BEHAVIOR_DEFAULT,
        }
    }

    /// True when mid-turn follow-ups should promote as interjections (Steer).
    pub fn follow_up_steer_enabled(&self) -> bool {
        self.follow_up_behavior() == "steer"
    }

    pub fn keep_text_selection_enabled(&self) -> bool {
        if let Some(ref s) = self.keep_text_selection {
            return s == "hold" || s == "word_select";
        }
        matches!(self.selection_highlight_duration_ms, Some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The leniency lives in `StatusLineConfig`'s own `Deserialize`.
    /// This pins that the real `[ui]` table gets it, and that `skip_serializing_if` keeps a section we misread out of a save that merges per key.
    #[test]
    fn one_typo_in_the_status_line_cannot_fail_the_rest_of_the_ui_table() {
        let ui: UiConfig = serde_json::from_str(
            r#"{"theme": "kanagawa", "status_line": {"type": "builtin", "items": "cwd"}}"#,
        )
        .expect("[ui] must survive whatever the status line says");

        assert_eq!(ui.theme.as_deref(), Some("kanagawa"));
        assert!(ui.status_line.problem().is_some());

        let saved = serde_json::to_value(&ui).expect("[ui] serializes");
        assert_eq!(saved.get("theme"), Some(&serde_json::json!("kanagawa")));
        assert!(
            saved.get("status_line").is_none(),
            "a section we misread must not be written back over"
        );
    }

    /// A settings write merges per key, so a section the parse could not read in full must stay out of it.
    #[test]
    fn only_a_status_line_we_read_in_full_is_written_back() {
        for json in [
            r#"{"status_line": {"type": "enabled"}}"#,
            // A type this build removed reads like any other unknown one.
            r#"{"status_line": {"type": "static", "text": "hi"}}"#,
            r#"{"status_line": {"type": "builtin", "items": "cwd"}}"#,
            r#"{"status_line": "builtin"}"#,
            r#"{"status_line": {"padding": 2}}"#,
            "{}",
        ] {
            let ui: UiConfig = serde_json::from_str(json).expect("[ui] survives it");
            let saved = serde_json::to_value(&ui).expect("[ui] serializes");
            assert!(saved.get("status_line").is_none(), "{json}");
        }

        // An unknown key is preserved by the merge, so the section still persists
        // `off` is a spelling of `disabled`, so it is a choice that was read rather than a value that was not, and it saves as the canonical name
        for json in [
            r#"{"status_line": {"type": "command", "command": "x"}}"#,
            r#"{"status_line": {"type": "off"}}"#,
        ] {
            let ui: UiConfig = serde_json::from_str(json).expect("[ui] survives it");
            let saved = serde_json::to_value(&ui).expect("[ui] serializes");
            assert!(saved.get("status_line").is_some(), "{json}");
        }
    }

    #[test]
    fn page_flip_on_send_defaults_on() {
        assert!(UiConfig::default().page_flip_on_send_enabled());
        let off = UiConfig {
            page_flip_on_send: Some(false),
            ..Default::default()
        };
        assert!(!off.page_flip_on_send_enabled());
    }

    #[test]
    fn confirm_before_rewind_defaults_on() {
        assert!(UiConfig::default().confirm_before_rewind_enabled());
        let off = UiConfig {
            confirm_before_rewind: Some(false),
            ..Default::default()
        };
        assert!(!off.confirm_before_rewind_enabled());
    }

    #[test]
    fn stop_gate_unfinished_todos_defaults_on() {
        assert!(UiConfig::default().stop_gate_unfinished_todos_enabled());
        let off = UiConfig {
            stop_gate_unfinished_todos: Some(false),
            ..Default::default()
        };
        assert!(!off.stop_gate_unfinished_todos_enabled());
    }

    #[test]
    fn stop_gate_ci_failing_defaults_on() {
        assert!(UiConfig::default().stop_gate_ci_failing_enabled());
        let off = UiConfig {
            stop_gate_ci_failing: Some(false),
            ..Default::default()
        };
        assert!(!off.stop_gate_ci_failing_enabled());
    }

    #[test]
    fn thinking_summaries_defaults_on() {
        assert!(UiConfig::default().thinking_summaries_enabled());
        let off = UiConfig {
            thinking_summaries: Some(false),
            ..Default::default()
        };
        assert!(!off.thinking_summaries_enabled());
    }

    #[test]
    fn keep_text_selection_enabled_precedence() {
        let mut ui = UiConfig::default();
        assert!(!ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(0);
        assert!(ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(150);
        assert!(!ui.keep_text_selection_enabled());

        ui.selection_highlight_duration_ms = Some(0);
        ui.keep_text_selection = Some("flash".into());
        assert!(!ui.keep_text_selection_enabled());

        ui.keep_text_selection = Some("hold".into());
        ui.selection_highlight_duration_ms = Some(999);
        assert!(ui.keep_text_selection_enabled());

        ui.keep_text_selection = Some("hold".into());
        ui.selection_highlight_duration_ms = None;
        assert!(ui.keep_text_selection_enabled());

        // `word_select` implies hold (persistent highlight).
        ui.keep_text_selection = Some("word_select".into());
        ui.selection_highlight_duration_ms = None;
        assert!(ui.keep_text_selection_enabled());
    }

    #[test]
    fn keep_text_selection_deserializes_legacy_bool_and_string() {
        let from_true: UiConfig = serde_json::from_str(r#"{"keep_text_selection": true}"#).unwrap();
        assert_eq!(from_true.keep_text_selection.as_deref(), Some("hold"));

        let from_false: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": false}"#).unwrap();
        assert_eq!(from_false.keep_text_selection.as_deref(), Some("flash"));

        let from_hold: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": "hold"}"#).unwrap();
        assert_eq!(from_hold.keep_text_selection.as_deref(), Some("hold"));

        let from_flash: UiConfig =
            serde_json::from_str(r#"{"keep_text_selection": "flash"}"#).unwrap();
        assert_eq!(from_flash.keep_text_selection.as_deref(), Some("flash"));
    }

    #[test]
    fn display_refresh_nested_deserialize() {
        let ui: UiConfig = serde_json::from_str(
            r#"{"display_refresh": {"auto_cadence_enabled": true, "floor_ms": 7, "probe_enabled": false}}"#,
        )
        .unwrap();
        assert_eq!(ui.display_refresh.auto_cadence_enabled, Some(true));
        assert_eq!(ui.display_refresh.floor_ms, Some(7));
        assert_eq!(ui.display_refresh.probe_enabled, Some(false));
        assert!(!ui.display_refresh.is_default());
    }

    #[test]
    fn display_refresh_default_is_skipped_shape() {
        assert!(DisplayRefreshSettings::default().is_default());
        let ui = UiConfig::default();
        assert!(ui.display_refresh.is_default());
    }
}
