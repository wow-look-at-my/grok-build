//! This module fetches the model catalog, caches it, and loads it at startup.

mod endpoint;
mod fetch;
mod manager;
mod model_fetch_auth;
mod prefetch;
mod resolution;
pub mod settings_get;
<<<<<<< fork
mod settings_refresh;
mod synthetic;
||||||| base
mod settings_refresh;
=======
>>>>>>> upstream
pub(crate) mod task_model_policy;

<<<<<<< fork
pub(in crate::agent::remote_config) use cache::ModelsCacheManager;
pub(crate) use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub(in crate::agent::remote_config) use fetch::{ModelsPrefetch, fetch_models_uncommitted};
pub(crate) use fetch::{prefetch_models_async, prefetch_models_blocking};
pub(crate) use manager::ModelsManager;
pub(crate) use metrics::{DegradedStartCause, record_degraded_start};
pub(in crate::agent::remote_config) use model_fetch_auth::ModelsCacheScope;
pub(crate) use model_fetch_auth::{CacheAuthMethod, ModelFetchAuth, task_model_error_for_catalog};
||||||| base
pub(in crate::agent::remote_config) use cache::ModelsCacheManager;
pub(crate) use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub(crate) use fetch::prefetch_models_blocking;
pub(in crate::agent::remote_config) use fetch::{ModelsPrefetch, fetch_models_uncommitted};
pub(crate) use manager::ModelsManager;
pub(crate) use metrics::{DegradedStartCause, record_degraded_start};
pub(in crate::agent::remote_config) use model_fetch_auth::ModelsCacheScope;
pub(crate) use model_fetch_auth::{CacheAuthMethod, ModelFetchAuth, task_model_error_for_catalog};
=======
pub(crate) use model_fetch_auth::{
    ModelFetchAuth, external_provider_auth, models_fetch_enabled, task_model_error_for_catalog,
};
pub(in crate::agent::remote_config) use model_fetch_auth::{
    resolve_live_models_cache_scope, resolve_models_cache_scope,
};
>>>>>>> upstream
pub(in crate::agent::remote_config) use prefetch::resolve_startup_endpoints;
pub(crate) use prefetch::{
    ResolvedModels, fetch_initial_models_blocking, start_initial_models_load,
};
pub(crate) use resolution::{
<<<<<<< fork
    ModelGlobSet, allowlist_denied_message, allowlist_excludes_all_message,
    allowlist_matches_nothing, available_models, is_campaign_only_flip, merge_additive_catalog,
    resolve_catalog_key, resolve_context_window, resolve_context_window_from_provider,
    resolve_default_model, resolve_model_catalog, resolve_route_catalog_key,
||||||| base
    ModelGlobSet, allowlist_denied_message, allowlist_excludes_all_message,
    allowlist_matches_nothing, available_models, is_campaign_only_flip, resolve_catalog_key,
    resolve_default_model, resolve_model_catalog, selectable_catalog_key_for_persisted,
    validate_selectable,
=======
    CatalogSource, allowlist_denied_message, allowlist_excludes_all_message,
    allowlist_matches_nothing, available_models, fallback_model_id, models_endpoint_empty_message,
    resolve_catalog_key, resolve_default_model, resolve_model_catalog,
>>>>>>> upstream
    selectable_catalog_key_for_persisted, validate_selectable,
};
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::Commit;
pub(in crate::agent) use xai_grok_cloud_config::SettingsRefresh;
pub(crate) use xai_grok_cloud_config::remote_settings::{
    DegradedStartCause, ModelGlobSet, ModelsCacheScope, is_campaign_only_flip,
    record_degraded_start,
};
<<<<<<< fork
pub(crate) use settings_cache::SettingsCacheManager;
pub(in crate::agent::remote_config) use settings_cache::settings_cache_disabled;
pub(in crate::agent) use settings_refresh::SettingsRefresh;
pub(crate) use synthetic::{is_synthetic_listing, parse_synthetic_model_entry};
||||||| base
pub(crate) use settings_cache::SettingsCacheManager;
pub(in crate::agent::remote_config) use settings_cache::settings_cache_disabled;
pub(in crate::agent) use settings_refresh::SettingsRefresh;
=======
pub(in crate::agent::remote_config) use xai_grok_cloud_config::resolve_disk_auth;
>>>>>>> upstream

pub(in crate::agent::remote_config) type ModelsCacheManager =
    xai_grok_cloud_config::remote_settings::ModelsCacheManager<crate::agent::config::ModelEntry>;

pub(crate) use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub(in crate::agent::remote_config) use fetch::fetch_models_uncommitted;
pub(crate) use fetch::prefetch_models_blocking;
pub(crate) use manager::ModelsManager;
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::remote_settings::ModelsPrefetch;
#[cfg(test)]
pub(in crate::agent::remote_config) use xai_grok_cloud_config::remote_settings::evaluate_models_commit;

// Re-exports reached only through the manager test module.
#[cfg(test)]
pub(crate) use endpoint::ModelsFetchFuture;
#[cfg(test)]
pub(crate) use fetch::build_prefetched_map;
#[cfg(test)]
pub(crate) use prefetch::resolve_prefetch_inputs_from_parts;
#[cfg(test)]
pub(crate) use xai_grok_cloud_config::remote_settings::{
    CACHE_TTL, CacheAuthMethod, MODELS_CACHE_FILE, degraded_log_level,
};
#[cfg(test)]
pub(crate) type ModelsCache =
    xai_grok_cloud_config::remote_settings::ModelsCache<crate::agent::config::ModelEntry>;
