use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub method: NotificationMethod,
    pub condition: NotificationCondition,
    pub idle_threshold_secs: u64,
    pub events: Vec<NotificationEventKind>,
    pub sleep_prevention: bool,
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
            sleep_prevention: true,
            progress_bar: true,
            session_recap: true,
            session_recap_threshold_secs: 30,
            title: TitleConfig::default(),
            hooks: Vec::new(),
        }
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
    /// A `config.json` snippet with the `ui.notifications` defaults and one example hook.
    /// It is a starter config for the docs. The config schema describes each key.
    pub fn to_json_example() -> serde_json::Result<String> {
        let mut config = Self::default();
        config.hooks.push(NotificationHook {
            command: "terminal-notifier -title 'Grok' -message '$GROK_MESSAGE'".to_owned(),
            events: vec![
                NotificationEventKind::TurnComplete,
                NotificationEventKind::ApprovalRequired,
            ],
            only_unfocused: default_only_unfocused(),
            timeout_secs: default_hook_timeout(),
        });
        let mut ui = serde_json::Map::new();
        ui.insert("notifications".to_owned(), serde_json::to_value(&config)?);
        let mut root = serde_json::Map::new();
        root.insert("ui".to_owned(), serde_json::Value::Object(ui));
        Ok(xai_grok_config::format::to_config_json_string(&root, None))
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
            sleep_prevention: false,
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
        assert!(parsed.sleep_prevention);
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
        assert!(parsed.sleep_prevention);
        assert!(parsed.progress_bar);
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

    fn json_example() -> serde_json::Value {
        serde_json::from_str(&NotificationConfig::to_json_example().unwrap())
            .expect("the example is valid JSON")
    }

    #[test]
    fn to_json_example_contains_all_sections() {
        let example = json_example();
        let notifications = &example["ui"]["notifications"];
        assert!(notifications.is_object());
        assert!(notifications["title"].is_object());
        assert!(notifications["hooks"].is_array());
    }

    /// Apart from the example hook, the example is the defaults.
    #[test]
    fn to_json_example_matches_defaults() {
        let example = json_example();
        let mut parsed: NotificationConfig =
            serde_json::from_value(example["ui"]["notifications"].clone()).unwrap();
        assert_eq!(parsed.hooks.len(), 1);
        parsed.hooks.clear();
        assert_eq!(parsed, NotificationConfig::default());
    }

    #[test]
    fn to_json_example_hook_parses() {
        let example = json_example();
        let parsed: NotificationConfig =
            serde_json::from_value(example["ui"]["notifications"].clone()).unwrap();
        let hook = &parsed.hooks[0];
        assert!(hook.command.contains("$GROK_MESSAGE"));
        assert_eq!(
            hook.events,
            vec![
                NotificationEventKind::TurnComplete,
                NotificationEventKind::ApprovalRequired
            ]
        );
        assert!(hook.only_unfocused);
        assert_eq!(hook.timeout_secs, 10);
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
