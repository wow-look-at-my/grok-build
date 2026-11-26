//! The chain of proxies between this client and a provider.

use indexmap::IndexMap;

use super::config::EnvKeys;
use super::model_providers::{ModelProviderConfig, inherit_headers};

/// How a hop passes a request on.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HopKind {
    /// Answers the API itself and picks its own upstream. Its `base_url` is the whole address.
    #[default]
    Gateway,
    /// Forwards a path to the upstream that `routes` names, and changes nothing else.
    Relay,
}

impl HopKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Relay => "relay",
        }
    }
}

/// One proxy in `models.providers.<id>.hops`.
#[derive(Clone, Debug, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct HopConfig {
    /// Names this hop in the `routes` of the hop before it and in the UI.
    pub id: String,
    /// The name the UI shows. Unset shows the id.
    pub name: Option<String>,
    pub kind: HopKind,
    /// The user may turn an optional hop off in the UI.
    pub optional: bool,
    /// Unset means on. Only an optional hop reads it.
    pub enabled: Option<bool>,
    /// Where this hop listens.
    pub base_url: Option<String>,
    /// A relay's path to each link after it, by that link's id. The provider's id names the provider.
    pub routes: IndexMap<String, String>,
    /// A gateway's prefix on the model id, for example `openrouter/`.
    pub model_prefix: Option<String>,
    /// A credential this hop takes in place of the provider's.
    pub env_key: Option<EnvKeys>,
    pub api_key: Option<String>,
    /// Headers sent while this hop is on.
    pub extra_headers: IndexMap<String, String>,
    /// Header name to environment variable, sent while this hop is on.
    pub env_http_headers: IndexMap<String, String>,
}

impl HopConfig {
    /// A hop that is not optional is always on.
    pub fn is_on(&self) -> bool {
        !self.optional || self.enabled.unwrap_or(true)
    }

    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.id)
    }

    fn has_credential(&self) -> bool {
        self.api_key
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty())
            || self.env_key.as_ref().and_then(EnvKeys::primary).is_some()
    }
}

/// What the chain changes on a request to the provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// The URL the client sends to.
    pub base_url: String,
    /// Put before the model id on the wire.
    pub model_prefix: String,
    pub extra_headers: IndexMap<String, String>,
    pub env_http_headers: IndexMap<String, String>,
    /// The id of the hop whose credential replaces the provider's.
    pub credential_hop: Option<String>,
}

/// Why a chain cannot carry a request. The provider's models get no URL then.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("`hops[{index}]` has no id")]
    MissingId { index: usize },
    #[error("two hops share the id `{id}`")]
    DuplicateId { id: String },
    #[error("hop `{id}` uses the provider's own id")]
    ProviderId { id: String },
    #[error("hop `{id}` has no base_url")]
    MissingUrl { id: String },
    #[error("relay `{id}` has no `routes.{next}` entry, so it cannot reach `{next}`")]
    MissingRoute { id: String, next: String },
    #[error("relay `{id}` cannot carry `{url}`: {reason}")]
    BadUrl {
        id: String,
        url: String,
        reason: String,
    },
    #[error("the provider has no base_url for the last hop to reach")]
    MissingProviderUrl,
}

/// One link of a chain, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteLink {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub optional: bool,
    pub enabled: bool,
    /// The link's own address, for a latency probe.
    pub url: String,
}

/// A provider's chain, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteView {
    pub provider_id: String,
    /// Client side first. The provider is the last link.
    pub links: Vec<RouteLink>,
    /// The URL a request enters at, or blank when the chain is broken.
    pub entry_url: String,
    pub error: Option<String>,
}

fn trimmed(url: Option<&str>) -> Option<&str> {
    url.map(str::trim).filter(|url| !url.is_empty())
}

fn provider_url(provider: &ModelProviderConfig) -> Option<&str> {
    trimmed(provider.base_url.as_deref()).or_else(|| trimmed(provider.api_base_url.as_deref()))
}

