//! Model autodetection for `[model_providers.<id>]`.
//!
//! A provider declares a base URL. Its models are what that base lists at
//! `/models`. Asking for them is the default, so a provider needs no
//! `[model.<id>]` block per model. `models_autodetect = false` turns it off for
//! a provider whose listing is too large to pick from.

use indexmap::IndexMap;

use crate::agent::config::{self, ConfigModelOverride, ModelEntry};
use crate::agent::model_providers::{ContextWindowSource, ModelProviderConfig, ModelsListDialect};

/// Deadline for one provider's listing. Discovery is additive and runs off the
/// startup path, but an unreachable provider must not hold a thread forever.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Catalog key for a discovered model: the provider id, then the routing slug.
/// The provider qualifies the key so two providers that list the same slug stay
/// two entries, and so a user's own `[model.<slug>]` block is never shadowed.
pub(crate) fn discovered_model_key(provider_id: &str, slug: &str) -> String {
    format!("{provider_id}/{slug}")
}

/// One model a provider's listing named, before any config is merged into it.
///
/// The entry is built at catalog rebuild time from the CURRENT config
/// (`resolve_discovered_models`). So an edit to a `[model.<id>]` block that
/// claims this model takes effect on reload, with no second listing request.
#[derive(Clone, Debug)]
pub(crate) struct DiscoveredModel {
    pub(crate) provider_id: String,
    /// What the listing says, with the provider's own window, backend and
    /// price switch already placed above the listing's values.
    pub(crate) listed: ConfigModelOverride,
    /// Runtime state, not config. The residency poll writes it here.
    pub(crate) loaded_in_vram: Option<bool>,
}

impl DiscoveredModel {
    fn slug(&self) -> &str {
        self.listed.model.as_deref().unwrap_or_default()
    }
}

/// Build the catalog entries for what discovery found.
///
/// A `[model.<id>]` block that routes to a listed model is merged with it,
/// under the block's own key. Every field the block sets wins. The listing
/// fills only the fields the block left unset, so a block that only renames a
/// model still gets the window and capabilities the runtime reported.
pub(crate) fn resolve_discovered_models(
    cfg: &config::Config,
    discovered: &IndexMap<String, DiscoveredModel>,
) -> IndexMap<String, ModelEntry> {
    let mut entries = IndexMap::with_capacity(discovered.len());
    for (discovered_key, model) in discovered {
        let Some(provider) = cfg.model_providers.get(&model.provider_id) else {
            // A reload removed the provider. Its models go with it.
            tracing::debug!(
                provider = %model.provider_id,
                model = %model.slug(),
                "provider no longer configured; dropping its discovered model"
            );
            continue;
        };
        let (key, merged) = match claiming_block(cfg, &model.provider_id, model.slug()) {
            Some((block_key, block)) => (block_key.clone(), block.laid_over(&model.listed)),
            None => (discovered_key.clone(), model.listed.clone()),
        };
        let mut entry =
            config::entry_for_provider_model(cfg, &key, &model.provider_id, provider, &merged);
        entry.info.loaded_in_vram = model.loaded_in_vram;
        entries.insert(key, entry);
    }
    entries
}

