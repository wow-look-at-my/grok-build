//! Model-catalog fetch transport.

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use super::{ModelFetchAuth, ModelsCacheManager, resolve_models_cache_scope};
use crate::agent::config::{self, ModelEntry};
use crate::remote::{FetchModelsResult, ModelSource, active_model_source};
use xai_grok_cloud_config::remote_settings::{
    ModelsPrefetch, fetch_catalog, prefetch_catalog_blocking,
};
use xai_grok_login::GrokAuth;

pub(crate) fn build_prefetched_map(
    models: Vec<config::ModelEntryConfig>,
    api_base_url_override: Option<String>,
) -> IndexMap<String, ModelEntry> {
    let mut map: IndexMap<String, ModelEntry> = IndexMap::with_capacity(models.len());
    for model in models {
        let key = model.id.clone().unwrap_or_else(|| model.model.clone());
        let info = config::ModelInfo::from_config(&model);
        let entry = ModelEntry {
            info,
            mtls_cert_dir: None,
            api_key: None,
            env_key: None,
            auth_provider: None,
            api_base_url: model.api_base_url.clone().or(api_base_url_override.clone()),
        };
        map.insert(key, entry);
    }
    map
}

pub(crate) fn prefetch_models_blocking(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    let source = active_model_source(endpoints, fetch_auth);
    let scope = resolve_models_cache_scope(endpoints, fetch_auth, auth);
    prefetch_catalog_blocking(
        &ModelsCacheManager::new(),
        &scope,
        crate::agent::remote_config::models_fetch_enabled(endpoints, auth),
        || fetched_model_map(&source, auth, endpoints, fetch_auth),
    )
}

pub(in crate::agent::remote_config) fn fetch_models_uncommitted(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
    remote_fetch_enabled: bool,
) -> ModelsPrefetch<ModelEntry> {
    let source = active_model_source(endpoints, fetch_auth);
    let scope = resolve_models_cache_scope(endpoints, fetch_auth, auth);
    fetch_catalog(
        &ModelsCacheManager::new(),
        &scope,
        remote_fetch_enabled,
        || fetched_model_map(&source, auth, endpoints, fetch_auth),
    )
}

fn fetched_model_map(
    source: &impl ModelSource,
    auth: Option<&GrokAuth>,
    endpoints: &config::EndpointsConfig,
    fetch_auth: ModelFetchAuth,
) -> Option<(IndexMap<String, ModelEntry>, Option<String>)> {
    let _timer = crate::instrumentation_timer!("startup.fetch_models_blocking");
    let fetched_at = Utc::now();
    let result = source.fetch(auth);
    prefetch_from_result(
        result,
        endpoints,
        fetch_auth,
        &source.cache_origin(),
        fetched_at,
    )
}

/// [`prefetch_models_blocking`] for a caller on a runtime. The fetch runs on the
/// async client, so it needs no blocking-pool thread.
pub(crate) async fn prefetch_models_async(
    endpoints: &config::EndpointsConfig,
    auth: Option<&GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    let scope = resolve_models_cache_scope(endpoints, fetch_auth, auth);
    let cache = ModelsCacheManager::new();
    if let Some(cached) = cache.load_fresh(&scope) {
        return Some(cached.models);
    }
    if !crate::agent::remote_config::models_fetch_enabled(endpoints, auth) {
        tracing::info!("models fetch skipped: remote_fetch disabled");
        return None;
    }
    let fetched_at = Utc::now();
    let result = crate::remote::fetch_active_models_async(endpoints, fetch_auth, auth).await;
    let (models, etag) =
        prefetch_from_result(result, endpoints, fetch_auth, &scope.origin, fetched_at)?;
    cache.persist(&models, etag.as_deref(), &scope, fetched_at);
    Some(models)
}

fn prefetch_from_result(
    result: Result<FetchModelsResult, crate::remote::BackendError>,
    endpoints: &config::EndpointsConfig,
    fetch_auth: ModelFetchAuth,
    origin: &str,
    fetched_at: DateTime<Utc>,
) -> Option<(IndexMap<String, ModelEntry>, Option<String>)> {
    match result {
        Ok(FetchModelsResult { models, etag }) if !models.is_empty() => {
            let api_base_url_override = match fetch_auth {
                ModelFetchAuth::ApiKey => Some(endpoints.xai_api_base_url.clone()),
                ModelFetchAuth::Session | ModelFetchAuth::CustomEndpoint => None,
            };
            let map = build_prefetched_map(models, api_base_url_override);
            tracing::info!(count = map.len(), etag = ?etag, "Prefetched models");
            Some((map, etag))
        }
        Ok(FetchModelsResult { .. }) => {
            tracing::error!(
                url = %origin,
                "models listing is empty: nothing to fall back on if a custom \
                 endpoint replaced the built-in defaults"
            );
            xai_grok_telemetry::unified_log::warn(
                "model catalog: the models listing is empty",
                None,
                Some(serde_json::json!({ "url": origin })),
            );
            None
        }
        Err(e) => {
            tracing::error!(
                url = %origin,
                error = ?e,
                "models fetch failed: the model picker will be empty unless \
                 `models.definitions.*` entries are configured"
            );
            xai_grok_telemetry::unified_log::warn(
                "model catalog: the models fetch failed",
                None,
                Some(serde_json::json!({
                    "url": origin,
                    "error": e.to_string(),
                    "elapsed_ms": (Utc::now() - fetched_at).num_milliseconds(),
                })),
            );
            None
        }
    }
}