/// The base URL a relay offers for `downstream`, the base URL of the link after it.
fn through_relay(hop: &HopConfig, next_id: &str, downstream: &str) -> Result<String, RouteError> {
    let route = hop
        .routes
        .get(next_id)
        .map(|route| route.trim())
        .filter(|route| !route.is_empty())
        .ok_or_else(|| RouteError::MissingRoute {
            id: hop.id.clone(),
            next: next_id.to_owned(),
        })?;
    if route.starts_with("http://") || route.starts_with("https://") {
        return Ok(route.trim_end_matches('/').to_owned());
    }
    let own = trimmed(hop.base_url.as_deref())
        .ok_or_else(|| RouteError::MissingUrl { id: hop.id.clone() })?;
    let parsed = xai_grok_net::Url::parse(downstream).map_err(|error| RouteError::BadUrl {
        id: hop.id.clone(),
        url: downstream.to_owned(),
        reason: error.to_string(),
    })?;
    let mut url = format!("{}/{}", own.trim_end_matches('/'), route.trim_matches('/'));
    let path = parsed.path().trim_end_matches('/');
    if !path.is_empty() {
        url.push_str(path);
    }
    if let Some(query) = parsed.query() {
        url.push('?');
        url.push_str(query);
    }
    Ok(url)
}

/// Check the ids before any hop is read by id.
fn check_ids(provider_id: &str, hops: &[HopConfig]) -> Result<(), RouteError> {
    for (index, hop) in hops.iter().enumerate() {
        if hop.id.trim().is_empty() {
            return Err(RouteError::MissingId { index });
        }
        if hop.id == provider_id {
            return Err(RouteError::ProviderId { id: hop.id.clone() });
        }
        let earlier_hops = hops.get(..index).unwrap_or_default();
        if earlier_hops.iter().any(|earlier| earlier.id == hop.id) {
            return Err(RouteError::DuplicateId { id: hop.id.clone() });
        }
    }
    Ok(())
}

/// Resolve the chain of `provider`. `Ok(None)` means it declares no hop.
pub fn resolve_route(
    provider_id: &str,
    provider: &ModelProviderConfig,
) -> Result<Option<ResolvedRoute>, RouteError> {
    if provider.hops.is_empty() {
        return Ok(None);
    }
    check_ids(provider_id, &provider.hops)?;
    let mut route = ResolvedRoute {
        base_url: provider_url(provider)
            .ok_or(RouteError::MissingProviderUrl)?
            .trim_end_matches('/')
            .to_owned(),
        ..Default::default()
    };
    let mut next_id = provider_id;
    // From the provider back to the client: each hop wraps what is after it.
    for hop in provider.hops.iter().rev().filter(|hop| hop.is_on()) {
        match hop.kind {
            HopKind::Relay => {
                route.base_url = through_relay(hop, next_id, &route.base_url)?;
            }
            HopKind::Gateway => {
                route.base_url = trimmed(hop.base_url.as_deref())
                    .ok_or_else(|| RouteError::MissingUrl { id: hop.id.clone() })?
                    .trim_end_matches('/')
                    .to_owned();
                if let Some(prefix) = hop.model_prefix.as_deref() {
                    route.model_prefix.insert_str(0, prefix);
                }
            }
        }
        // The hop nearer the client wins a header that hops both set.
        let mut headers = hop.extra_headers.clone();
        inherit_headers(&mut headers, &route.extra_headers);
        route.extra_headers = headers;
        let mut env_headers = hop.env_http_headers.clone();
        inherit_headers(&mut env_headers, &route.env_http_headers);
        route.env_http_headers = env_headers;
        if hop.has_credential() {
            route.credential_hop = Some(hop.id.clone());
        }
        next_id = &hop.id;
    }
    Ok(Some(route))
}

/// The chain of `provider` for the UI. A provider with no hop has none.
pub fn route_view(provider_id: &str, provider: &ModelProviderConfig) -> Option<RouteView> {
    if provider.hops.is_empty() {
        return None;
    }
    let mut links: Vec<RouteLink> = provider
        .hops
        .iter()
        .map(|hop| RouteLink {
            id: hop.id.clone(),
            name: hop.display_name().to_owned(),
            kind: hop.kind.as_str().to_owned(),
            optional: hop.optional,
            enabled: hop.is_on(),
            url: trimmed(hop.base_url.as_deref())
                .unwrap_or_default()
                .to_owned(),
        })
        .collect();
    links.push(RouteLink {
        id: provider_id.to_owned(),
        name: provider_id.to_owned(),
        kind: "provider".to_owned(),
        optional: false,
        enabled: true,
        url: provider_url(provider).unwrap_or_default().to_owned(),
    });
    let (entry_url, error) = match resolve_route(provider_id, provider) {
        Ok(Some(route)) => (route.base_url, None),
        Ok(None) => (String::new(), None),
        Err(error) => (String::new(), Some(error.to_string())),
    };
    Some(RouteView {
        provider_id: provider_id.to_owned(),
        links,
        entry_url,
        error,
    })
}

