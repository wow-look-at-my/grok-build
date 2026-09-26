//! Model-id resolution: catalog keys, routing slugs, and selection.

use globset::{Glob, GlobSet, GlobSetBuilder};
use indexmap::IndexMap;

use crate::agent::config::{self, ModelEntry};
use agent_client_protocol as acp;
use xai_grok_sampling_types::ReasoningEffort;

/// Resolve a model's context window from a `/v1/models` listing, per exact
/// requested slug.
///
/// The listing is the parsed provider response (each entry carrying its own
/// `contextWindow` / `context_window` value read via
/// `parse_remote_model_value`). An entry is matched by its
/// catalog key or its routing `model` slug, so a multi-model listing yields
/// each model's own window — never a max or first-match value. A requested
/// slug absent from the listing (or a provider whose listing exposes no
/// per-model window) resolves to the documented `DEFAULT_CONTEXT_WINDOW`
/// fallback rather than an error, so a cold catalog never aborts the build.
pub(crate) fn resolve_context_window(
    requested: &str,
    listing: &IndexMap<String, ModelEntry>,
) -> std::num::NonZeroU64 {
    listing
        .get(requested)
        .or_else(|| listing.values().find(|entry| entry.info.model == requested))
        .map(|entry| entry.info.context_window)
        .unwrap_or_else(|| {
            std::num::NonZeroU64::new(crate::remote::DEFAULT_CONTEXT_WINDOW)
                .expect("DEFAULT_CONTEXT_WINDOW is non-zero")
        })
}

/// Resolve a model's context window directly from its **own** provider base
/// (`/v1/models` at `api_base_url`, OpenAI-compatible), authenticated with the
/// model's own API key.
///
/// This closes the BYOK/custom-base gap that [`resolve_context_window`]
/// cannot: the generic `/v1/models` prefetch is driven by `EndpointsConfig`
/// and only ever queries the configured xAI proxy (or a single global
/// `[endpoints].models_base_url`). A model that ships its own
/// `api_base_url` + API key on the catalog entry (e.g.
/// `openrouter/deepseek/...` served by `https://gateway.pazer.ai/v1`) is never
/// in that listing, so its window stays at a hardcoded default. Here we ask
/// the model's own provider for the real value.
///
/// The listing is fetched **on a dedicated OS thread** (`reqwest::blocking`
/// constructs an inner tokio runtime, which panics if the caller happens to be
/// running inside an async tokio context — e.g. `resolve_model_list` reached
/// from `SessionActor::model_auth_state`). Offloading the network I/O to a
/// `std::thread` keeps this call safe from both sync and async callers.
///
/// Returns `None` when the listing can't be fetched, doesn't carry the slug,
/// or the listed window is itself a default sentinel — so a cold/unreachable
/// provider never aborts the catalog build.
pub(crate) fn resolve_context_window_from_provider(
    model: &str,
    api_base_url: &str,
    api_key: Option<&str>,
) -> Option<std::num::NonZeroU64> {
    use crate::remote::DEFAULT_CONTEXT_WINDOW;
    let default = std::num::NonZeroU64::new(DEFAULT_CONTEXT_WINDOW).expect("non-zero");
    let listing = provider_listing(api_base_url, api_key);
    let listed = listing
        .iter()
        .find(|entry| entry.model == model || entry.id.as_deref() == Some(&model))?;
    let cw = listed.context_window;
    (cw != default).then_some(cw)
}

const LISTING_REUSE: std::time::Duration = std::time::Duration::from_secs(60);

