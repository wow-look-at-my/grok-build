use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub method: NotificationMethod,
    pub condition: NotificationCondition,
    pub idle_threshold_secs: u64,
    pub events: Vec<NotificationEventKind>,
    /// When the pager holds the system awake. The key `sleep_prevention` still reads, as a bool.
    #[serde(alias = "sleep_prevention")]
    pub keep_awake: KeepAwake,
    pub progress_bar: bool,
    /// Show an automatic "where was I" session recap when you return to the terminal after being away.
    /// Only applies when the shell has rolled out session recap (`sessionRecap` on ACP initialize or remote settings).
    /// Manual `/recap` is gated by the shell flag alone, not this toggle.
    pub session_recap: bool,
    /// Minimum seconds the terminal must be unfocused ("stepped away") before the client requests an automatic recap.
    /// This is a short debounce against quick tab blips.
    /// The authoritative timing (at least 3 minutes since the last completed turn) is enforced agent-side.
    pub session_recap_threshold_secs: u64,
    pub title: TitleConfig,
    pub hooks: Vec<NotificationHook>,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            method: NotificationMethod::default(),
            condition: NotificationCondition::default(),
            idle_threshold_secs: 3,
            events: vec![
                NotificationEventKind::TurnComplete,
                NotificationEventKind::ApprovalRequired,
            ],
            keep_awake: KeepAwake::default(),
            progress_bar: true,
            session_recap: true,
            session_recap_threshold_secs: 30,
            title: TitleConfig::default(),
            hooks: Vec::new(),
        }
    }
}

/// When to stop the system from idle-sleeping.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeepAwake {
    Never,
    /// While any session has a turn, an active goal, a background task, a live subagent or an active workflow.
    #[default]
    WhileRunning,
    /// For as long as the pager runs.
    Always,
}

impl KeepAwake {
    pub const ALL: [Self; 3] = [Self::Never, Self::WhileRunning, Self::Always];

    pub const fn as_canonical(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::WhileRunning => "while_running",
            Self::Always => "always",
        }
    }

    pub fn from_canonical(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_canonical() == s)
    }

    /// Whether to hold the system awake, given whether any work runs now.
    pub const fn wants_awake(self, running: bool) -> bool {
        match self {
            Self::Never => false,
            Self::WhileRunning => running,
            Self::Always => true,
        }
    }
}

