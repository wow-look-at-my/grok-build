//! Remote settings and model-catalog fetching, caching, and startup wiring.

mod cache;
mod cache_file;
mod endpoint;
mod fetch;
mod manager;
mod metrics;
mod model_fetch_auth;
mod prefetch;
mod resolution;
mod scope;
mod settings_cache;
pub mod settings_get;
mod settings_refresh;
mod synthetic;
pub mod task_model_policy;

pub use cache::ModelsCacheManager;
pub use endpoint::{HttpModelsEndpoint, ModelsEndpoint};
pub use fetch::prefetch_models_blocking;
pub use fetch::{ModelsPrefetch, fetch_models_uncommitted};
pub use manager::ModelsManager;
pub use metrics::{DegradedStartCause, record_degraded_start};
pub use model_fetch_auth::ModelsCacheScope;
pub use model_fetch_auth::{CacheAuthMethod, ModelFetchAuth, task_model_error_for_catalog};
pub use prefetch::resolve_startup_endpoints;
pub use prefetch::{ResolvedModels, fetch_initial_models_blocking, start_initial_models_load};
pub use resolution::{
    ModelGlobSet, allowlist_denied_message, allowlist_excludes_all_message,
    allowlist_matches_nothing, available_models, is_campaign_only_flip, merge_additive_catalog,
    resolve_catalog_key, resolve_context_window, resolve_context_window_from_provider,
    resolve_default_model, resolve_model_catalog, selectable_catalog_key_for_persisted,
    validate_selectable,
};
pub use scope::{Commit, evaluate_commit, evaluate_models_commit, resolve_disk_auth};
pub use settings_cache::SettingsCacheManager;
pub use settings_cache::settings_cache_disabled;
pub use settings_refresh::SettingsRefresh;
pub use synthetic::{is_synthetic_listing, parse_synthetic_model_entry};

// Re-exports reached only through the manager test module.
#[cfg(any(test, feature = "test-util"))]
pub use cache::{CACHE_TTL, MODELS_CACHE_FILE, ModelsCache};
#[cfg(any(test, feature = "test-util"))]
pub use endpoint::ModelsFetchFuture;
#[cfg(any(test, feature = "test-util"))]
pub use fetch::build_prefetched_map;
#[cfg(any(test, feature = "test-util"))]
pub use metrics::degraded_log_level;
#[cfg(any(test, feature = "test-util"))]
pub use prefetch::resolve_prefetch_inputs_from_parts;