/// The provider's `/v1/models` listing, fetched a single time per [`LISTING_REUSE`].
fn provider_listing(
    api_base_url: &str,
    api_key: Option<&str>,
) -> Vec<crate::agent::config::ModelEntryConfig> {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex, PoisonError};
    use std::time::Instant;

    type Listing = Vec<crate::agent::config::ModelEntryConfig>;
    type Slot = Arc<Mutex<Option<(Instant, Listing)>>>;
    static SLOTS: LazyLock<Mutex<HashMap<(String, Option<String>), Slot>>> =
        LazyLock::new(Default::default);

    let slot = SLOTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((api_base_url.to_owned(), api_key.map(str::to_owned)))
        .or_default()
        .clone();
    let mut cached = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((fetched_at, listing)) = cached.as_ref()
        && fetched_at.elapsed() < LISTING_REUSE
    {
        return listing.clone();
    }
    let listing = fetch_provider_listing(api_base_url, api_key);
    *cached = Some((Instant::now(), listing.clone()));
    listing
}

/// Fetch a provider listing on its own OS thread. `reqwest::blocking` builds a
/// runtime that panics inside an async context. The deadline stops a provider
/// that never answers from stalling the catalog build.
fn fetch_provider_listing(
    api_base_url: &str,
    api_key: Option<&str>,
) -> Vec<crate::agent::config::ModelEntryConfig> {
    const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
    let (base, key) = (api_base_url.to_owned(), api_key.map(str::to_owned));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::remote::fetch_models_for_api_base_blocking(
            &base,
            key.as_deref(),
        ));
    });
    match rx.recv_timeout(FETCH_TIMEOUT) {
        Ok(Ok(listing)) => listing,
        Ok(Err(error)) => {
            tracing::warn!(%error, api_base_url, "provider model listing failed");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(
                api_base_url,
                timeout_secs = FETCH_TIMEOUT.as_secs(),
                "provider model listing gave no answer"
            );
            Vec::new()
        }
    }
}

/// Map a model id (catalog key or routing slug) to its catalog key.
pub(crate) fn resolve_catalog_key(
    models: &IndexMap<String, ModelEntry>,
    id: &acp::ModelId,
) -> Option<acp::ModelId> {
    let id_str = id.0.as_ref();
    if models.contains_key(id_str) {
        return Some(id.clone());
    }
    models
        .iter()
        .rev()
        .find(|(_, entry)| entry.info.has_model_id(id_str))
        .map(|(key, _)| acp::ModelId::new(key.clone()))
}

/// Catalog key for a persisted session model id, restricted to **selectable** entries.
pub(crate) fn selectable_catalog_key_for_persisted(
    models: &IndexMap<String, ModelEntry>,
    available: &IndexMap<acp::ModelId, acp::ModelInfo>,
    id: &acp::ModelId,
) -> Option<acp::ModelId> {
    if available.contains_key(id) {
        return Some(id.clone());
    }
    let id_str = id.0.as_ref();
    if let Some((key, _)) = models.iter().rev().find(|(key, entry)| {
        available.contains_key(&acp::ModelId::new((*key).clone()))
            && entry.info.has_model_id(id_str)
    }) {
        return Some(acp::ModelId::new(key.clone()));
    }
    resolve_catalog_key(models, id).filter(|key| available.contains_key(key))
}

/// A "campaign-only" preferred flip: the default changed and either side's value is an active campaign default.
pub(crate) fn is_campaign_only_flip(
    old_preferred: &Option<String>,
    new_preferred: &Option<String>,
    campaign_defaults: &std::collections::HashSet<String>,
) -> bool {
    if new_preferred == old_preferred || new_preferred.is_none() {
        return false;
    }
    new_preferred
        .as_ref()
        .is_some_and(|p| campaign_defaults.contains(p))
        || old_preferred
            .as_ref()
            .is_some_and(|p| campaign_defaults.contains(p))
}