/// Reads a mode name, or the bool of the old `sleep_prevention` key: `true` is `while_running`, `false` is `never`.
impl<'de> Deserialize<'de> for KeepAwake {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = KeepAwake;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(r#""never", "while_running", "always", or a bool"#)
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<KeepAwake, E> {
                Ok(if v {
                    KeepAwake::WhileRunning
                } else {
                    KeepAwake::Never
                })
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<KeepAwake, E> {
                KeepAwake::from_canonical(v)
                    .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(v), &self))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationMethod {
    #[default]
    Auto,
    Osc9,
    Osc99,
    Osc777,
    Bel,
    None,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationCondition {
    #[default]
    Unfocused,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TitleConfig {
    pub enabled: bool,
    pub items: Vec<TitleItem>,
}

impl Default for TitleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            items: vec![
                TitleItem::ActionRequired,
                TitleItem::Spinner,
                TitleItem::Activity,
                TitleItem::SessionName,
                TitleItem::Grok,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TitleItem {
    Spinner,
    Activity,
    SessionName,
    Cwd,
    Model,
    TurnTimer,
    Grok,
    ActionRequired,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::AsRefStr, strum::IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEventKind {
    #[strum(serialize = "Turn complete")]
    TurnComplete,
    #[strum(serialize = "Approval required")]
    ApprovalRequired,
    #[strum(serialize = "Session ready")]
    SessionReady,
    #[strum(serialize = "Task complete")]
    TaskComplete,
    #[strum(serialize = "Agent error")]
    AgentError,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct NotificationHook {
    pub command: String,
    #[serde(default)]
    pub events: Vec<NotificationEventKind>,
    #[serde(default = "default_only_unfocused")]
    pub only_unfocused: bool,
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u64,
}

fn default_only_unfocused() -> bool {
    true
}

fn default_hook_timeout() -> u64 {
    10
}

impl NotificationConfig {
    /// Generate a commented TOML template for the `[ui.notifications]` section.
    /// Mirrors `RawAppearanceConfig::to_toml_with_comments()` for `pager.toml`.
    /// The output is suitable for documentation or as a starter config snippet.
    pub fn to_toml_with_comments() -> String {
        "\
[ui.notifications]
# Notification protocol: auto|osc9|osc99|osc777|bel|none
# \"auto\" selects the best protocol for your terminal.
method = \"auto\"
# When to notify: unfocused|always|never
# \"unfocused\" only fires when the terminal has lost focus.
condition = \"unfocused\"
# Minimum seconds the terminal must be unfocused before notifications fire.
idle_threshold_secs = 3
# Events that trigger notifications.
# Options: turn_complete, approval_required, session_ready, task_complete, agent_error
events = [\"turn_complete\", \"approval_required\"]
# Keep the system awake: never|while_running|always (macOS/Linux).
# \"while_running\" holds it while a turn, goal, background task, subagent or
# workflow runs. \"always\" holds it while grok runs.
keep_awake = \"while_running\"
# Show a progress indicator in the terminal tab (OSC 9;4).
progress_bar = true
# Show an automatic \"where was I\" session recap when you return after being away.
# Shell session_recap is on by default; disable via [features] session_recap or
# GROK_SESSION_RECAP=0. Manual /recap uses only the shell flag.
session_recap = true
# Minimum seconds unfocused (\"stepped away\") before requesting a recap; a
# debounce against quick tab blips. The \"3 min since the last turn\" timing is
# enforced agent-side.
session_recap_threshold_secs = 30

[ui.notifications.title]
# Set the terminal/tab title to reflect agent state.
enabled = true
# Items shown in the title. Options: action-required, spinner, activity,
# session-name, cwd, model, turn-timer, grok
items = [\"action-required\", \"spinner\", \"activity\", \"session-name\", \"grok\"]

# [[ui.notifications.hooks]]
# command = \"terminal-notifier -title 'Grok' -message '$GROK_MESSAGE'\"
# events = [\"turn_complete\", \"approval_required\"]
# only_unfocused = true
# timeout_secs = 10
"
        .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_toml() {
        let config = NotificationConfig {
            method: NotificationMethod::Osc99,
            condition: NotificationCondition::Always,
            idle_threshold_secs: 10,
            events: vec![
                NotificationEventKind::TurnComplete,
                NotificationEventKind::AgentError,
            ],
            keep_awake: KeepAwake::Always,
            progress_bar: false,
            session_recap: false,
            session_recap_threshold_secs: 90,
            title: TitleConfig {
                enabled: false,
                items: vec![TitleItem::Grok, TitleItem::Cwd],
            },
            hooks: vec![NotificationHook {
                command: "notify-send".into(),
                events: vec![NotificationEventKind::TurnComplete],
                only_unfocused: false,
                timeout_secs: 5,
            }],
        };

        let toml_str = toml::to_string(&config).expect("serialize");
        let parsed: NotificationConfig = toml::from_str(&toml_str).expect("deserialize");

        assert_eq!(config, parsed);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let parsed: NotificationConfig = toml::from_str("").expect("deserialize empty");

        assert_eq!(parsed.method, NotificationMethod::Auto);
        assert_eq!(parsed.condition, NotificationCondition::Unfocused);
        assert_eq!(parsed.idle_threshold_secs, 3);
        assert_eq!(
            parsed.events,
            vec![
                NotificationEventKind::TurnComplete,
                NotificationEventKind::ApprovalRequired,
            ]
        );
        assert_eq!(parsed.keep_awake, KeepAwake::WhileRunning);
        assert!(parsed.progress_bar);
        assert!(parsed.session_recap);
        assert_eq!(parsed.session_recap_threshold_secs, 30);
        assert!(parsed.title.enabled);
        assert!(parsed.hooks.is_empty());
    }

    #[test]
    fn partial_toml_merges_with_defaults() {
        let toml_str = r#"
            method = "bel"
            idle_threshold_secs = 60
        "#;
        let parsed: NotificationConfig = toml::from_str(toml_str).expect("deserialize partial");

        assert_eq!(parsed.method, NotificationMethod::Bel);
        assert_eq!(parsed.idle_threshold_secs, 60);
        // The remaining fields keep their defaults
        assert_eq!(parsed.condition, NotificationCondition::Unfocused);
        assert_eq!(parsed.keep_awake, KeepAwake::WhileRunning);
        assert!(parsed.progress_bar);
    }

    #[test]
    fn keep_awake_reads_every_mode() {
        for mode in KeepAwake::ALL {
            let toml_str = format!("keep_awake = \"{}\"", mode.as_canonical());
            let parsed: NotificationConfig = toml::from_str(&toml_str).expect("deserialize");
            assert_eq!(parsed.keep_awake, mode, "{toml_str}");
        }
    }

    #[test]
    fn keep_awake_reads_the_old_sleep_prevention_bool() {
        for (toml_str, expected) in [
            ("sleep_prevention = true", KeepAwake::WhileRunning),
            ("sleep_prevention = false", KeepAwake::Never),
            ("keep_awake = false", KeepAwake::Never),
        ] {
            let parsed: NotificationConfig = toml::from_str(toml_str).expect("deserialize");
            assert_eq!(parsed.keep_awake, expected, "{toml_str}");
        }
    }

    #[test]
    fn keep_awake_rejects_an_unknown_mode() {
        let err = toml::from_str::<NotificationConfig>("keep_awake = \"sometimes\"")
            .expect_err("an unknown mode must not parse");
        assert!(err.to_string().contains("while_running"), "{err}");
    }

    #[test]
    fn keep_awake_wants_awake_per_mode() {
        assert!(!KeepAwake::Never.wants_awake(true));
        assert!(!KeepAwake::WhileRunning.wants_awake(false));
        assert!(KeepAwake::WhileRunning.wants_awake(true));
        assert!(KeepAwake::Always.wants_awake(false));
    }

    #[test]
    fn hook_defaults_applied() {
        let toml_str = r#"
            [[hooks]]
            command = "my-script.sh"
        "#;
        let parsed: NotificationConfig = toml::from_str(toml_str).expect("deserialize hooks");

        assert_eq!(parsed.hooks.len(), 1);
        let Some(hook) = parsed.hooks.first() else {
            panic!("expected one hook: {:?}", parsed.hooks);
        };
        assert_eq!(hook.command, "my-script.sh");
        assert!(hook.events.is_empty());
        assert!(hook.only_unfocused);
        assert_eq!(hook.timeout_secs, 10);
    }

    #[test]
    fn all_notification_methods_deserialize() {
        for (input, expected) in [
            ("\"auto\"", NotificationMethod::Auto),
            ("\"osc9\"", NotificationMethod::Osc9),
            ("\"osc99\"", NotificationMethod::Osc99),
            ("\"osc777\"", NotificationMethod::Osc777),
            ("\"bel\"", NotificationMethod::Bel),
            ("\"none\"", NotificationMethod::None),
        ] {
            let parsed: NotificationMethod = toml::from_str(&format!("method = {input}\n"))
                .map(|c: MethodWrapper| c.method)
                .unwrap_or_else(|e| panic!("failed to parse {input}: {e}"));
            assert_eq!(parsed, expected, "mismatch for {input}");
        }
    }

    #[test]
    fn all_conditions_deserialize() {
        for (input, expected) in [
            ("\"unfocused\"", NotificationCondition::Unfocused),
            ("\"always\"", NotificationCondition::Always),
            ("\"never\"", NotificationCondition::Never),
        ] {
            let parsed: NotificationCondition = toml::from_str(&format!("condition = {input}\n"))
                .map(|c: ConditionWrapper| c.condition)
                .unwrap_or_else(|e| panic!("failed to parse {input}: {e}"));
            assert_eq!(parsed, expected, "mismatch for {input}");
        }
    }

    #[test]
    fn title_items_kebab_case() {
        let toml_str = r#"
            [title]
            enabled = true
            items = ["action-required", "turn-timer", "session-name"]
        "#;
        let parsed: NotificationConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(
            parsed.title.items,
            vec![
                TitleItem::ActionRequired,
                TitleItem::TurnTimer,
                TitleItem::SessionName,
            ]
        );
    }

    #[test]
    fn to_toml_with_comments_contains_all_sections() {
        let toml = NotificationConfig::to_toml_with_comments();
        assert!(toml.contains("[ui.notifications]"));
        assert!(toml.contains("[ui.notifications.title]"));
        assert!(toml.contains("[[ui.notifications.hooks]]"));
    }

    #[test]
    fn to_toml_with_comments_matches_defaults() {
        let template = NotificationConfig::to_toml_with_comments();
        let defaults = NotificationConfig::default();

        assert!(
            template.contains(&format!(
                "idle_threshold_secs = {}",
                defaults.idle_threshold_secs
            )),
            "template idle_threshold_secs does not match default"
        );
        assert!(
            template.contains(&format!(
                "keep_awake = \"{}\"",
                defaults.keep_awake.as_canonical()
            )),
            "template keep_awake does not match default"
        );
        assert!(
            template.contains(&format!("progress_bar = {}", defaults.progress_bar)),
            "template progress_bar does not match default"
        );
        assert!(
            template.contains(&format!("enabled = {}", defaults.title.enabled)),
            "template title.enabled does not match default"
        );
    }

    #[test]
    fn to_toml_with_comments_hook_section_is_valid_toml() {
        let template = NotificationConfig::to_toml_with_comments();
        // Uncomment only TOML structural lines (table headers and key = value).
        // Plain-English comment lines are left as comments so they don't produce parse errors
        let uncommented: String = template
            .lines()
            .map(|line| {
                if let Some(stripped) = line.strip_prefix("# ") {
                    let trimmed = stripped.trim_start();
                    if trimmed.starts_with("[[") || trimmed.contains(" = ") {
                        return format!("{stripped}\n");
                    }
                }
                format!("{line}\n")
            })
            .collect();
        let parsed: toml::Value =
            toml::from_str(&uncommented).expect("uncommented template should be valid TOML");
        let hooks = parsed
            .get("ui")
            .and_then(|u| u.get("notifications"))
            .and_then(|n| n.get("hooks"))
            .expect("hooks key must exist after uncommenting");
        assert!(hooks.is_array(), "hooks should be an array of tables");
    }

    // Helper wrappers for single-field deserialization tests
    #[derive(Deserialize)]
    struct MethodWrapper {
        method: NotificationMethod,
    }

    #[derive(Deserialize)]
    struct ConditionWrapper {
        condition: NotificationCondition,
    }
}
