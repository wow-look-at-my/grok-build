//! Per-token pricing for a model whose endpoint reports no cost.
//!
//! Resolution order is config, then the on-disk catalog cache, then a fetch
//! from modelinfo. Config wins outright: a price the user wrote is the price,
//! and the network never overrides it.
//!
//! The lookup runs on the turn path, so it never blocks. A model with no
//! cached answer yet returns unusable pricing for this call and starts a
//! background fetch that lands for the next one.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use xai_grok_sampling_types::ModelPricing;

pub(crate) const PRICING_CACHE_FILE: &str = "model_pricing_cache.json";
/// How long a price stays good. Prices move on the scale of a release.
const POSITIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long "modelinfo does not know this model" stays good. Shorter, because
/// a model the catalog gains is worth picking up soon.
const NEGATIVE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// One model's answer. `pricing` is `None` for a model modelinfo does not
/// know. A negative answer is cached too, or every turn on an unpriced model
/// re-fetches.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct PricingCacheEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pricing: Option<ModelPricing>,
    pub(crate) fetched_at: DateTime<Utc>,
}

impl PricingCacheEntry {
    pub(crate) fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        let ttl = if self.pricing.is_some() {
            POSITIVE_TTL
        } else {
            NEGATIVE_TTL
        };
        let Ok(ttl) = chrono::Duration::from_std(ttl) else {
            return false;
        };
        let age = now.signed_duration_since(self.fetched_at);
        age >= chrono::Duration::zero() && age < ttl
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PricingCacheFile {
    #[serde(default)]
    pub(crate) entries: HashMap<String, PricingCacheEntry>,
}

/// modelinfo's per-model document. Every price is optional: a model priced
/// with no cache tier reports null there.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ModelinfoDocument {
    #[serde(default)]
    pub(crate) input_cost_per_token: Option<f64>,
    #[serde(default)]
    pub(crate) output_cost_per_token: Option<f64>,
    #[serde(default)]
    pub(crate) cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub(crate) cache_creation_input_token_cost: Option<f64>,
}

impl ModelinfoDocument {
    /// The four tiers `ModelPricing` bills on. A document that prices nothing
    /// gives `None`, which is recorded as a negative answer rather than as
    /// an all-zero price the cost path would read as configured.
    pub(crate) fn to_pricing(&self) -> Option<ModelPricing> {
        let pricing = ModelPricing {
            input_per_token_usd: self.input_cost_per_token.unwrap_or(0.0),
            output_per_token_usd: self.output_cost_per_token.unwrap_or(0.0),
            cached_read_per_token_usd: self.cache_read_input_token_cost.unwrap_or(0.0),
            cache_creation_per_token_usd: self.cache_creation_input_token_cost.unwrap_or(0.0),
        };
        (!pricing.is_unusable()).then_some(pricing)
    }
}

/// The process's view of the catalog. Hydrated from disk on first use.
struct Store {
    entries: HashMap<String, PricingCacheEntry>,
    /// Models with a fetch in flight. A second turn on the same model must
    /// not start a second request.
    in_flight: HashSet<String>,
    loaded_from_disk: bool,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(Store {
            entries: HashMap::new(),
            in_flight: HashSet::new(),
            loaded_from_disk: false,
        })
    })
}

fn cache_path() -> std::path::PathBuf {
    crate::util::grok_home::grok_home().join(PRICING_CACHE_FILE)
}

fn read_cache_file(path: &std::path::Path) -> PricingCacheFile {
    let Ok(data) = std::fs::read(path) else {
        return PricingCacheFile::default();
    };
    serde_json::from_slice(&data).unwrap_or_default()
}

