//! Configuration for intra-compaction.

use serde::{Deserialize, Serialize};

/// Which targets intra-compaction may compact. - `FullReplace` (default).
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntraCompactionMode {
    #[default]
    FullReplace,
    StepsOnly,
    HistoryOnly,
    HistoryThenSteps,
}

/// Which *summarization algorithm* intra-compaction uses to turn the selected
/// turns into the replacement summary.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntraSummarizer {
    /// New (default): the shared summarization core.
    #[default]
    Shared,
    /// Previous intra algorithm: per-target prompt (`format_compaction_prompt` / history dev+user prompts).
    Legacy,
}

/// Intra-compaction configuration for an agent's sample loop. This is the
/// intra-compaction analog of
/// [`InterCompactionConfig`](crate::inter_compaction::InterCompactionConfig).
/// The structural difference is *where the config lives*: - inter-compaction
/// runs as a singleton between-turn service, so it has one
///   global config resolved from service YAML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntraCompactionConfig {
    // ───────────────────────────── Common (all modes) ───────────────────────────── Present on every config path regardless.

    // -- Enablement & strategy selection -- Enable intra-compaction between steps. Default: `false` (disabled).
    pub enabled: bool,

    /// Which targets intra-compaction may compact. See [`IntraCompactionMode`]. Default: `FullReplace`.
    pub mode: IntraCompactionMode,

    // -- Trigger gating: when a compaction pass fires (see `should_compact`).
    pub trigger_threshold_percent: u8,

    /// Minimum number of completed steps before compaction can trigger. Default: `3`.
    pub min_steps_before_compact: u32,

    // -- Reduction guards: whether a produced summary is worth keeping.
    pub min_compactable_tokens: u32,

    /// Discard the compaction if it didn't shrink tokens below this ratio.
    pub max_reduction_ratio: f64,

    // -- Compaction LLM call (sampling) -- Compaction model name. Blank/`None` → [`DEFAULT_COMPACTION_MODEL_NAME`].
    pub compaction_model_name: Option<String>,

    /// End-to-end timeout for the compaction LLM call.
    pub sampling_timeout_secs: u64,

    /// Max attempts for the compaction LLM call (effective value is `max(1)`).
    pub max_attempts: u32,
    /// Delay between retries. Default: `3`.
    pub retry_delay_secs: u64,

    // -- Audit -- Version string for the compaction (e.g. `"intra-v1"`). Recorded in audit logs. Default: `"intra-v1"`.
    pub compaction_version: String,

    // ───────────────────────────── Mode-specific ───────────────────────────── Each field below is read by only a subset of modes.

    // -- Partial modes only: StepsOnly / HistoryOnly / HistoryThenSteps.
    pub summarizer: IntraSummarizer,

    /// [StepsOnly / HistoryOnly / HistoryThenSteps] Target usage percentage after compaction.
    pub target_threshold_percent: u8,

    // -- HistoryThenSteps only -- [HistoryThenSteps mode] Only compact accumulated step turns when their token count exceeds this fraction.
    pub steps_trigger_ratio: f64,

    // -- History target only: HistoryOnly + HistoryThenSteps' history pass.
    pub user_message_truncate_chars: u32,
}

/// Code-level default compaction model name (last resort).
pub const DEFAULT_COMPACTION_MODEL_NAME: &str = "grok-4.20";

impl IntraCompactionConfig {
    /// Agent field; blank/`None` → [`DEFAULT_COMPACTION_MODEL_NAME`].
    pub fn effective_compaction_model_name(&self) -> &str {
        self.compaction_model_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_COMPACTION_MODEL_NAME)
    }
}

impl Default for IntraCompactionConfig {
    fn default() -> Self {
        // These are the unset/blank defaults: the value each field takes when it
        // is absent in YAML or left blank in an agent config editor.
        Self {
            // Common (all modes; min_steps stored always, enforced except FullReplace)
            enabled: false,
            mode: IntraCompactionMode::default(),
            trigger_threshold_percent: 85,
            min_steps_before_compact: 3,
            min_compactable_tokens: 5_000,
            max_reduction_ratio: 0.8,
            compaction_model_name: Some(DEFAULT_COMPACTION_MODEL_NAME.to_string()),
            sampling_timeout_secs: 120,
            max_attempts: 2,
            retry_delay_secs: 3,
            compaction_version: "intra-v1".to_string(),
            // Mode-specific
            summarizer: IntraSummarizer::default(),
            target_threshold_percent: 50,
            steps_trigger_ratio: 0.3,
            user_message_truncate_chars: 3_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let p = IntraCompactionConfig::default();
        assert!(!p.enabled);
        assert_eq!(p.mode, IntraCompactionMode::FullReplace);
        assert_eq!(p.summarizer, IntraSummarizer::Shared);
        assert_eq!(p.trigger_threshold_percent, 85);
        assert_eq!(p.target_threshold_percent, 50);
        assert_eq!(
            p.compaction_model_name.as_deref(),
            Some(DEFAULT_COMPACTION_MODEL_NAME)
        );
        assert_eq!(
            p.effective_compaction_model_name(),
            DEFAULT_COMPACTION_MODEL_NAME
        );
        assert_eq!(p.max_attempts, 2);
        assert_eq!(p.retry_delay_secs, 3);
        assert!((p.steps_trigger_ratio - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn blank_or_none_compaction_model_name_uses_default() {
        let none = IntraCompactionConfig {
            compaction_model_name: None,
            ..Default::default()
        };
        assert_eq!(
            none.effective_compaction_model_name(),
            DEFAULT_COMPACTION_MODEL_NAME
        );
        let empty = IntraCompactionConfig {
            compaction_model_name: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            empty.effective_compaction_model_name(),
            DEFAULT_COMPACTION_MODEL_NAME
        );
        let ws = IntraCompactionConfig {
            compaction_model_name: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(
            ws.effective_compaction_model_name(),
            DEFAULT_COMPACTION_MODEL_NAME
        );
        let custom = IntraCompactionConfig {
            compaction_model_name: Some("custom-model".into()),
            ..Default::default()
        };
        assert_eq!(custom.effective_compaction_model_name(), "custom-model");
    }

    #[test]
    fn mode_serde_round_trip() {
        for (mode, s) in [
            (IntraCompactionMode::FullReplace, "\"full_replace\""),
            (IntraCompactionMode::StepsOnly, "\"steps_only\""),
            (IntraCompactionMode::HistoryOnly, "\"history_only\""),
            (
                IntraCompactionMode::HistoryThenSteps,
                "\"history_then_steps\"",
            ),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, s);
            let back: IntraCompactionMode = serde_json::from_str(s).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn summarizer_serde_round_trip() {
        for (s, json) in [
            (IntraSummarizer::Shared, "\"shared\""),
            (IntraSummarizer::Legacy, "\"legacy\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), json);
            let back: IntraSummarizer = serde_json::from_str(json).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn json_round_trip_with_serde_default() {
        // Partial JSON — `#[serde(default)]` fills missing fields.
        let json = r#"{
            "enabled": true,
            "trigger_threshold_percent": 80
        }"#;
        let p: IntraCompactionConfig = serde_json::from_str(json).unwrap();
        assert!(p.enabled);
        assert_eq!(p.trigger_threshold_percent, 80);
        // Defaults preserved.
        assert_eq!(p.target_threshold_percent, 50);
        assert_eq!(p.compaction_version, "intra-v1");
    }
}