/// Pick the default model: CLI > env > config > remote-settings hint, falling back to the first visible model, then the bundled default.
pub(crate) fn resolve_default_model(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> (String, ModelEntry, config::ConfigSource) {
    let visible: IndexMap<String, ModelEntry> = catalog
        .iter()
        .filter(|(_, e)| e.info.visible_for_auth(is_session_auth) && e.info.user_selectable)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let model_pref = config::resolve_string_flag(
        cfg.default_model_override.as_deref(),
        "GROK_DEFAULT_MODEL",
        cfg.models.default.as_deref(),
        cfg.remote_settings
            .as_ref()
            .and_then(|rs| rs.default_model.as_deref()),
    );

    let first_or_fallback = || -> (String, ModelEntry) {
        if let Some((key, first)) = visible.first() {
            return (key.clone(), first.clone());
        }
        if let Some((key, entry)) = catalog.iter().find(|(_, e)| e.info.user_selectable) {
            tracing::warn!("no auth-visible selectable model; using first selectable entry");
            return (key.clone(), entry.clone());
        }
        tracing::warn!("no selectable models; falling back to bundled default (pre-catalog)");
        let default_id = crate::models::default_model().to_string();
        let mut entry = ModelEntry::fallback(&default_id, &cfg.endpoints);
        entry.info.user_selectable = model_is_allowlisted(cfg, &default_id, &default_id);
        (default_id, entry)
    };

    match &model_pref {
        None => {
            let (key, first) = first_or_fallback();
            (key, first, config::ConfigSource::Default)
        }
        Some(pref) => {
            let found = visible
                .get_key_value(&pref.value)
                .or_else(|| visible.iter().find(|(_, m)| m.has_model_id(&pref.value)));

            if let Some((key, entry)) = found {
                (key.clone(), entry.clone(), pref.source)
            } else {
                let is_explicit = matches!(
                    pref.source,
                    config::ConfigSource::Cli
                        | config::ConfigSource::Env
                        | config::ConfigSource::Config
                );
                if is_explicit {
                    tracing::warn!(
                        model_id = %pref.value, source = %pref.source,
                        "preferred model not in available models, falling back"
                    );
                } else {
                    tracing::debug!(
                        model_id = %pref.value, source = %pref.source,
                        "remote default_model not in available models, skipping"
                    );
                }
                let campaign_pref_missing = cfg.models.default_is_campaign_driven
                    && matches!(pref.source, config::ConfigSource::Config);
                if campaign_pref_missing
                    && let Some(prev) = cfg
                        .models
                        .pre_campaign_default
                        .as_deref()
                        .filter(|s| !s.is_empty())
                    && let Some((key, entry)) = visible
                        .get_key_value(prev)
                        .or_else(|| visible.iter().find(|(_, m)| m.has_model_id(prev)))
                {
                    tracing::info!(
                        unavailable = %pref.value, fallback = %prev,
                        "campaign-driven default unavailable in catalog; recovering the pre-campaign default"
                    );
                    return (key.clone(), entry.clone(), config::ConfigSource::Config);
                }
                let (key, first) = first_or_fallback();
                (key, first, config::ConfigSource::Default)
            }
        }
    }
}

/// Keep the picker projection of `catalog` (`ModelInfo::is_picker_eligible`) in ACP wire format.
pub(crate) fn available_models(
    catalog: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> IndexMap<acp::ModelId, acp::ModelInfo> {
    let visible: IndexMap<String, ModelEntry> = catalog
        .iter()
        .filter(|(_, e)| e.info.is_picker_eligible(is_session_auth))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    config::to_acp_model_info(&visible)
}

/// Compiled glob matcher shared by `allowed_models`, `disabled_models`, and `hidden_models` (matched against catalog key or model id).
pub(crate) struct ModelGlobSet(GlobSet);

impl ModelGlobSet {
    /// Compile a filter list (`Ok(None)` for `None`/empty). Fails **closed**: an invalid pattern returns `Err` listing every bad one.
    pub(crate) fn compile(patterns: Option<&[String]>) -> Result<Option<Self>, Vec<String>> {
        let patterns = match patterns {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(None),
        };
        let mut builder = GlobSetBuilder::new();
        let mut invalid = Vec::new();
        for pat in patterns {
            match Glob::new(pat) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(_) => invalid.push(pat.clone()),
            }
        }
        if !invalid.is_empty() {
            return Err(invalid);
        }
        builder
            .build()
            .map(|set| Some(Self(set)))
            .map_err(|e| vec![e.to_string()])
    }

    pub(crate) fn matches(&self, key: &str, model: &str) -> bool {
        self.0.is_match(key) || self.0.is_match(model)
    }

    fn matches_model(&self, model: &str) -> bool {
        self.0.is_match(model)
    }
}

/// Mark every entry that matches a favorites glob, and clear the mark on every
/// entry that does not.
///
/// Two lists feed this: `[models].favorite_models` covers the whole catalog, and
/// `[model_providers.<id>].favorite_models` covers the models of that provider.
/// A model is a favorite when either list matches it.
///
/// An invalid pattern fails OPEN — the mark is cosmetic, and dropping every
/// favorite would empty the picker's opening list. `allowed_models` fails closed
/// because it decides what may be used at all.
pub(crate) fn apply_favorites(cfg: &config::Config, catalog: &mut IndexMap<String, ModelEntry>) {
    let global = match ModelGlobSet::compile(cfg.models.favorite_models.as_deref()) {
        Ok(set) => set,
        Err(bad) => {
            tracing::error!(patterns = ?bad, "favorite_models: invalid glob(s); ignoring the list");
            None
        }
    };
    let per_provider: std::collections::HashMap<&str, ModelGlobSet> = cfg
        .model_providers
        .iter()
        .filter_map(|(id, provider)| {
            match ModelGlobSet::compile(Some(provider.favorite_models.as_slice())) {
                Ok(set) => set.map(|set| (id.as_str(), set)),
                Err(bad) => {
                    tracing::error!(
                        provider = %id, patterns = ?bad,
                        "favorite_models: invalid glob(s); ignoring this provider's list"
                    );
                    None
                }
            }
        })
        .collect();

    for (key, entry) in catalog.iter_mut() {
        let model = entry.info.model.clone();
        let by_global = global.as_ref().is_some_and(|set| set.matches(key, &model));
        let by_provider = entry
            .info
            .model_provider
            .as_deref()
            .and_then(|id| per_provider.get(id))
            .is_some_and(|set| set.matches(key, &model));
        entry.info.favorite = by_global || by_provider;
    }
}

/// A model that names a `model_provider` routes to that provider. A config
/// model starts from an entry that carries the cli-chat-proxy URL, and some
/// provider shapes leave it there. Replace it with the provider's other URL,
/// or with none.
fn keep_provider_models_off_the_proxy(
    cfg: &config::Config,
    catalog: &mut IndexMap<String, ModelEntry>,
) {
    let proxy = cfg.endpoints.resolve_inference_base_url();
    if proxy.trim().is_empty() {
        return;
    }
    let models_base = cfg
        .endpoints
        .models_base_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(str::to_owned);
    for (key, entry) in catalog.iter_mut() {
        if entry.info.model_provider.is_none() || entry.info.base_url != proxy {
            continue;
        }
        let replacement = entry
            .api_base_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .or_else(|| models_base.clone())
            .unwrap_or_default();
        tracing::warn!(
            model = %key,
            provider = ?entry.info.model_provider,
            base_url = %replacement,
            "provider model had the cli-chat-proxy URL; using the provider's own URL"
        );
        entry.info.base_url = replacement;
    }
}

/// A model with no URL cannot answer. The picker never offers it, and the
/// default never picks it.
fn unselect_models_without_a_url(catalog: &mut IndexMap<String, ModelEntry>) {
    let unreachable: Vec<&str> = catalog
        .iter_mut()
        .filter(|(_, entry)| !entry.has_endpoint())
        .map(|(key, entry)| {
            entry.info.user_selectable = false;
            key.as_str()
        })
        .collect();
    if !unreachable.is_empty() {
        tracing::debug!(
            models = ?unreachable,
            "these models have no URL and cannot be selected; set their base_url, or the [endpoints] URL they use"
        );
    }
}

/// Resolved allowlist: fleet pin, user/project list, or unrestricted.
enum EffectiveAllowlist<'a> {
    Unrestricted,
    Invalid,
    User(&'a [String]),
    Fleet(&'a [String]),
}

fn effective_allowlist(cfg: &config::Config) -> EffectiveAllowlist<'_> {
    use crate::agent::config::AllowlistPin;
    match cfg.requirements.allowed_models.pin_ref() {
        Some(AllowlistPin::FailClosed) => EffectiveAllowlist::Invalid,
        Some(AllowlistPin::List(patterns)) if patterns.is_empty() => {
            EffectiveAllowlist::Unrestricted
        }
        Some(AllowlistPin::List(patterns)) => EffectiveAllowlist::Fleet(patterns),
        None => match cfg.models.allowed_models.as_deref() {
            Some(patterns) if !patterns.is_empty() => EffectiveAllowlist::User(patterns),
            _ => EffectiveAllowlist::Unrestricted,
        },
    }
}

impl EffectiveAllowlist<'_> {
    fn is_unrestricted(&self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    fn is_fleet(&self) -> bool {
        matches!(self, Self::Fleet(_) | Self::Invalid)
    }

    fn is_selected(&self, key: &str, model: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Invalid => false,
            Self::Fleet(patterns) | Self::User(patterns) => {
                match ModelGlobSet::compile(Some(patterns)) {
                    Ok(None) => true,
                    Ok(Some(set)) => {
                        if matches!(self, Self::Fleet(_)) {
                            set.matches_model(model)
                        } else {
                            set.matches(key, model)
                        }
                    }
                    Err(_) => false,
                }
            }
        }
    }

    fn apply_selectability(&self, catalog: &mut IndexMap<String, ModelEntry>) {
        match self {
            Self::Unrestricted => {
                for entry in catalog.values_mut() {
                    entry.info.user_selectable = true;
                }
            }
            Self::Invalid => {
                for entry in catalog.values_mut() {
                    entry.info.user_selectable = false;
                }
            }
            Self::Fleet(patterns) | Self::User(patterns) => {
                match ModelGlobSet::compile(Some(patterns)) {
                    Ok(None) => {
                        for entry in catalog.values_mut() {
                            entry.info.user_selectable = true;
                        }
                    }
                    Ok(Some(set)) => {
                        let fleet = matches!(self, Self::Fleet(_));
                        for (key, entry) in catalog.iter_mut() {
                            entry.info.user_selectable = if fleet {
                                set.matches_model(&entry.model)
                            } else {
                                set.matches(key, &entry.model)
                            };
                        }
                    }
                    Err(bad) => {
                        tracing::error!(
                            patterns = ?bad,
                            "allowed_models: invalid glob(s); marking nothing selectable"
                        );
                        for entry in catalog.values_mut() {
                            entry.info.user_selectable = false;
                        }
                    }
                }
            }
        }
    }
}