impl ConfigModelOverride {
    /// `self` over `base`, field by field: every value `self` sets wins, and
    /// `base` fills the rest. Maps merge per key. Credentials move as one set,
    /// for the reason `with_provider_defaults` gives.
    pub(crate) fn laid_over(&self, base: &ConfigModelOverride) -> ConfigModelOverride {
        let ConfigModelOverride {
            model,
            base_url,
            name,
            description,
            api_key,
            env_key,
            auth_provider,
            model_provider,
            api_base_url,
            max_completion_tokens,
            temperature,
            top_p,
            api_backend,
            extra_headers,
            query_params,
            env_http_headers,
            context_window,
            auto_compact_threshold_percent,
            system_prompt_label,
            use_concise,
            agent_type,
            inference_idle_timeout_secs,
            max_retries,
            hidden,
            supported_in_api,
            reasoning_effort,
            supports_reasoning_effort,
            reasoning_efforts,
            supports_backend_search,
            compactions_remaining,
            compaction_at_tokens,
            show_model_fingerprint,
            stream_tool_calls,
            strict_message_schema,
            pricing,
            min_output_tokens_per_sec,
            ttft_timeout_secs,
            extra_body,
            pricing_lookup_enabled,
        } = self.clone();

        let sets_own_auth = api_key.as_deref().is_some_and(|k| !k.trim().is_empty())
            || env_key
                .as_ref()
                .and_then(config::EnvKeys::primary)
                .is_some()
            || auth_provider.is_some();
        let (api_key, env_key, auth_provider) = if sets_own_auth {
            (api_key, env_key, auth_provider)
        } else {
            (
                base.api_key.clone(),
                base.env_key.clone(),
                base.auth_provider.clone(),
            )
        };

        let mut merged_headers = extra_headers;
        crate::agent::model_providers::inherit_headers(&mut merged_headers, &base.extra_headers);
        let mut merged_env_headers = env_http_headers;
        crate::agent::model_providers::inherit_headers(
            &mut merged_env_headers,
            &base.env_http_headers,
        );
        let mut merged_query = query_params;
        for (k, v) in &base.query_params {
            merged_query.entry(k.clone()).or_insert_with(|| v.clone());
        }
        let mut merged_body = extra_body;
        for (k, v) in &base.extra_body {
            merged_body.entry(k.clone()).or_insert_with(|| v.clone());
        }

        ConfigModelOverride {
            model: model.or_else(|| base.model.clone()),
            base_url: base_url.or_else(|| base.base_url.clone()),
            name: name.or_else(|| base.name.clone()),
            description: description.or_else(|| base.description.clone()),
            api_key,
            env_key,
            auth_provider,
            model_provider: model_provider.or_else(|| base.model_provider.clone()),
            api_base_url: api_base_url.or_else(|| base.api_base_url.clone()),
            max_completion_tokens: max_completion_tokens.or(base.max_completion_tokens),
            temperature: temperature.or(base.temperature),
            top_p: top_p.or(base.top_p),
            api_backend: api_backend.or_else(|| base.api_backend.clone()),
            extra_headers: merged_headers,
            query_params: merged_query,
            env_http_headers: merged_env_headers,
            context_window: context_window.or(base.context_window),
            auto_compact_threshold_percent: auto_compact_threshold_percent
                .or(base.auto_compact_threshold_percent),
            system_prompt_label: system_prompt_label.or_else(|| base.system_prompt_label.clone()),
            use_concise: use_concise.or(base.use_concise),
            agent_type: agent_type.or_else(|| base.agent_type.clone()),
            inference_idle_timeout_secs: inference_idle_timeout_secs
                .or(base.inference_idle_timeout_secs),
            max_retries: max_retries.or(base.max_retries),
            hidden: hidden.or(base.hidden),
            supported_in_api: supported_in_api.or(base.supported_in_api),
            reasoning_effort: reasoning_effort.or(base.reasoning_effort),
            supports_reasoning_effort: supports_reasoning_effort.or(base.supports_reasoning_effort),
            // A menu turns support on (`derive_reasoning_effort_fields`). So a
            // block that turns support off must not inherit the base's menu.
            reasoning_efforts: if !reasoning_efforts.is_empty()
                || supports_reasoning_effort == Some(false)
            {
                reasoning_efforts
            } else {
                base.reasoning_efforts.clone()
            },
            supports_backend_search: supports_backend_search.or(base.supports_backend_search),
            compactions_remaining: compactions_remaining.or(base.compactions_remaining),
            compaction_at_tokens: compaction_at_tokens.or(base.compaction_at_tokens),
            show_model_fingerprint: show_model_fingerprint.or(base.show_model_fingerprint),
            stream_tool_calls: stream_tool_calls.or(base.stream_tool_calls),
            strict_message_schema: strict_message_schema.or(base.strict_message_schema),
            pricing: pricing.or_else(|| base.pricing.clone()),
            min_output_tokens_per_sec: min_output_tokens_per_sec.or(base.min_output_tokens_per_sec),
            ttft_timeout_secs: ttft_timeout_secs.or(base.ttft_timeout_secs),
            extra_body: merged_body,
            pricing_lookup_enabled: pricing_lookup_enabled.or(base.pricing_lookup_enabled),
        }
    }
}

