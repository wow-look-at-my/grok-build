//! Settings registry and modal: the canonical place for user preferences.

pub mod defs;
pub mod registry;

pub use registry::{
    CodingDataSharingLock, DynamicEnumSource, EnumChoice, FeatureOverrideState, OwnedEnumChoice,
    PagerLocalSnapshot, PendingWrite, SettingCategory, SettingKey, SettingKind, SettingMeta,
    SettingOwner, SettingValue, SettingsRegistry, StringValidator, canonical_hunk_tracker_mode,
    canonical_voice_capture_mode, canonical_voice_stt_language, current_value_for,
    default_value_for, dynamic_enum_choices, is_consent_chooser,
};