/// Catalog-key match is user-config only. A fleet pin matches the routing
/// slug so a user `[model.grok-4-anything]` cannot satisfy `grok-4*`.
fn model_is_allowlisted(cfg: &config::Config, key: &str, model: &str) -> bool {
    effective_allowlist(cfg).is_selected(key, model)
}

pub(crate) fn allowlist_denied_message(cfg: &config::Config) -> &'static str {
    if effective_allowlist(cfg).is_fleet() {
        "This model isn't allowed by your organization's policy. Contact your administrator."
    } else {
        "This model isn't allowed by your allowed_models setting."
    }
}

pub(crate) fn allowlist_excludes_all_message(cfg: &config::Config) -> String {
    match effective_allowlist(cfg) {
        EffectiveAllowlist::Invalid => {
            "The organization model policy is invalid. Contact your administrator.".to_owned()
        }
        EffectiveAllowlist::Fleet(_) => {
            "None of your models are allowed by your organization's policy. Contact your administrator."
                .to_owned()
        }
        _ => "None of your models are allowed by allowed_models. \
             Broaden it or remove it from your config, then restart."
            .to_owned(),
    }
}

/// Single source of truth for the catalog: applies `disabled_models`, then `allowed_models`, then `hidden_models`.
pub(crate) fn resolve_model_catalog(
    cfg: &config::Config,
    prefetched: Option<IndexMap<String, ModelEntry>>,
) -> IndexMap<String, ModelEntry> {
    let mut catalog: IndexMap<String, ModelEntry> = config::resolve_model_list(cfg, prefetched);
    keep_provider_models_off_the_proxy(cfg, &mut catalog);

    if let Ok(Some(disabled)) = ModelGlobSet::compile(cfg.models.disabled_models.as_deref()) {
        let before = catalog.len();
        catalog.retain(|key, entry| !disabled.matches(key, &entry.model));
        let removed = before - catalog.len();
        if removed > 0 {
            tracing::info!(count = removed, "disabled_models: removed from catalog");
        }
    }

    effective_allowlist(cfg).apply_selectability(&mut catalog);
    unselect_models_without_a_url(&mut catalog);

    if let Ok(Some(hidden)) = ModelGlobSet::compile(cfg.models.hidden_models.as_deref()) {
        for (key, entry) in catalog.iter_mut() {
            if hidden.matches(key, &entry.model) {
                entry.info.hidden = true;
            }
        }
    }

    force_reasoning_effort_support(cfg, &mut catalog);

    if let Some(effort) = cfg.models.default_reasoning_effort
        && let Some(default_id) = cfg.models.default.as_deref()
        && let Some(entry) = catalog.get_mut(default_id)
        && entry.info.supports_reasoning_effort
    {
        stamp_effort(&mut entry.info, effort);
    }

    if let Some(effort) = cfg.reasoning_effort_override {
        for entry in catalog.values_mut() {
            if model_offers_reasoning_effort(&entry.info, effort) {
                stamp_effort(&mut entry.info, effort);
            }
        }
    }

    apply_favorites(cfg, &mut catalog);
    catalog
}

