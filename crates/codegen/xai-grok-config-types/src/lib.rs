#![allow(clippy::cast_lossless)]
#![deny(clippy::indexing_slicing)]

mod file_acceleration;
pub use file_acceleration::*;
mod flags;
pub use flags::*;
mod registry;
pub use registry::*;
mod memory;
pub use memory::*;
mod mcp;
pub use mcp::*;
mod permission;
pub use permission::*;
mod auth_provider;
pub use auth_provider::*;
<<<<<<< fork
    #[test]
    fn remote_settings_goal_skeptic_models_fully_valid_pool_round_trips() {
        let json = r#"{"goal_skeptic_models": [
            {"model": "grok-4", "agent_type": "general-purpose"},
            {"model": "grok-3", "agent_type": "cursor"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_skeptic_models,
            vec![
                GoalRoleModel {
                    model: "grok-4".to_string(),
                    agent_type: "general-purpose".to_string(),
                },
                GoalRoleModel {
                    model: "grok-3".to_string(),
                    agent_type: "cursor".to_string(),
                },
            ]
        );
        let out = serde_json::to_string(&s).unwrap();
        let s2: RemoteSettings = serde_json::from_str(&out).unwrap();
        assert_eq!(s2.goal_skeptic_models, s.goal_skeptic_models);
    }
    #[test]
    fn remote_settings_goal_skeptic_models_one_bad_item_does_not_poison_pool() {
        let json = r#"{"goal_skeptic_models": [
            {"model": "grok-4", "agent_type": "general-purpose"},
            {"model": "grok-broken"},
            {"model": "grok-3", "agent_type": "cursor"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_skeptic_models,
            vec![
                GoalRoleModel {
                    model: "grok-4".to_string(),
                    agent_type: "general-purpose".to_string(),
                },
                GoalRoleModel {
                    model: "grok-3".to_string(),
                    agent_type: "cursor".to_string(),
                },
            ]
        );
    }
    #[test]
    fn remote_settings_goal_skeptic_models_all_entries_bad_yields_empty() {
        let json = r#"{"goal_skeptic_models": [
            {"model": "only-model"},
            {"agent_type": "only-agent-type"},
            "scalar-not-an-object",
            42
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert!(s.goal_skeptic_models.is_empty());
    }
    #[test]
    fn remote_settings_goal_skeptic_models_non_array_yields_empty() {
        for json in [
            r#"{"goal_skeptic_models": {"model": "x", "agent_type": "y"}}"#,
            r#"{"goal_skeptic_models": "not-an-array"}"#,
            r#"{"goal_skeptic_models": 7}"#,
        ] {
            let s: RemoteSettings = serde_json::from_str(json).unwrap();
            assert!(
                s.goal_skeptic_models.is_empty(),
                "non-array pool must yield empty for {json}"
            );
        }
    }
    #[test]
    fn remote_settings_goal_skeptic_models_missing_model_entry_dropped() {
        let json = r#"{"goal_skeptic_models": [
            {"agent_type": "general-purpose"},
            {"model": "grok-3", "agent_type": "cursor"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_skeptic_models,
            vec![GoalRoleModel {
                model: "grok-3".to_string(),
                agent_type: "cursor".to_string(),
            }]
        );
    }
    #[test]
    fn remote_settings_goal_skeptic_models_wrong_typed_scalar_dropped() {
        let json = r#"{"goal_skeptic_models": [
            {"model": 123, "agent_type": "general-purpose"},
            {"model": "grok-3", "agent_type": ["cursor"]},
            {"model": "grok-4", "agent_type": "general-purpose"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_skeptic_models,
            vec![GoalRoleModel {
                model: "grok-4".to_string(),
                agent_type: "general-purpose".to_string(),
            }]
        );
    }
    #[test]
    fn remote_settings_goal_skeptic_models_extra_unknown_fields_kept() {
        let json = r#"{"goal_skeptic_models": [
            {"model": "grok-4", "agent_type": "general-purpose", "reasoning_effort": "high"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_skeptic_models,
            vec![GoalRoleModel {
                model: "grok-4".to_string(),
                agent_type: "general-purpose".to_string(),
            }]
        );
    }
    #[test]
    fn remote_settings_goal_skeptic_models_survivor_order_preserved() {
        let json = r#"{"goal_skeptic_models": [
            {"model": "first", "agent_type": "general-purpose"},
            {"model": "bad"},
            {"model": "second", "agent_type": "cursor"},
            "garbage",
            {"model": "third", "agent_type": "general-purpose"}
        ]}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        let order: Vec<&str> = s
            .goal_skeptic_models
            .iter()
            .map(|m| m.model.as_str())
            .collect();
        assert_eq!(order, vec!["first", "second", "third"]);
    }
    #[test]
    fn remote_settings_goal_planner_model_malformed_yields_none() {
        for json in [
            r#"{"goal_planner_model": {"model": "only-model"}}"#,
            r#"{"goal_planner_model": {"agent_type": "only-agent-type"}}"#,
            r#"{"goal_planner_model": {"model": 1, "agent_type": "x"}}"#,
            r#"{"goal_planner_model": "scalar"}"#,
            r#"{"goal_planner_model": null}"#,
        ] {
            let s: RemoteSettings = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("must not hard-error for {json}: {e}"));
            assert_eq!(s.goal_planner_model, None, "for {json}");
        }
    }
    #[test]
    fn remote_settings_goal_role_models_malformed_pair_does_not_drop_other_fields() {
        let json = r#"{
            "goal_planner_model": {"model": "broken"},
            "goal_strategist_model": {"model": "grok-4.5", "agent_type": "cursor"},
            "default_model": "grok-4"
        }"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.goal_planner_model, None);
        assert_eq!(
            s.goal_strategist_model,
            Some(GoalRoleModel {
                model: "grok-4.5".to_string(),
                agent_type: "cursor".to_string(),
            })
        );
        assert_eq!(s.default_model.as_deref(), Some("grok-4"));
    }
    #[test]
    fn remote_settings_goal_role_model_extra_unknown_fields_kept_single_pair() {
        let json = r#"{"goal_planner_model": {"model": "grok-4", "agent_type": "general-purpose", "future": true}}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(
            s.goal_planner_model,
            Some(GoalRoleModel {
                model: "grok-4".to_string(),
                agent_type: "general-purpose".to_string(),
            })
        );
    }
    #[test]
    fn remote_settings_inference_idle_timeout_present() {
        let json = r#"{"inference_idle_timeout_secs": 180}"#;
        let settings: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.inference_idle_timeout_secs, Some(180));
    }
    #[test]
    fn remote_settings_initial_injection_deserialize_present() {
        let json = r#"{"memory_initial_injection_enabled": false, "memory_initial_injection_min_score": 0.66}"#;
        let settings: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.memory_initial_injection_enabled, Some(false));
        assert_eq!(settings.memory_initial_injection_min_score, Some(0.66));
    }
    #[test]
    fn remote_settings_memory_v2_round_trips_and_defaults_absent() {
        for expected in [true, false] {
            let settings: RemoteSettings = serde_json::from_value(serde_json::json!({
                "memory_v2": {
                    "enabled": expected,
                    "capture_status_enabled": expected,
                },
            }))
            .unwrap();
            let memory_v2 = settings.memory_v2.as_ref().unwrap();
            assert_eq!(memory_v2.enabled, Some(expected));
            assert_eq!(memory_v2.capture_status_enabled, Some(expected));
            let round_trip: RemoteSettings =
                serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
            let memory_v2 = round_trip.memory_v2.as_ref().unwrap();
            assert_eq!(memory_v2.enabled, Some(expected));
            assert_eq!(memory_v2.capture_status_enabled, Some(expected));
        }
        let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.memory_v2, None);
    }
    #[test]
    fn remote_settings_initial_injection_deserialize_absent() {
        let json = r#"{"memory_enabled": true}"#;
        let settings: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.memory_initial_injection_enabled, None);
        assert_eq!(settings.memory_initial_injection_min_score, None);
    }
    #[test]
    fn remote_settings_inference_idle_timeout_absent() {
        let json = r#"{"memory_enabled": true}"#;
        let settings: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.inference_idle_timeout_secs, None);
    }
    #[test]
    fn remote_settings_unknown_fields_tolerated() {
        let json = r#"{
            "inference_idle_timeout_secs": 120,
            "future_remote_field": 42,
            "verification_staleness_enabled": true
        }"#;
        let settings: RemoteSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.inference_idle_timeout_secs, Some(120));
    }
    #[test]
    fn remote_settings_display_refresh_present_partial() {
        let json = r#"{"display_refresh": {"auto_cadence_enabled": true, "floor_ms": 7}}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        let dr = s.display_refresh.expect("display_refresh present");
        assert_eq!(dr.auto_cadence_enabled, Some(true));
        assert_eq!(dr.floor_ms, Some(7));
        assert_eq!(dr.probe_enabled, None);
        assert_eq!(dr.ceiling_ms, None);
        assert_eq!(dr.min_hz, None);
        assert_eq!(dr.max_hz, None);
    }
    #[test]
    fn remote_settings_display_refresh_unknown_keys_preserved() {
        let json =
            r#"{"display_refresh": {"probe_enabled": true, "future_knob": 42, "floor_ms": "bad"}}"#;
        let s: RemoteSettings = serde_json::from_str(json).unwrap();
        let dr = s.display_refresh.expect("display_refresh present");
        assert_eq!(dr.probe_enabled, Some(true));
        assert_eq!(dr.floor_ms, None, "wrong-typed floor_ms ignored");
        assert_eq!(dr.extra.get("future_knob"), Some(&serde_json::json!(42)));
        let out = serde_json::to_value(&dr).unwrap();
        assert_eq!(out.get("future_knob"), Some(&serde_json::json!(42)));
        assert_eq!(out.get("probe_enabled"), Some(&serde_json::json!(true)));
    }
    #[test]
    fn remote_settings_accept_request_encodings_unknown_tolerated_and_default_empty() {
        let s: RemoteSettings =
            serde_json::from_str(r#"{"accept_request_encodings": ["zstd", "br"]}"#).unwrap();
        assert_eq!(
            vec![RemoteRequestEncoding::Zstd, RemoteRequestEncoding::Unknown],
            s.accept_request_encodings
        );
        let absent: RemoteSettings = serde_json::from_str("{}").unwrap();
        assert!(absent.accept_request_encodings.is_empty());
    }
    type JemallocFields<'a> = (Option<bool>, Option<&'a [u64]>, Option<u64>);
    fn jemalloc_fields(s: &RemoteSettings) -> JemallocFields<'_> {
        (
            s.jemalloc_heap_profile_enabled,
            s.jemalloc_heap_profile_thresholds_bytes.as_deref(),
            s.jemalloc_heap_profile_poll_interval_secs,
        )
    }
    fn parse_remote(json: &str) -> RemoteSettings {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("parse failed for {json}: {e}"))
    }
    fn round_trip_remote(s: &RemoteSettings) -> RemoteSettings {
        let out = serde_json::to_string(s).unwrap();
        parse_remote(&out)
    }
    fn assert_jemalloc_round_trip(json: &str, expected: JemallocFields<'_>) {
        let s = parse_remote(json);
        assert_eq!(jemalloc_fields(&s), expected);
        assert_eq!(jemalloc_fields(&round_trip_remote(&s)), expected);
    }
    fn assert_remote_parse_err(json: &str) {
        assert!(
            serde_json::from_str::<RemoteSettings>(json).is_err(),
            "expected parse error for {json}"
        );
    }
    #[test]
    fn remote_settings_jemalloc_heap_profile_thresholds_populated_and_empty_round_trip() {
        assert_jemalloc_round_trip(
            r#"{
                "jemalloc_heap_profile_thresholds_bytes": [2147483648, 5368709120, 10737418240]
            }"#,
            (
                None,
                Some(&[2_147_483_648, 5_368_709_120, 10_737_418_240]),
                None,
            ),
        );
        assert_jemalloc_round_trip(
            r#"{"jemalloc_heap_profile_thresholds_bytes": []}"#,
            (None, Some(&[]), None),
        );
        assert_jemalloc_round_trip(
            r#"{
                "jemalloc_heap_profile_enabled": true,
                "jemalloc_heap_profile_thresholds_bytes": []
            }"#,
            (Some(true), Some(&[]), None),
        );
    }
    #[test]
    fn remote_settings_jemalloc_heap_profile_malformed_fails_whole_parse() {
        for json in [
            r#"{"jemalloc_heap_profile_enabled": "yes"}"#,
            r#"{"jemalloc_heap_profile_enabled": 1}"#,
            r#"{"jemalloc_heap_profile_enabled": []}"#,
            r#"{"jemalloc_heap_profile_enabled": {}}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": "2G"}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": {"bytes": 1}}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": [true]}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": ["2G"]}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": [-1]}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": [null]}"#,
            r#"{"jemalloc_heap_profile_thresholds_bytes": [1, null, 2]}"#,
            r#"{"jemalloc_heap_profile_poll_interval_secs": "30"}"#,
            r#"{"jemalloc_heap_profile_poll_interval_secs": true}"#,
            r#"{"jemalloc_heap_profile_poll_interval_secs": -1}"#,
            r#"{
                "jemalloc_heap_profile_enabled": true,
                "jemalloc_heap_profile_thresholds_bytes": "bad",
                "workspace_command_enabled": true
            }"#,
        ] {
            assert_remote_parse_err(json);
        }
    }
}
@@END_DROP@@
pub mod retry_budget;
mod privacy;
pub use privacy::{PrivacyMode, SharedPrivacyMode};
mod sandbox;
pub use sandbox::*;

use serde::{Deserialize, Serialize};

pub use xai_grok_config::{
    CampaignOverride, ConsentGate, ContextualHintsRemote, DisplayRefreshSettings,
    DoomLoopRecoverySettings, GoalRoleModel, LongReasoningReminderSettings, RemoteRequestEncoding,
    RemoteSettings, WorktreeAutoGcSettings, WorktreeKindMaxAge, deserialize_tolerant,
};

/// Advanced knobs for the output-rate floor: the `[output_rate_floor]` TOML
/// table. Every field is `Option` so a partial table never fails the parse
/// and each key falls through to the client default on its own.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(default)]
pub struct OutputRateFloorSettings {
    /// Trailing window the rate is measured over, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<u64>,
    /// Reissue budget per model call. `-1` or `"unlimited"` never runs out.
    #[serde(with = "retry_budget", skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "retry_budget::json_schema")]
    pub max_retries: Option<u32>,
}
