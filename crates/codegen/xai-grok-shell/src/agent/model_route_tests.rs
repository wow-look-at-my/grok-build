// model_route_tests.rs holds chain.

use super::*;
use crate::agent::config::Config;

/// The chain this feature was written for: a zstd relay on a NAS, then
/// Bifrost, then OpenRouter. `overrides` is TOML appended to the block.
fn chain(nas_on: bool, bifrost_on: bool) -> ModelProviderConfig {
    let raw: toml::Value = toml::from_str(&format!(
        r#"
        [model_providers.openrouter]
        base_url = "https://openrouter.ai/api/v1"
        api_key = "sk-or"

        [[model_providers.openrouter.hops]]
        id = "nas"
        name = "NAS"
        kind = "relay"
        optional = true
        enabled = {nas_on}
        base_url = "http://nas.lan:8317"
        routes = {{ bifrost = "bifrost", openrouter = "openrouter" }}
        extra_headers = {{ "X-AI-Proxy-Token" = "relay-secret" }}

        [[model_providers.openrouter.hops]]
        id = "bifrost"
        kind = "gateway"
        optional = true
        enabled = {bifrost_on}
        base_url = "http://bifrost.lan:8080/v1"
        model_prefix = "openrouter/"
        api_key = "sk-bf"
        extra_headers = {{ "x-bf-send-back-raw-response" = "true" }}
        "#
    ))
    .expect("the chain is valid TOML");
    let cfg = Config::new_from_toml_cfg(&raw).expect("the config parses");
    cfg.model_providers["openrouter"].clone()
}

fn resolved(nas_on: bool, bifrost_on: bool) -> ResolvedRoute {
    resolve_route("openrouter", &chain(nas_on, bifrost_on))
        .expect("the chain resolves")
        .expect("the provider has hops")
}

#[test]
fn every_hop_on_enters_at_the_relay_and_reaches_the_gateway_through_it() {
    let route = resolved(true, true);
    assert_eq!(route.base_url, "http://nas.lan:8317/bifrost/v1");
    assert_eq!(route.model_prefix, "openrouter/");
    assert_eq!(route.credential_hop.as_deref(), Some("bifrost"));
    assert_eq!(route.extra_headers["X-AI-Proxy-Token"], "relay-secret");
    assert_eq!(route.extra_headers["x-bf-send-back-raw-response"], "true");
}

#[test]
fn the_gateway_off_sends_the_relay_straight_to_the_provider() {
    let route = resolved(true, false);
    assert_eq!(route.base_url, "http://nas.lan:8317/openrouter/api/v1");
    assert_eq!(route.model_prefix, "", "no gateway renames the model");
    assert_eq!(
        route.credential_hop, None,
        "the provider's own key goes out"
    );
    assert!(
        !route
            .extra_headers
            .contains_key("x-bf-send-back-raw-response"),
        "a hop that is off sends none of its headers"
    );
}

#[test]
fn the_relay_off_enters_at_the_next_link() {
    let route = resolved(false, true);
    assert_eq!(route.base_url, "http://bifrost.lan:8080/v1");
    assert_eq!(route.model_prefix, "openrouter/");
    assert!(!route.extra_headers.contains_key("X-AI-Proxy-Token"));
}

#[test]
fn every_optional_hop_off_goes_straight_to_the_provider() {
    let route = resolved(false, false);
    assert_eq!(route.base_url, "https://openrouter.ai/api/v1");
    assert_eq!(route.model_prefix, "");
    assert_eq!(route.credential_hop, None);
    assert!(route.extra_headers.is_empty());
}

#[test]
fn a_hop_that_is_not_optional_stays_on_whatever_enabled_says() {
    let mut provider = chain(true, true);
    provider.hops[1].optional = false;
    provider.hops[1].enabled = Some(false);
    let route = resolve_route("openrouter", &provider).unwrap().unwrap();
    assert_eq!(route.base_url, "http://nas.lan:8317/bifrost/v1");
}

#[test]
fn a_route_written_as_a_url_is_the_whole_address() {
    let mut provider = chain(true, false);
    provider.hops[0].routes.insert(
        "openrouter".to_owned(),
        "http://nas.lan:9000/or/v1/".to_owned(),
    );
    let route = resolve_route("openrouter", &provider).unwrap().unwrap();
    assert_eq!(route.base_url, "http://nas.lan:9000/or/v1");
}