/// Force the effort gate on for every model `[models].force_reasoning_effort_models`
/// matches. This runs on the FINISHED catalog, so it is the one knob that does not
/// need a `[model.<key>]` table name to equal the catalog key — which is what makes
/// it usable against a server catalog that omits `supports_reasoning_effort`.
///
/// A forced model with no menu of its own falls back to the built-in low..xhigh
/// menu, the same one any flagged model with no server list gets.
pub(crate) fn force_reasoning_effort_support(
    cfg: &config::Config,
    catalog: &mut IndexMap<String, ModelEntry>,
) {
    let Ok(Some(forced)) =
        ModelGlobSet::compile(cfg.models.force_reasoning_effort_models.as_deref())
    else {
        return;
    };
    for (key, entry) in catalog.iter_mut() {
        if !forced.matches(key, &entry.model) || entry.info.supports_reasoning_effort {
            continue;
        }
        tracing::info!(
            model_key = %key,
            model = %entry.info.model,
            "force_reasoning_effort_models: forcing supports_reasoning_effort on",
        );
        entry.info.supports_reasoning_effort = true;
    }
}

/// Add a provider-qualified catalog (Codex, or an autodetected
/// `[model_providers.<id>]`) to the resolved xAI/custom catalog.
///
/// Provider entries are deliberately kept outside `prefetched`: xAI auth
/// refreshes and cache reloads may replace that catalog wholesale, while an
/// independent Codex sign-in or a provider's own listing must remain available.
/// User model filters still apply uniformly to every provider.
pub(crate) fn merge_additive_catalog(
    cfg: &config::Config,
    mut catalog: IndexMap<String, ModelEntry>,
    provider_models: &IndexMap<String, ModelEntry>,
) -> IndexMap<String, ModelEntry> {
    let mut additive = provider_models.clone();
    keep_provider_models_off_the_proxy(cfg, &mut additive);

    if let Ok(Some(disabled)) = ModelGlobSet::compile(cfg.models.disabled_models.as_deref()) {
        additive.retain(|key, entry| !disabled.matches(key, &entry.model));
    }

    effective_allowlist(cfg).apply_selectability(&mut additive);
    unselect_models_without_a_url(&mut additive);

    if let Ok(Some(hidden)) = ModelGlobSet::compile(cfg.models.hidden_models.as_deref()) {
        for (key, entry) in additive.iter_mut() {
            if hidden.matches(key, &entry.model) {
                entry.info.hidden = true;
            }
        }
    }

    force_reasoning_effort_support(cfg, &mut additive);

    if let Some(effort) = cfg.models.default_reasoning_effort
        && let Some(default_id) = cfg.models.default.as_deref()
        && let Some(entry) = additive.get_mut(default_id)
        && entry.info.supports_reasoning_effort
    {
        entry.info.reasoning_effort = Some(effort);
    }

    if let Some(effort) = cfg.reasoning_effort_override {
        for entry in additive.values_mut() {
            if model_offers_reasoning_effort(&entry.info, effort) {
                entry.info.reasoning_effort = Some(effort);
            }
        }
    }

    catalog.extend(additive);
    apply_favorites(cfg, &mut catalog);
    catalog
}