/// The credential of the hop a request stops at. An `env_key` that names no
/// set variable is an error: the provider's key must never go to that hop.
fn hop_credential(hop: &HopConfig) -> Result<String, String> {
    if let Some(key) = hop
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return Ok(key.to_owned());
    }
    hop.env_key
        .as_ref()
        .and_then(EnvKeys::resolve_value)
        .ok_or_else(|| {
            format!(
                "hop `{}` names an env_key that is not set in the environment",
                hop.id
            )
        })
}

/// What the sampler does for `provider`: a target, or the reason it refuses.
fn sampler_route(
    provider_id: &str,
    provider: &ModelProviderConfig,
) -> Option<xai_grok_sampler::provider_route::ProviderRoute> {
    let provider_base_url = provider_url(provider)?.to_owned();
    let target = match resolve_route(provider_id, provider) {
        Ok(None) => return None,
        Ok(Some(route)) => {
            let credential = route
                .credential_hop
                .as_deref()
                .and_then(|id| provider.hops.iter().find(|hop| hop.id == id))
                .map(hop_credential)
                .transpose();
            credential.map(|api_key| xai_grok_sampler::provider_route::RouteTarget {
                base_url: route.base_url,
                model_prefix: route.model_prefix,
                extra_headers: route.extra_headers,
                env_http_headers: route.env_http_headers,
                api_key,
            })
        }
        Err(error) => Err(error.to_string()),
    };
    Some(xai_grok_sampler::provider_route::ProviderRoute {
        provider_id: provider_id.to_owned(),
        provider_base_url,
        target,
        hop_names: provider
            .hops
            .iter()
            .filter(|hop| hop.is_on())
            .map(|hop| hop.display_name().to_owned())
            .collect(),
    })
}

/// Hand every provider's chain to the sampler. The answer is one reason per
/// chain that cannot carry a request, by provider id.
pub(crate) fn publish_provider_routes(
    providers: &IndexMap<String, ModelProviderConfig>,
) -> Vec<(String, String)> {
    let mut broken = Vec::new();
    let mut routes = Vec::new();
    for (id, provider) in providers {
        if provider.hops.is_empty() {
            continue;
        }
        match sampler_route(id, provider) {
            Some(route) => {
                if let Err(reason) = &route.target {
                    broken.push((id.clone(), reason.clone()));
                }
                routes.push(route);
            }
            None => broken.push((id.clone(), RouteError::MissingProviderUrl.to_string())),
        }
    }
    xai_grok_sampler::provider_route::set_provider_routes(routes);
    broken
}

/// Publish the chains of a raw config tree. The config watcher calls this when
/// `models.providers` changes, so a hop toggle reaches the next request.
pub(crate) fn publish_from_raw(raw: &toml::Value) {
    let (providers, _) = super::model_providers::parse_model_providers(raw);
    for (provider_id, reason) in publish_provider_routes(&providers) {
        tracing::error!(
            model_provider = %provider_id,
            %reason,
            "models.providers hops cannot carry a request; requests to this provider are refused"
        );
    }
}

/// Every provider chain in the effective config on disk.
pub fn load_route_views() -> Result<Vec<RouteView>, String> {
    let raw = crate::config::load_effective_config().map_err(|error| error.to_string())?;
    let (providers, _) = super::model_providers::parse_model_providers(&raw);
    Ok(providers
        .iter()
        .filter_map(|(id, provider)| route_view(id, provider))
        .collect())
}

/// How long one probe waits for an answer.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// The first round pays for the connection. The second measures the path.
const PROBE_ROUNDS: usize = 2;

/// The round-trip time to `url` in milliseconds: the fastest of the rounds.
pub async fn probe_link(url: &str) -> Result<u64, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("no URL".to_owned());
    }
    xai_grok_extra_ca::endpoint_allowlist::check(url).map_err(|refusal| refusal.to_string())?;
    let client = crate::http::shared_client();
    let mut best: Option<u64> = None;
    let mut last_error = None;
    for _ in 0..PROBE_ROUNDS {
        let started = std::time::Instant::now();
        match client
            .get(url, xai_grok_net::Auth::None)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
        {
            Ok(_response) => {
                let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                best = Some(best.map_or(elapsed, |fastest| fastest.min(elapsed)));
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    best.ok_or_else(|| last_error.unwrap_or_else(|| "no answer".to_owned()))
}

#[cfg(test)]
#[path = "model_route_tests.rs"]
mod tests;
