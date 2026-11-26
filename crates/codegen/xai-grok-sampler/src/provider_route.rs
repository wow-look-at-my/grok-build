//! The proxy chain between this client and a provider, applied where a client is built.

use std::sync::{LazyLock, RwLock};

use indexmap::IndexMap;

use crate::config::SamplerConfig;

/// Where a provider's requests go while its chain is in this state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteTarget {
    /// The URL the client sends to.
    pub base_url: String,
    /// Put before the model id on the wire.
    pub model_prefix: String,
    pub extra_headers: IndexMap<String, String>,
    pub env_http_headers: IndexMap<String, String>,
    /// The credential of the hop the request stops at. `None` keeps the provider's.
    pub api_key: Option<String>,
}

/// One provider's chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRoute {
    pub provider_id: String,
    /// The provider's own base URL. A client built for it takes this route.
    pub provider_base_url: String,
    /// `Err` is why the chain cannot carry a request. Such a client is refused.
    pub target: Result<RouteTarget, String>,
}

static ROUTES: LazyLock<RwLock<Vec<ProviderRoute>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// Replace the table. The shell calls this at each config load.
pub fn set_provider_routes(routes: Vec<ProviderRoute>) {
    let mut table = ROUTES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *table != routes {
        tracing::info!(
            routes = ?routes.iter().map(describe).collect::<Vec<_>>(),
            "provider routes set"
        );
        *table = routes;
    }
}

fn describe(route: &ProviderRoute) -> String {
    match &route.target {
        Ok(target) => format!("{} -> {}", route.provider_id, target.base_url),
        Err(error) => format!("{} -> refused ({error})", route.provider_id),
    }
}

/// A URL without case or trailing slashes, for comparison only.
fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// The route of the provider at `base_url`.
pub fn route_for(base_url: &str) -> Option<ProviderRoute> {
    let wanted = normalize(base_url);
    if wanted.is_empty() {
        return None;
    }
    ROUTES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .find(|route| normalize(&route.provider_base_url) == wanted)
        .cloned()
}

/// Point `config` at its provider's first hop. The answer is the model prefix
/// the chain wants on the wire. `Err` is why a broken chain refuses the client.
pub(crate) fn apply(config: &mut SamplerConfig) -> Result<String, String> {
    let Some(route) = route_for(&config.base_url) else {
        return Ok(String::new());
    };
    let target = route.target.map_err(|error| {
        format!(
            "`models.providers.{}.hops` cannot carry a request: {error}",
            route.provider_id
        )
    })?;
    config.base_url = target.base_url;
    for (name, value) in target.extra_headers {
        if !has_header(&config.extra_headers, &name) {
            config.extra_headers.insert(name, value);
        }
    }
    for (name, variable) in target.env_http_headers {
        if !has_header(&config.env_http_headers, &name) {
            config.env_http_headers.insert(name, variable);
        }
    }
    if target.api_key.is_some() {
        config.api_key = target.api_key;
    }
    Ok(target.model_prefix)
}

fn has_header(headers: &IndexMap<String, String>, name: &str) -> bool {
    headers.keys().any(|own| own.eq_ignore_ascii_case(name))
}

/// The id to send for `model` under `prefix`.
pub(crate) fn wire_model(prefix: &str, model: String) -> String {
    if prefix.is_empty() {
        return model;
    }
    format!("{prefix}{model}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_model_puts_the_gateway_prefix_first() {
        assert_eq!(wire_model("", "a/b".to_owned()), "a/b");
        assert_eq!(
            wire_model("openrouter/", "a/b".to_owned()),
            "openrouter/a/b"
        );
        assert_eq!(
            wire_model("openrouter/", "openrouter/auto".to_owned()),
            "openrouter/openrouter/auto"
        );
    }
}