/// Ask every autodetecting provider for its models.
///
/// Returns what every listing named, keyed `<provider>/<slug>`. A provider that
/// declares no endpoint, that cannot be reached, or that answers with an empty
/// listing contributes nothing and never fails the others.
pub(crate) async fn discover_provider_models(
    cfg: &config::Config,
) -> IndexMap<String, DiscoveredModel> {
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
) -> IndexMap<String, DiscoveredModel> {
    // The credential comes from the provider's own fields, resolved through the
    // same merge an inheriting `[model.<id>]` gets. A provider that mints its
    // token with a helper needs that helper run first: the cache is cold at
    // startup, and a cold cache reads as no credential.
    let probe = config::provider_probe_entry(cfg, provider_id, provider);
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

    let dialect = provider.dialect();
    let window_source = provider.window_source();
    let inference_base = provider
        .base_url
        .as_deref()
        .or(provider.api_base_url.as_deref())
        .unwrap_or(url)
        .trim_end_matches('/')
        .to_owned();
    let listing = fetch_listing(
        dialect,
        url,
        &inference_base,
        api_key.as_deref(),
        window_source,
    )
    .await;
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
            // A value the user wrote on the provider is the value. Listing
            // fields are the fallback, and a listing that named no window
            // carries the client default, which is a guess about a provider
            // whose own block states the answer.
            api_backend: provider
                .api_backend
                .clone()
                .or_else(|| Some(listed.api_backend.clone())),
            context_window: provider
                .context_window
                .or_else(|| Some(listed.context_window.get())),
            model_provider: Some(provider_id.to_owned()),
            reasoning_efforts: listed.reasoning_efforts.clone(),
            supports_reasoning_effort: listed.supports_reasoning_effort.then_some(true),
            // A local runtime charges nothing and its model names are in no
            // catalog, so the price lookup there is a request that can only
            // fail. The provider's own value wins where it wrote one.
            pricing_lookup_enabled: provider
                .pricing_lookup_enabled
                .or_else(|| dialect.is_local_runtime().then_some(false)),
            ..Default::default()
        };
        entries.insert(
            key,
            DiscoveredModel {
                provider_id: provider_id.to_owned(),
                listed: override_for_listed,
                loaded_in_vram: listed.loaded_in_vram,
            },
        );
    }
    if dialect.is_local_runtime() {
        // A local model charges nothing and is in no catalog, so the price
        // lookup can only 404. `resolve_configured_pricing` rebuilds the
        // catalog from config alone and never sees a DISCOVERED model, so the
        // suppression has to be registered here.
        crate::agent::model_pricing::suppress_lookup_for(
            entries.values().map(|m| m.slug().to_owned()),
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

/// How long a residency answer is good for. A model loads on its first
/// request and LM Studio's idle TTL unloads it again, so the dot is stale
/// within minutes of a catalog build. One localhost request per provider per
/// tick is cheap; the poll is what makes the dot mean "right now".
pub(crate) const RESIDENCY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether any configured provider can report residency at all. Nothing polls
/// for a session with only remote providers.
pub(crate) fn has_local_runtime(cfg: &config::Config) -> bool {
    cfg.model_providers
        .values()
        .any(|p| p.autodetect_enabled() && p.dialect().is_local_runtime())
}

/// Re-read which of every local provider's models are resident, keyed by the
/// catalog key discovery gave them.
///
/// Only residency: the window, the capabilities and the price do not change
/// while the runtime is up, and re-reading them costs an `/api/show` per
/// model.
pub(crate) async fn refresh_local_residency(cfg: &config::Config) -> IndexMap<String, bool> {
    let mut out = IndexMap::new();
    for (id, provider) in &cfg.model_providers {
        let dialect = provider.dialect();
        if !provider.autodetect_enabled() || !dialect.is_local_runtime() {
            continue;
        }
        let Some(base) = provider
            .base_url
            .clone()
            .or_else(|| provider.api_base_url.clone())
        else {
            continue;
        };
        let probe = config::provider_probe_entry(cfg, id, provider);
        let api_key = probe.own_credential();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result =
                crate::remote::fetch_residency_blocking(dialect, &base, api_key.as_deref());
            let _ = tx.send(result);
        });
        let answer = match tokio::time::timeout(FETCH_TIMEOUT, rx).await {
            Ok(Ok(Ok(map))) => map,
            // An unreachable runtime leaves the dots as they were. Reporting
            // every model cold would claim an unload nothing observed.
            other => {
                tracing::debug!(provider = %id, "no residency answer: {other:?}");
                continue;
            }
        };
        for (slug, loaded) in answer {
            out.insert(discovered_model_key(id, &slug), loaded);
        }
    }
    out
}