/// The entry keeps its own model id, and `model_at` picks the id for this effort when a request is prepared.
fn stamp_effort(info: &mut config::ModelInfo, effort: ReasoningEffort) {
    info.reasoning_effort = Some(effort);
}

/// Whether `effort` is a value this model will accept on the wire.
pub(crate) fn model_offers_reasoning_effort(
    info: &config::ModelInfo,
    effort: ReasoningEffort,
) -> bool {
    if !info.supports_reasoning_effort {
        return false;
    }
    if info.reasoning_efforts.is_empty() {
        matches!(
            effort,
            ReasoningEffort::Low
                | ReasoningEffort::Medium
                | ReasoningEffort::High
                | ReasoningEffort::Xhigh
        )
    } else {
        info.reasoning_efforts.iter().any(|opt| opt.value == effort)
    }
}

/// True when an active `allowed_models` allowlist leaves no selectable model.
pub(crate) fn allowlist_matches_nothing(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
) -> bool {
    !effective_allowlist(cfg).is_unrestricted() && !catalog.values().any(|e| e.info.user_selectable)
}

/// Reject an `allowed_models` allowlist that leaves no selectable model, or excludes an explicitly configured default.
/// Run only against a real catalog.
pub(crate) fn validate_selectable(
    cfg: &config::Config,
    catalog: &IndexMap<String, ModelEntry>,
) -> Result<(), String> {
    let allowlist = effective_allowlist(cfg);
    match allowlist {
        EffectiveAllowlist::Unrestricted => return Ok(()),
        EffectiveAllowlist::Invalid => {
            return Err(
                "The organization model policy is invalid. Contact your administrator.".to_owned(),
            );
        }
        EffectiveAllowlist::Fleet(_) | EffectiveAllowlist::User(_) => {}
    }
    if !catalog.values().any(|e| e.info.user_selectable) {
        return Err(allowlist_excludes_all_message(cfg));
    }
    for (src, id) in [
        ("default", cfg.models.default.as_deref()),
        ("-m flag", cfg.default_model_override.as_deref()),
    ] {
        if let Some(id) = id
            && let Some(entry) = catalog
                .get(id)
                .or_else(|| catalog.values().find(|e| e.has_model_id(id)))
            && !entry.info.user_selectable
        {
            return Err(if allowlist.is_fleet() {
                format!(
                    "\"{id}\" (your {src}) isn't allowed by your organization's policy. \
                     Contact your administrator."
                )
            } else {
                format!(
                    "\"{id}\" (your {src}) isn't allowed by allowed_models. \
                     Broaden the patterns or remove allowed_models, then try again."
                )
            });
        }
    }
    Ok(())
}
