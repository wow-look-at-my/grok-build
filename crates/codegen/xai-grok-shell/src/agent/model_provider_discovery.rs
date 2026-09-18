//! Model autodetection for `[model_providers.<id>]`.
//!
//! A provider declares a base URL. Its models are what that base lists at
//! `/models`. Asking for them is the default, so a provider needs no
//! `[model.<id>]` block per model. `models_autodetect = false` turns it off for
//! a provider whose listing is too large to pick from.

use indexmap::IndexMap;

use crate::agent::config::{self, ConfigModelOverride, ModelEntry};
use crate::agent::model_providers::ModelProviderConfig;

/// Deadline for one provider's listing. Discovery is additive and runs off the
/// startup path, but an unreachable provider must not hold a thread forever.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Catalog key for a discovered model: the provider id, then the routing slug.
/// The provider qualifies the key so two providers that list the same slug stay
/// two entries, and so a user's own `[model.<slug>]` block is never shadowed.
pub(crate) fn discovered_model_key(provider_id: &str, slug: &str) -> String {
    format!("{provider_id}/{slug}")
}

/// Ask every autodetecting provider for its models.
///
/// Returns one additive catalog for all of them. A provider that declares no
/// endpoint, that cannot be reached, or that answers with an empty listing
/// contributes nothing and never fails the others.
pub(crate) async fn discover_provider_models(
    cfg: &config::Config,
) -> IndexMap<String, ModelEntry> {
    let mut discovered = IndexMap::new();
    for (id, provider) in &cfg.model_providers {
        if !provider.autodetect_enabled() {
            tracing::debug!(provider = %id, "model autodetection is off for this provider");
            continue;
        }
        let Some(url) = provider.resolve_models_list_url() else {
            tracing::debug!(
                provider = %id,
                "no base_url, api_base_url or models_list_url: nothing to ask for a model list"
            );
            continue;
        };
        let models = discover_one_provider(cfg, id, provider, &url).await;
        discovered.extend(models);
    }
    discovered
}

/// One provider's listing, resolved into catalog entries.
async fn discover_one_provider(
    cfg: &config::Config,
    provider_id: &str,
    provider: &ModelProviderConfig,
    url: &str,
) -> IndexMap<String, ModelEntry> {
    // The credential comes from the provider's own fields, resolved through the
    // same merge an inheriting `[model.<id>]` gets. A provider that mints its
    // token with a helper needs that helper run first: the cache is cold at
    // startup, and a cold cache reads as no credential.
    let probe = provider_entry(cfg, provider_id, provider);
    let api_key = match probe.own_credential() {
        Some(key) => Some(key),
        None => match probe.effective_auth_provider() {
            Some(auth) => {
                // The mint writes the token into the provider's own slot. The
                // outcome describes a wire key this caller does not hold.
                let _ = auth.ensure_fresh_token(None).await;
                auth.cached_token()
            }
            None => None,
        },
    };

    let listing = fetch_listing(url, api_key.as_deref()).await;
    let listing = match listing {
        Ok(models) => models,
        Err(error) => {
            // Name the URL. A 404 here usually means the base serves inference
            // and no listing, which is a different fix from a bad key.
            tracing::warn!(
                provider = %provider_id,
                url = %url,
                %error,
                "model autodetection failed; this provider contributes no models"
            );
            return IndexMap::new();
        }
    };

    let mut entries = IndexMap::with_capacity(listing.len());
    for listed in listing {
        let key = discovered_model_key(provider_id, &listed.model);
        let override_for_listed = ConfigModelOverride {
            model: Some(listed.model.clone()),
            name: listed.name.clone(),
            description: listed.description.clone(),
            api_backend: Some(listed.api_backend.clone()),
            context_window: Some(listed.context_window.get()),
            model_provider: Some(provider_id.to_owned()),
            reasoning_efforts: listed.reasoning_efforts.clone(),
            supports_reasoning_effort: listed
                .supports_reasoning_effort
                .then_some(true),
            ..Default::default()
        };
        entries.insert(
            key.clone(),
            config::entry_for_provider_model(cfg, &key, provider_id, provider, &override_for_listed),
        );
    }
    tracing::info!(
        provider = %provider_id,
        url = %url,
        count = entries.len(),
        "autodetected models from the provider's listing"
    );
    entries
}

/// A credential-only stand-in for the provider: every connection and auth field
/// the provider declares, with no model of its own.
fn provider_entry(
    cfg: &config::Config,
    provider_id: &str,
    provider: &ModelProviderConfig,
) -> ModelEntry {
    let probe = ConfigModelOverride {
        model_provider: Some(provider_id.to_owned()),
        ..Default::default()
    };
    config::entry_for_provider_model(cfg, provider_id, provider_id, provider, &probe)
}

/// Fetch and parse a listing off the async path.
///
/// `reqwest::blocking` builds its own runtime, which panics when it is
/// constructed inside an async context, so the request runs on a dedicated OS
/// thread. That is the same reason `resolve_context_window_from_provider`
/// spawns one.
async fn fetch_listing(
    url: &str,
    api_key: Option<&str>,
) -> Result<Vec<config::ModelEntryConfig>, String> {
    let (url, key) = (url.to_owned(), api_key.map(str::to_owned));
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = crate::remote::fetch_models_for_list_url_blocking(&url, key.as_deref());
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    match tokio::time::timeout(FETCH_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the listing thread ended without an answer".to_owned()),
        Err(_) => Err(format!("no answer within {}s", FETCH_TIMEOUT.as_secs())),
    }
}