/// The `[model.<id>]` block that routes to `slug` on this provider, if any.
///
/// The block's own key counts too: `[model.claude-sonnet] model_provider =
/// "gateway"` with no `model` field routes to the key.
///
/// A block that names no provider also claims the model when its URL is the
/// provider's URL. It routes to the same endpoint and slug, so without the
/// claim the picker shows the same model twice.
fn claiming_block<'a>(
    cfg: &'a config::Config,
    provider_id: &str,
    slug: &str,
) -> Option<(&'a String, &'a ConfigModelOverride)> {
    let provider = cfg.model_providers.get(provider_id);
    cfg.config_models.iter().find(|(key, model_override)| {
        if model_override.model.as_deref().unwrap_or(key.as_str()) != slug {
            return false;
        }
        match model_override.model_provider.as_deref() {
            Some(named) => named == provider_id,
            None => provider.is_some_and(|p| same_endpoint(model_override, p)),
        }
    })
}

/// Whether a block with no provider points at this provider's endpoint.
fn same_endpoint(block: &ConfigModelOverride, provider: &ModelProviderConfig) -> bool {
    let block_urls = [block.base_url.as_deref(), block.api_base_url.as_deref()];
    let provider_urls = [
        provider.base_url.as_deref(),
        provider.api_base_url.as_deref(),
    ];
    block_urls.into_iter().flatten().any(|b| {
        provider_urls
            .into_iter()
            .flatten()
            .any(|p| normalize_url(b) == normalize_url(p))
    })
}