#[test]
fn a_relay_keeps_the_query_of_the_url_after_it() {
    let mut provider = chain(true, false);
    provider.base_url = Some("https://openrouter.ai/api/v1?tenant=a".to_owned());
    let route = resolve_route("openrouter", &provider).unwrap().unwrap();
    assert_eq!(
        route.base_url,
        "http://nas.lan:8317/openrouter/api/v1?tenant=a"
    );
}

#[test]
fn a_relay_with_no_route_to_the_next_link_is_an_error() {
    let mut provider = chain(true, false);
    provider.hops[0].routes.shift_remove("openrouter");
    assert_eq!(
        resolve_route("openrouter", &provider),
        Err(RouteError::MissingRoute {
            id: "nas".to_owned(),
            next: "openrouter".to_owned(),
        })
    );
}

#[test]
fn bad_ids_are_errors() {
    let mut provider = chain(true, true);
    provider.hops[1].id = "nas".to_owned();
    assert_eq!(
        resolve_route("openrouter", &provider),
        Err(RouteError::DuplicateId {
            id: "nas".to_owned()
        })
    );
    provider.hops[1].id = "openrouter".to_owned();
    assert_eq!(
        resolve_route("openrouter", &provider),
        Err(RouteError::ProviderId {
            id: "openrouter".to_owned()
        })
    );
    provider.hops[1].id = " ".to_owned();
    assert_eq!(
        resolve_route("openrouter", &provider),
        Err(RouteError::MissingId { index: 1 })
    );
}

#[test]
fn a_provider_with_no_hop_has_no_route() {
    let mut provider = chain(true, true);
    provider.hops.clear();
    assert_eq!(resolve_route("openrouter", &provider), Ok(None));
    assert_eq!(route_view("openrouter", &provider), None);
}

#[test]
fn the_view_lists_every_link_and_ends_at_the_provider() {
    let view = route_view("openrouter", &chain(true, false)).expect("the provider has hops");
    let rows: Vec<(&str, &str, &str, bool, bool)> = view
        .links
        .iter()
        .map(|link| {
            (
                link.id.as_str(),
                link.name.as_str(),
                link.kind.as_str(),
                link.optional,
                link.enabled,
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("nas", "NAS", "relay", true, true),
            ("bifrost", "bifrost", "gateway", true, false),
            ("openrouter", "openrouter", "provider", false, true),
        ]
    );
    assert_eq!(view.entry_url, "http://nas.lan:8317/openrouter/api/v1");
    assert_eq!(view.error, None);
}

#[test]
fn a_broken_chain_shows_its_reason_in_the_view() {
    let mut provider = chain(true, true);
    provider.hops[0].routes.clear();
    let view = route_view("openrouter", &provider).unwrap();
    assert_eq!(view.entry_url, "");
    assert!(
        view.error.as_deref().unwrap().contains("routes.bifrost"),
        "{:?}",
        view.error
    );
}

/// The sampler is what sends, so the table it holds is the behavior.
#[test]
fn a_config_load_hands_the_chain_to_the_sampler() {
    let provider = chain(true, true);
    let route = xai_grok_sampler::provider_route::route_for("https://openrouter.ai/api/v1/")
        .expect("the load published the provider's route");
    let target = route.target.expect("the chain resolves");
    assert_eq!(route.provider_id, "openrouter");
    assert_eq!(target.base_url, "http://nas.lan:8317/bifrost/v1");
    assert_eq!(target.model_prefix, "openrouter/");
    assert_eq!(target.api_key.as_deref(), Some("sk-bf"));
    assert_eq!(provider.hops.len(), 2);
}

#[test]
fn a_hop_env_key_that_is_not_set_refuses_the_route() {
    let mut provider = chain(true, true);
    provider.hops[1].api_key = None;
    provider.hops[1].env_key = Some(EnvKeys::single("GROK_TEST_HOP_KEY_THAT_IS_NOT_SET"));
    let mut providers = IndexMap::new();
    providers.insert("openrouter".to_owned(), provider);
    let broken = publish_provider_routes(&providers);
    assert_eq!(broken.len(), 1);
    assert!(broken[0].1.contains("env_key"), "{broken:?}");
    let route =
        xai_grok_sampler::provider_route::route_for("https://openrouter.ai/api/v1").unwrap();
    assert!(route.target.is_err(), "the sampler refuses the provider");
}