/// Merge one model's answer into the file and rewrite it. Read-modify-write
/// under a fresh read so a sibling process's entries survive this one.
fn persist_entry(path: &std::path::Path, model_id: &str, entry: &PricingCacheEntry) {
    let mut file = read_cache_file(path);
    file.entries.insert(model_id.to_string(), entry.clone());
    let Ok(json) = serde_json::to_vec_pretty(&file) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    if std::fs::write(&tmp, &json).is_ok() {
        if std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Resolve `model_id`'s per-token pricing.
///
/// Config first. Then the cache. A miss starts a background fetch and answers
/// with unusable pricing, which keeps the cost honestly absent for this call
/// rather than stalling the turn on the network.
pub(crate) fn resolve(model_id: &str) -> ModelPricing {
    let configured = crate::agent::config::resolve_configured_pricing(model_id);
    if !configured.model.is_unusable() {
        return configured.model;
    }
    if model_id.is_empty() || !configured.lookup_enabled || lookup_suppressed(model_id) {
        return configured.model;
    }
    cached_or_schedule(model_id, &configured.catalog_url).unwrap_or(configured.model)
}

/// Model ids no lookup may run for, registered at catalog build.
///
/// A model DISCOVERED from a local runtime is not in config, so
/// `resolve_configured_pricing` — which rebuilds the catalog from config
/// alone — cannot see its `pricing_lookup_enabled = false`. Without this the
/// session asks the modelinfo catalog to price `qwen3-coder:30b` on every TTL
/// expiry, forever, and every one of those requests is a 404 by construction:
/// the model runs on this machine, charges nothing, and is in no catalog.
fn suppressed() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static SUPPRESSED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SUPPRESSED.get_or_init(Default::default)
}

/// Register model ids whose price must never be looked up. Additive: a
/// catalog rebuild that drops a provider leaves its ids registered, which
/// costs nothing and keeps a rebuild from re-enabling a lookup mid-session.
pub(crate) fn suppress_lookup_for(model_ids: impl IntoIterator<Item = String>) {
    let Ok(mut guard) = suppressed().lock() else {
        return;
    };
    guard.extend(model_ids);
}

fn lookup_suppressed(model_id: &str) -> bool {
    suppressed()
        .lock()
        .map(|guard| guard.contains(model_id))
        .unwrap_or(false)
}

/// The cached answer for `model_id`, or `None` after arranging a fetch.
fn cached_or_schedule(model_id: &str, base_url: &str) -> Option<ModelPricing> {
    let now = Utc::now();
    let mut guard = store().lock().ok()?;
    if !guard.loaded_from_disk {
        guard.entries = read_cache_file(&cache_path()).entries;
        guard.loaded_from_disk = true;
    }
    if let Some(entry) = guard.entries.get(model_id) {
        if entry.is_fresh(now) {
            return entry.pricing.clone();
        }
    }
    if !guard.in_flight.insert(model_id.to_string()) {
        return None;
    }
    drop(guard);
    spawn_fetch(model_id.to_string(), base_url.to_string());
    None
}

/// Run one fetch on its own thread. The turn path is sync and may hold no
/// tokio runtime, so this owns a thread rather than a spawned task.
fn spawn_fetch(model_id: String, base_url: String) {
    let owned = model_id.clone();
    let spawned = std::thread::Builder::new()
        .name("model-pricing-fetch".to_string())
        .spawn(move || {
            let fetched = fetch_pricing_blocking(&base_url, &model_id);
            match fetched {
                Ok(pricing) => {
                    let entry = PricingCacheEntry {
                        pricing,
                        fetched_at: Utc::now(),
                    };
                    persist_entry(&cache_path(), &model_id, &entry);
                    if let Ok(mut guard) = store().lock() {
                        guard.entries.insert(model_id.clone(), entry);
                    }
                }
                Err(e) => {
                    tracing::debug!(model = %model_id, error = %e, "model pricing lookup failed");
                }
            }
            if let Ok(mut guard) = store().lock() {
                guard.in_flight.remove(&model_id);
            }
        });
    // A thread that never started still holds the slot. Release it, or this
    // model never gets another attempt for the life of the process.
    if spawned.is_err() {
        if let Ok(mut guard) = store().lock() {
            guard.in_flight.remove(&owned);
        }
    }
}

/// `Ok(None)` means modelinfo answered and knows no price for this model.
/// `Err` means the lookup itself failed, which is not an answer and is not
/// cached.
pub(crate) fn fetch_pricing_blocking(
    base_url: &str,
    model_id: &str,
) -> Result<Option<ModelPricing>, String> {
    let url = format!(
        "{}/v1/models/{}",
        base_url.trim_end_matches('/'),
        model_id.trim_start_matches('/')
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("modelinfo answered {}", response.status()));
    }
    let document: ModelinfoDocument = response.json().map_err(|e| e.to_string())?;
    Ok(document.to_pricing())
}

#[cfg(test)]
mod tests;