/// A URL with its case and trailing slashes removed, for comparison only.
fn normalize_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Fetch and parse a listing off the async path.
///
/// `reqwest::blocking` builds its own runtime, which panics when it is
/// constructed inside an async context, so the request runs on a dedicated OS
/// thread. That is the same reason `resolve_context_window_from_provider`
/// spawns one.
async fn fetch_listing(
    dialect: ModelsListDialect,
    url: &str,
    inference_base_url: &str,
    api_key: Option<&str>,
    window_source: ContextWindowSource,
) -> Result<Vec<config::ModelEntryConfig>, String> {
    let (url, key) = (url.to_owned(), api_key.map(str::to_owned));
    let inference_base_url = inference_base_url.to_owned();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = match dialect {
            // The OpenAI dialect asks the listing URL as written, which may be
            // a `models_list_url` that lives nowhere near the inference base.
            ModelsListDialect::Openai => {
                crate::remote::fetch_models_for_list_url_blocking(&url, key.as_deref())
            }
            local => crate::remote::fetch_local_listing_blocking(
                local,
                &inference_base_url,
                &inference_base_url,
                key.as_deref(),
                window_source,
            ),
        };
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    match tokio::time::timeout(FETCH_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the listing thread ended without an answer".to_owned()),
        Err(_) => Err(format!("no answer within {}s", FETCH_TIMEOUT.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loopback listing at both the conventional path and a custom one, so a
    /// test can prove which URL the discovery asked for.
    async fn start_listing_server(
        body: serde_json::Value,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::get;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let for_custom = body.clone();
        let app = axum::Router::new()
            .route("/v1/models", get(move || async move { axum::Json(body) }))
            .route(
                "/catalog.json",
                get(move || async move { axum::Json(for_custom) }),
            );
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, handle)
    }

    fn two_model_listing() -> serde_json::Value {
        serde_json::json!({
            "data": [
                { "model": "big-one", "contextWindow": 1_000_000 },
                { "model": "small-one", "contextWindow": 128_000 },
            ]
        })
    }

    /// A loopback Ollama: `/api/tags`, `/api/ps` and `/api/show`, which is
    /// the whole set the dialect reads.
    async fn start_ollama_server() -> (String, tokio::task::JoinHandle<()>) {
        use axum::routing::{get, post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let app = axum::Router::new()
            .route(
                "/api/tags",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "models": [
                            {"name": "qwen3-coder:30b", "model": "qwen3-coder:30b",
                             "details": {"parameter_size": "30B", "quantization_level": "Q4_K_M"}},
                            {"name": "llama3.2:latest", "model": "llama3.2:latest"},
                        ]
                    }))
                }),
            )
            .route(
                "/api/ps",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "models": [{
                            "name": "qwen3-coder:30b", "model": "qwen3-coder:30b",
                            "size_vram": 21_474_836_480u64, "context_length": 32768,
                        }]
                    }))
                }),
            )
            .route(
                "/api/show",
                post(|body: axum::Json<serde_json::Value>| async move {
                    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("");
                    let arch = if model.starts_with("qwen") {
                        "qwen2"
                    } else {
                        "llama"
                    };
                    axum::Json(serde_json::json!({
                        "capabilities": ["completion", "tools", "thinking"],
                        "model_info": {
                            "general.architecture": arch,
                            format!("{arch}.context_length"): 131072,
                        },
                    }))
                }),
            );
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, handle)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_ollama_dialect_reads_the_loaded_window_and_the_residency() {
        let (base, server) = start_ollama_server().await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.ollama]
            base_url = "{base}/v1"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        let loaded = discovered
            .get("ollama/qwen3-coder:30b")
            .expect("the tag listing's models are discovered");
        assert_eq!(
            loaded.info.context_window.get(),
            32768,
            "the loaded runner's window is what inference enforces, not the model's 131072 maximum"
        );
        assert_eq!(loaded.info.loaded_in_vram, Some(true));
        assert!(
            loaded.info.supports_reasoning_effort,
            "`thinking` in the capabilities opens the effort gate"
        );
        assert!(
            !loaded.info.pricing_lookup_enabled,
            "a local model is in no catalog, so its price is never looked up"
        );

        let cold = discovered
            .get("ollama/llama3.2:latest")
            .expect("an unloaded model is still listed");
        assert_eq!(cold.info.loaded_in_vram, Some(false));
        assert_eq!(
            cold.info.context_window.get(),
            131_072,
            "nothing is loaded, so the model's own maximum stands in — never the client default"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn residency_is_re_read_without_re_reading_anything_else() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let loaded = Arc::new(AtomicBool::new(false));
        // `/api/show` is the expensive call — one per model — and the poll
        // must never make it: only residency changes while the runtime is up.
        let show_calls = Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let ps_loaded = Arc::clone(&loaded);
        let counted = Arc::clone(&show_calls);
        let app = axum::Router::new()
            .route(
                "/api/tags",
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "models": [{"name": "m:latest", "model": "m:latest"}]
                    }))
                }),
            )
            .route(
                "/api/ps",
                axum::routing::get(move || {
                    let loaded = Arc::clone(&ps_loaded);
                    async move {
                        let models = if loaded.load(Ordering::SeqCst) {
                            serde_json::json!([{
                                "name": "m:latest", "model": "m:latest",
                                "size_vram": 1024, "context_length": 8192
                            }])
                        } else {
                            serde_json::json!([])
                        };
                        axum::Json(serde_json::json!({ "models": models }))
                    }
                }),
            )
            .route(
                "/api/show",
                axum::routing::post(move |_: axum::Json<serde_json::Value>| {
                    let counted = Arc::clone(&counted);
                    async move {
                        counted.fetch_add(1, Ordering::SeqCst);
                        axum::Json(serde_json::json!({
                            "capabilities": ["completion"],
                            "model_info": {
                                "general.architecture": "llama",
                                "llama.context_length": 4096,
                            },
                        }))
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = config_from(&format!(
            r#"
            [model_providers.ollama]
            base_url = "{base}/v1"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        assert_eq!(
            discovered
                .get("ollama/m:latest")
                .unwrap()
                .info
                .loaded_in_vram,
            Some(false)
        );
        let show_after_discovery = show_calls.load(Ordering::SeqCst);
        assert!(show_after_discovery > 0, "discovery reads the window");

        loaded.store(true, Ordering::SeqCst);
        let residency = refresh_local_residency(&cfg).await;
        server.abort();

        assert_eq!(
            residency.get("ollama/m:latest"),
            Some(&true),
            "the poll sees the model that just loaded"
        );
        assert_eq!(
            show_calls.load(Ordering::SeqCst),
            show_after_discovery,
            "the poll re-reads residency only; /api/show is one request per model"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_runtime_reports_no_residency_rather_than_cold() {
        let cfg = config_from(
            r#"
            [model_providers.ollama]
            base_url = "http://127.0.0.1:1/v1"
            "#,
        );

        let residency = refresh_local_residency(&cfg).await;

        assert!(
            residency.is_empty(),
            "an empty answer leaves the dots alone; reporting cold claims an unload nobody saw"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_a_local_provider_is_worth_polling() {
        assert!(has_local_runtime(&config_from(
            "[model_providers.ollama]\n"
        )));
        assert!(!has_local_runtime(&config_from(
            r#"
            [model_providers.gateway]
            base_url = "https://gateway.example/v1"
            "#
        )));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_provider_id_alone_configures_a_local_runtime() {
        let cfg = config_from("[model_providers.ollama]\n");
        let provider = cfg
            .model_providers
            .get("ollama")
            .expect("the block is parsed");

        assert_eq!(
            provider.base_url.as_deref(),
            Some("http://localhost:11434/v1"),
            "a well-known id carries its own default endpoint"
        );
        assert_eq!(provider.dialect(), ModelsListDialect::Ollama);
        assert_eq!(provider.pricing_lookup_enabled, Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_written_value_beats_the_preset() {
        let cfg = config_from(
            r#"
            [model_providers.ollama]
            base_url = "http://box.local:9999/v1"
            models_list_dialect = "openai"
            "#,
        );
        let provider = cfg.model_providers.get("ollama").expect("the block");

        assert_eq!(
            provider.base_url.as_deref(),
            Some("http://box.local:9999/v1")
        );
        assert_eq!(
            provider.dialect(),
            ModelsListDialect::Openai,
            "the preset fills only what the user left unset"
        );
    }

    fn config_from(toml_text: &str) -> config::Config {
        let raw: toml::Value = toml::from_str(toml_text).expect("test config should parse");
        config::Config::new_from_toml_cfg(&raw).expect("test config should build")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_provider_contributes_the_models_its_listing_names() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"
            api_key = "sk-provider"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        let big = discovered
            .get("gateway/big-one")
            .expect("the listing's models are keyed by provider and slug");
        assert_eq!(big.info.model, "big-one");
        assert_eq!(big.info.context_window.get(), 1_000_000);
        assert_eq!(big.info.base_url, format!("{base}/v1"));
        assert_eq!(big.info.model_provider.as_deref(), Some("gateway"));
        assert_eq!(
            config::resolve_credentials(big, Some("session-jwt"))
                .api_key
                .as_deref(),
            Some("sk-provider"),
            "a discovered model inherits the provider's credential, and the \
             session token never reaches the provider's endpoint"
        );
        assert!(discovered.contains_key("gateway/small-one"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_providers_own_window_and_backend_beat_the_listing() {
        // `small-one` names a window, `no-window-one` names none and so carries
        // the client default. The provider states both answers itself.
        let listing = serde_json::json!({
            "data": [
                { "model": "small-one", "contextWindow": 128_000 },
                { "model": "no-window-one" },
            ]
        });
        let (base, server) = start_listing_server(listing).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"
            context_window = 1048576
            api_backend = "messages"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        for key in ["gateway/small-one", "gateway/no-window-one"] {
            let entry = discovered.get(key).expect("the listing's models");
            assert_eq!(
                entry.info.context_window.get(),
                1_048_576,
                "{key}: the window the user wrote is the window"
            );
            assert_eq!(
                entry.info.api_backend,
                crate::sampling::ApiBackend::Messages,
                "{key}: the backend the user wrote is the backend"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn autodetection_off_asks_for_nothing() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"
            models_autodetect = false
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            discovered.is_empty(),
            "a provider that flooded the picker must stay off: {:?}",
            discovered.keys().collect::<Vec<_>>()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_custom_list_url_is_asked_verbatim() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        // The base serves no listing at all. Only the custom URL answers, so a
        // discovery that derived `<base>/models` from it finds nothing.
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/inference"
            models_list_url = "{base}/catalog.json"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            discovered.contains_key("gateway/big-one"),
            "the listing named by models_list_url is the one asked for"
        );
        assert_eq!(
            discovered["gateway/big-one"].info.base_url,
            format!("{base}/inference"),
            "the models still route to the provider's own base, not to the listing"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_provider_contributes_nothing_and_fails_no_other() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.dead]
            base_url = "http://127.0.0.1:1/v1"

            [model_providers.gateway]
            base_url = "{base}/v1"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            !discovered.keys().any(|k| k.starts_with("dead/")),
            "an unreachable provider contributes no models"
        );
        assert!(
            discovered.contains_key("gateway/big-one"),
            "and it does not take the reachable provider down with it"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_model_the_user_wrote_a_block_for_is_not_listed_twice() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"

            [model.my-big-one]
            model = "big-one"
            model_provider = "gateway"
            context_window = 999
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            !discovered.contains_key("gateway/big-one"),
            "the user's own block owns that model"
        );
        assert!(
            discovered.contains_key("my-big-one"),
            "the merged entry keeps the block's key"
        );
        assert!(
            discovered.contains_key("gateway/small-one"),
            "the rest of the listing still arrives"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_block_on_the_providers_url_claims_its_model_without_naming_it() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        // The block names the provider's URL, with a different case and a
        // trailing slash, and no `model_provider`.
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"

            [model.big-build]
            model = "big-one"
            name = "Big Build"
            base_url = "{upper}/V1/"
            "#,
            upper = base.to_uppercase(),
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            !discovered.contains_key("gateway/big-one"),
            "the block routes to the same endpoint and slug, so it owns the model"
        );
        let merged = discovered
            .get("big-build")
            .expect("merged under the block's key");
        assert_eq!(merged.info.name.as_deref(), Some("Big Build"));
        assert_eq!(
            merged.info.context_window.get(),
            1_000_000,
            "the listing fills the window the block left unset"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_block_on_another_url_does_not_claim_the_model() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"

            [model.big-elsewhere]
            model = "big-one"
            base_url = "https://elsewhere.example/v1"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server.abort();

        assert!(
            discovered.contains_key("gateway/big-one"),
            "another endpoint is another route, so both stay"
        );
        assert!(!discovered.contains_key("big-elsewhere"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_block_claims_only_the_provider_on_its_url() {
        let (base_a, server_a) = start_listing_server(two_model_listing()).await;
        let (base_b, server_b) = start_listing_server(two_model_listing()).await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.alpha]
            base_url = "{base_a}/v1"

            [model_providers.beta]
            base_url = "{base_b}/v1"

            [model.big-on-alpha]
            model = "big-one"
            base_url = "{base_a}/v1"
            "#
        ));

        let discovered = resolve_discovered_models(&cfg, &discover_provider_models(&cfg).await);
        server_a.abort();
        server_b.abort();

        assert!(!discovered.contains_key("alpha/big-one"));
        assert_eq!(
            discovered["big-on-alpha"].info.model_provider.as_deref(),
            Some("alpha")
        );
        let beta = discovered
            .get("beta/big-one")
            .expect("the other provider serving the same slug keeps its own entry");
        assert_eq!(beta.info.base_url, format!("{base_b}/v1"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_user_block_is_merged_with_the_listing_and_its_fields_win() {
        let (base, server) = start_ollama_server().await;
        let cfg = config_from(&format!(
            r#"
            [model_providers.ollama]
            base_url = "{base}/v1"

            [model.coder]
            model = "qwen3-coder:30b"
            model_provider = "ollama"
            name = "My Coder"
            temperature = 0.2

            [model.llama]
            model = "llama3.2:latest"
            model_provider = "ollama"
            context_window = 4096
            supports_reasoning_effort = false
            "#
        ));

        let discovered = discover_provider_models(&cfg).await;
        server.abort();
        let resolved = resolve_discovered_models(&cfg, &discovered);

        let coder = resolved
            .get("coder")
            .expect("the block's key names the merged entry");
        assert_eq!(
            coder.info.name.as_deref(),
            Some("My Coder"),
            "the block's name wins"
        );
        assert_eq!(
            coder.info.temperature,
            Some(0.2),
            "a field only the block sets is kept"
        );
        assert_eq!(
            coder.info.context_window.get(),
            32768,
            "the block set no window, so the runtime's loaded window fills it"
        );
        assert!(
            coder.info.supports_reasoning_effort,
            "the listing's `thinking` capability fills a field the block left unset"
        );
        assert_eq!(
            coder.info.loaded_in_vram,
            Some(true),
            "residency still reaches the entry"
        );
        assert!(!coder.info.pricing_lookup_enabled);

        let llama = resolved.get("llama").expect("merged entry");
        assert_eq!(
            llama.info.context_window.get(),
            4096,
            "the block's window beats the runtime's 131072"
        );
        assert!(
            !llama.info.supports_reasoning_effort,
            "the block's explicit false beats the listing's true"
        );

        assert!(!resolved.contains_key("ollama/qwen3-coder:30b"));
        assert!(!resolved.contains_key("ollama/llama3.2:latest"));
    }

    /// The merge reads the config it is given, so a reload that edits the
    /// block changes the entry without a second listing request.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reloaded_block_is_merged_without_asking_the_provider_again() {
        let (base, server) = start_listing_server(two_model_listing()).await;
        let before = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"
            "#
        ));
        let discovered = discover_provider_models(&before).await;
        server.abort();

        let after = config_from(&format!(
            r#"
            [model_providers.gateway]
            base_url = "{base}/v1"

            [model.big]
            model = "big-one"
            model_provider = "gateway"
            name = "Big"
            "#
        ));
        let resolved = resolve_discovered_models(&after, &discovered);

        let big = resolved
            .get("big")
            .expect("the new block claims the listed model");
        assert_eq!(big.info.name.as_deref(), Some("Big"));
        assert_eq!(
            big.info.context_window.get(),
            1_000_000,
            "the listing still supplies the window the block did not write"
        );
        assert!(!resolved.contains_key("gateway/big-one"));
    }

    #[test]
    fn laid_over_keeps_every_set_field_and_fills_the_rest() {
        let block = ConfigModelOverride {
            name: Some("mine".into()),
            context_window: Some(1000),
            extra_headers: [("X-Mine".to_owned(), "a".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let listed = ConfigModelOverride {
            model: Some("slug".into()),
            name: Some("listed".into()),
            context_window: Some(2000),
            supports_reasoning_effort: Some(true),
            extra_headers: [
                ("x-mine".to_owned(), "b".to_owned()),
                ("X-Other".to_owned(), "c".to_owned()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let merged = block.laid_over(&listed);

        assert_eq!(merged.name.as_deref(), Some("mine"));
        assert_eq!(merged.context_window, Some(1000));
        assert_eq!(merged.model.as_deref(), Some("slug"));
        assert_eq!(merged.supports_reasoning_effort, Some(true));
        assert_eq!(
            merged.extra_headers.get("X-Mine").map(String::as_str),
            Some("a")
        );
        assert!(
            !merged.extra_headers.contains_key("x-mine"),
            "a header name is one header whatever its case, and the block's wins"
        );
        assert_eq!(
            merged.extra_headers.get("X-Other").map(String::as_str),
            Some("c")
        );
    }

    #[test]
    fn a_provider_with_no_endpoint_is_skipped() {
        let cfg = config_from(
            r#"
            [model_providers.nameless]
            context_window = 200000
            "#,
        );
        let provider = cfg.model_providers.get("nameless").expect("provider");
        assert!(provider.autodetect_enabled());
        assert_eq!(provider.resolve_models_list_url(), None);
    }

    #[test]
    fn a_base_that_already_names_the_listing_is_not_doubled() {
        let cfg = config_from(
            r#"
            [model_providers.gateway]
            base_url = "https://gateway.example/v1/models"
            "#,
        );
        assert_eq!(
            cfg.model_providers["gateway"].resolve_models_list_url(),
            Some("https://gateway.example/v1/models".to_owned())
        );
    }
}
