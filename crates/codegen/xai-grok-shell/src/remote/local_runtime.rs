//! Model listings for a LOCAL runtime: Ollama and LM Studio.
//!
//! Both also serve an OpenAI-compatible `/v1/models`, and both answer it with
//! an id and nothing else — no window, no capabilities, no residency. A model
//! discovered that way lands on [`DEFAULT_CONTEXT_WINDOW`], which for Ollama
//! is 256k against a runner the server loaded at whatever its VRAM allowed
//! (`OLLAMA_CONTEXT_LENGTH` documents the default as "4k/32k/256k based on
//! VRAM"). The harness then never compacts, and Ollama drops the head of the
//! conversation in silence. Reading each runtime's own listing is what makes
//! the catalog's number true.
//!
//! Residency is the other thing only these listings carry, and it is what the
//! picker's green dot reads.

use xai_grok_sampling_types::ollama::{OllamaPsResponse, OllamaShowResponse, OllamaTagsResponse};

use crate::agent::config::ModelEntryConfig;
use crate::agent::model_providers::{ContextWindowSource, ModelsListDialect};

use super::client::BackendError;

/// One model as a local runtime describes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LocalModel {
    /// The routing slug, which is what the inference endpoint takes.
    pub slug: String,
    pub display_name: Option<String>,
    /// The model's own maximum window.
    pub max_context: Option<u64>,
    /// The window the loaded instance is running at, when one is loaded.
    pub loaded_context: Option<u64>,
    /// Whether a runner for this model is resident right now.
    pub loaded_in_vram: bool,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    /// The reasoning levels the runtime says this model takes, where it says.
    pub reasoning_levels: Vec<String>,
    /// Quantization / architecture, for the picker's description line.
    pub description: Option<String>,
}

impl LocalModel {
    /// The window this entry contributes to the catalog.
    ///
    /// `Loaded` prefers the running instance's window because that is the one
    /// inference actually enforces. An unloaded model has none, so it falls
    /// back to the maximum rather than to the client's 256k guess.
    fn context_window(&self, source: ContextWindowSource) -> Option<u64> {
        match source {
            ContextWindowSource::Loaded => self.loaded_context.or(self.max_context),
            ContextWindowSource::Max => self.max_context.or(self.loaded_context),
        }
    }
}

/// The host root a native API lives under, given an inference base URL.
///
/// A provider's `base_url` points at the OpenAI-compatible endpoint
/// (`http://localhost:11434/v1`), and every native path is a sibling of it at
/// the host root. Stripping the known suffixes is what lets one provider block
/// serve both.
pub(crate) fn host_root(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    for suffix in ["/api/v1", "/api/v0", "/v1", "/api"] {
        if let Some(root) = trimmed.strip_suffix(suffix) {
            return root.trim_end_matches('/').to_owned();
        }
    }
    trimmed.to_owned()
}

/// Fetch one local runtime's listing and turn it into catalog entries.
///
/// `inference_base_url` is what the discovered models are reached at; the
/// listing itself comes from the host root.
pub(crate) fn fetch_local_listing_blocking(
    dialect: ModelsListDialect,
    base_url: &str,
    inference_base_url: &str,
    api_key: Option<&str>,
    window_source: ContextWindowSource,
) -> Result<Vec<ModelEntryConfig>, BackendError> {
    let host = host_root(base_url);
    let models = match dialect {
        ModelsListDialect::Ollama => fetch_ollama_models(&host, api_key)?,
        ModelsListDialect::Lmstudio => fetch_lmstudio_models(&host, api_key)?,
        ModelsListDialect::Openai => {
            return super::client::fetch_models_for_api_base_blocking(base_url, api_key);
        }
    };
    Ok(models
        .into_iter()
        .map(|model| to_entry(model, inference_base_url, window_source))
        .collect())
}

/// Which of a local runtime's models are resident RIGHT NOW, by routing slug.
///
/// Residency changes without anything else changing: a model loads on its
/// first request, and LM Studio's idle TTL unloads it again. A dot painted
/// once at startup is therefore wrong within minutes, so this is the cheap
/// re-read behind it — one request for Ollama, one for LM Studio, and no
/// `/api/show` per model.
pub(crate) fn fetch_residency_blocking(
    dialect: ModelsListDialect,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<std::collections::HashMap<String, bool>, BackendError> {
    let host = host_root(base_url);
    let client = crate::http::shared_startup_blocking_client();
    let mut residency = std::collections::HashMap::new();
    match dialect {
        ModelsListDialect::Ollama => {
            let running: OllamaPsResponse = get_json(&client, &format!("{host}/api/ps"), api_key)?;
            for model in running.models {
                // A CPU-resident runner is not in VRAM, and the dot says VRAM.
                let in_vram = model.size_vram.unwrap_or(0) > 0;
                for name in [model.model, model.name] {
                    if !name.is_empty() {
                        residency.insert(name, in_vram);
                    }
                }
            }
        }
        ModelsListDialect::Lmstudio => {
            let models = fetch_lmstudio_models(&host, api_key)?;
            for model in models {
                residency.insert(model.slug, model.loaded_in_vram);
            }
        }
        // A remote provider reports no residency, so there is nothing to poll
        // and nothing draws a dot.
        ModelsListDialect::Openai => {}
    }
    Ok(residency)
}

fn to_entry(
    model: LocalModel,
    inference_base_url: &str,
    window_source: ContextWindowSource,
) -> ModelEntryConfig {
    let context_window = model
        .context_window(window_source)
        .and_then(std::num::NonZeroU64::new)
        .unwrap_or_else(|| {
            std::num::NonZeroU64::new(super::client::DEFAULT_CONTEXT_WINDOW)
                .expect("the default window is non-zero")
        });
    let reasoning_efforts = reasoning_efforts_from_levels(&model.reasoning_levels);
    let description = describe(&model);
    ModelEntryConfig {
        id: None,
        model: model.slug.clone(),
        base_url: inference_base_url.to_owned(),
        name: model.display_name.or(Some(model.slug)),
        description,
        context_window,
        // A local runtime that reports thinking support gets the gate opened;
        // `/effort` reads exactly this.
        supports_reasoning_effort: model.supports_thinking,
        reasoning_efforts,
        loaded_in_vram: Some(model.loaded_in_vram),
        ..ModelEntryConfig::minimal(inference_base_url)
    }
}

/// The picker's description line: what the runtime said about the model,
/// plus a warning when it says the model was never trained to call tools.
///
/// That warning is the one piece of capability here with no home on
/// `ModelInfo`, and it decides whether this agent can use the model at all —
/// a model that cannot call tools answers a coding request with prose.
fn describe(model: &LocalModel) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(description) = model.description.clone() {
        parts.push(description);
    }
    if !model.supports_tools {
        parts.push("not trained for tool use".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The runtime's own reasoning levels, as catalog options.
///
/// LM Studio names them (`off`/`on`/`low`/`medium`/`high`); Ollama reports
/// only that a model thinks, and the levels its API documents are the same
/// low/medium/high plus `max`. A level this client has no
/// [`ReasoningEffort`](xai_grok_sampling_types::ReasoningEffort) for is
/// skipped rather than guessed at.
fn reasoning_efforts_from_levels(
    levels: &[String],
) -> Vec<xai_grok_sampling_types::ReasoningEffortOption> {
    use xai_grok_sampling_types::{ReasoningEffort, ReasoningEffortOption};
    levels
        .iter()
        .filter_map(|level| {
            let value = match level.as_str() {
                "off" | "none" => ReasoningEffort::None,
                "low" => ReasoningEffort::Low,
                // LM Studio's binary models report `on` and nothing finer.
                "on" | "medium" => ReasoningEffort::Medium,
                "high" => ReasoningEffort::High,
                "max" => ReasoningEffort::Max,
                _ => return None,
            };
            Some(ReasoningEffortOption {
                id: level.clone(),
                value,
                label: level.clone(),
                description: None,
                default: false,
            })
        })
        .collect()
}

// ── Ollama ──────────────────────────────────────────────────────────────

/// `/api/tags` for what is on disk, `/api/ps` for what is resident, and one
/// `/api/show` per model for its window and capabilities.
///
/// The per-model call is what `/api/tags` cannot avoid: the tag listing
/// carries a size and a quantization and says nothing about the context
/// length or whether the model was trained for tools. These are localhost
/// metadata reads.
fn fetch_ollama_models(host: &str, api_key: Option<&str>) -> Result<Vec<LocalModel>, BackendError> {
    let client = crate::http::shared_startup_blocking_client();
    let tags: OllamaTagsResponse = get_json(&client, &format!("{host}/api/tags"), api_key)?;

    // A failed `/api/ps` costs the dot, not the listing: a model with no
    // residency answer is reported as not loaded rather than not listed.
    let running: OllamaPsResponse = get_json(&client, &format!("{host}/api/ps"), api_key)
        .unwrap_or_else(|error| {
            tracing::debug!(%error, "ollama /api/ps did not answer; nothing reads as resident");
            OllamaPsResponse::default()
        });

    let mut models = Vec::with_capacity(tags.models.len());
    for tag in tags.models {
        let slug = if tag.model.is_empty() {
            tag.name.clone()
        } else {
            tag.model.clone()
        };
        if slug.is_empty() {
            continue;
        }
        let resident = running
            .models
            .iter()
            .find(|r| r.model == slug || r.name == slug);
        let show: Option<OllamaShowResponse> = post_json(
            &client,
            &format!("{host}/api/show"),
            api_key,
            &serde_json::json!({ "model": slug }),
        )
        .map_err(|error| {
            tracing::debug!(model = %slug, %error, "ollama /api/show failed; no window for this one");
        })
        .ok();

        let details = tag.details.as_ref();
        let description = details.map(|d| {
            let params = d.parameter_size.as_deref().unwrap_or("");
            let quant = d.quantization_level.as_deref().unwrap_or("");
            [params, quant]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ")
        });

        models.push(LocalModel {
            slug: slug.clone(),
            display_name: Some(slug),
            max_context: show
                .as_ref()
                .and_then(OllamaShowResponse::max_context_length),
            loaded_context: resident.and_then(|r| r.context_length).filter(|&c| c > 0),
            // `/api/ps` lists CPU-resident runners too, and a dot that claims
            // VRAM for one is wrong. `size_vram` is what separates them.
            loaded_in_vram: resident.is_some_and(|r| r.size_vram.unwrap_or(0) > 0),
            supports_tools: show.as_ref().is_some_and(|s| s.has_capability("tools")),
            supports_thinking: show.as_ref().is_some_and(|s| s.has_capability("thinking")),
            reasoning_levels: show
                .as_ref()
                .filter(|s| s.has_capability("thinking"))
                .map(|_| {
                    ["low", "medium", "high", "max"]
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            description: description.filter(|d| !d.is_empty()),
        });
    }
    Ok(models)
}

// ── LM Studio ───────────────────────────────────────────────────────────

/// `/api/v1/models` (LM Studio 0.4.0+), falling back to `/api/v0/models`.
///
/// Both carry the window, the quantization and the load state in one answer,
/// so neither needs a per-model call. Only v1 reports capabilities and the
/// reasoning menu.
fn fetch_lmstudio_models(
    host: &str,
    api_key: Option<&str>,
) -> Result<Vec<LocalModel>, BackendError> {
    let client = crate::http::shared_startup_blocking_client();
    match get_json::<serde_json::Value>(&client, &format!("{host}/api/v1/models"), api_key) {
        Ok(body) => Ok(parse_lmstudio_v1(&body)),
        Err(error) => {
            tracing::debug!(
                %error,
                "LM Studio /api/v1/models did not answer; trying the v0 listing"
            );
            let body: serde_json::Value =
                get_json(&client, &format!("{host}/api/v0/models"), api_key)?;
            Ok(parse_lmstudio_v0(&body))
        }
    }
}

/// Parse the v1 listing. Embedding models are left out: the picker lists what
/// a session can chat with.
pub(crate) fn parse_lmstudio_v1(body: &serde_json::Value) -> Vec<LocalModel> {
    let Some(models) = body.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    models
        .iter()
        .filter(|m| m.get("type").and_then(|t| t.as_str()) != Some("embedding"))
        .filter_map(|m| {
            let slug = m.get("key")?.as_str()?.to_owned();
            let capabilities = m.get("capabilities");
            let loaded = m
                .get("loaded_instances")
                .and_then(|l| l.as_array())
                .filter(|l| !l.is_empty());
            Some(LocalModel {
                display_name: m
                    .get("display_name")
                    .and_then(|n| n.as_str())
                    .map(str::to_owned),
                max_context: m.get("max_context_length").and_then(|c| c.as_u64()),
                loaded_context: loaded
                    .and_then(|l| l.first())
                    .and_then(|i| i.pointer("/config/context_length"))
                    .and_then(|c| c.as_u64()),
                loaded_in_vram: loaded.is_some(),
                supports_tools: capabilities
                    .and_then(|c| c.get("trained_for_tool_use"))
                    .and_then(|t| t.as_bool())
                    .unwrap_or(false),
                supports_thinking: capabilities.and_then(|c| c.get("reasoning")).is_some(),
                reasoning_levels: capabilities
                    .and_then(|c| c.pointer("/reasoning/allowed_options"))
                    .and_then(|o| o.as_array())
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(|o| o.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                description: lmstudio_description(
                    m.get("architecture").and_then(|a| a.as_str()),
                    m.pointer("/quantization/name").and_then(|q| q.as_str()),
                    m.get("params_string").and_then(|p| p.as_str()),
                ),
                slug,
            })
        })
        .collect()
}

/// Parse the v0 listing, which is OpenAI-shaped (`{data: [...]}`) with extra
/// fields. It reports `state` rather than the loaded instance's own window,
/// so a loaded model there contributes its maximum.
pub(crate) fn parse_lmstudio_v0(body: &serde_json::Value) -> Vec<LocalModel> {
    let Some(models) = body.get("data").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    models
        .iter()
        .filter(|m| m.get("type").and_then(|t| t.as_str()) != Some("embeddings"))
        .filter_map(|m| {
            let slug = m.get("id")?.as_str()?.to_owned();
            Some(LocalModel {
                display_name: Some(slug.clone()),
                max_context: m.get("max_context_length").and_then(|c| c.as_u64()),
                loaded_context: None,
                loaded_in_vram: m.get("state").and_then(|s| s.as_str()) == Some("loaded"),
                // v0 reports no capabilities at all. Claiming either way is a
                // guess; the config's own keys are the place to state it.
                supports_tools: false,
                supports_thinking: false,
                reasoning_levels: Vec::new(),
                description: lmstudio_description(
                    m.get("arch").and_then(|a| a.as_str()),
                    m.get("quantization").and_then(|q| q.as_str()),
                    None,
                ),
                slug,
            })
        })
        .collect()
}

fn lmstudio_description(
    arch: Option<&str>,
    quant: Option<&str>,
    params: Option<&str>,
) -> Option<String> {
    let parts: Vec<&str> = [arch, params, quant].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

// ── HTTP helpers ────────────────────────────────────────────────────────

fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
) -> Result<T, BackendError> {
    let mut request = client.get(url);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    read_json(request, url)
}

fn post_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
    body: &serde_json::Value,
) -> Result<T, BackendError> {
    let mut request = client.post(url).json(body);
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    read_json(request, url)
}

fn read_json<T: serde::de::DeserializeOwned>(
    request: reqwest::blocking::RequestBuilder,
    url: &str,
) -> Result<T, BackendError> {
    let response = request.send()?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        tracing::debug!(%url, status = status.as_u16(), "local runtime listing failed");
        return Err(BackendError::RequestFailed {
            status: status.as_u16(),
            body,
        });
    }
    Ok(response.json()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_root_is_found_under_every_endpoint_spelling() {
        for base in [
            "http://localhost:11434/v1",
            "http://localhost:11434/v1/",
            "http://localhost:11434/api",
            "http://localhost:11434",
        ] {
            assert_eq!(host_root(base), "http://localhost:11434", "base: {base}");
        }
        assert_eq!(
            host_root("http://localhost:1234/api/v1"),
            "http://localhost:1234"
        );
    }

    #[test]
    fn a_loaded_lmstudio_model_reports_the_window_it_is_running_at() {
        let body = serde_json::json!({
            "models": [{
                "type": "llm",
                "key": "google/gemma-4-26b",
                "display_name": "Gemma 4 26B",
                "architecture": "gemma4",
                "quantization": { "name": "Q4_K_M" },
                "params_string": "26B",
                "max_context_length": 262144,
                "loaded_instances": [{ "id": "google/gemma-4-26b",
                                       "config": { "context_length": 8192 } }],
                "capabilities": {
                    "vision": true,
                    "trained_for_tool_use": true,
                    "reasoning": { "allowed_options": ["off", "on"], "default": "on" }
                }
            }]
        });

        let models = parse_lmstudio_v1(&body);

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert!(model.loaded_in_vram);
        assert_eq!(model.loaded_context, Some(8192));
        assert_eq!(model.max_context, Some(262_144));
        assert_eq!(
            model.context_window(ContextWindowSource::Loaded),
            Some(8192),
            "compaction has to respect the window inference runs under"
        );
        assert_eq!(
            model.context_window(ContextWindowSource::Max),
            Some(262_144)
        );
        assert!(model.supports_tools);
        assert!(model.supports_thinking);
        assert_eq!(model.reasoning_levels, vec!["off", "on"]);
    }

    #[test]
    fn an_unloaded_model_falls_back_to_its_maximum_not_to_the_client_default() {
        let body = serde_json::json!({
            "models": [{
                "type": "llm",
                "key": "deepseek-r1",
                "max_context_length": 131072,
                "loaded_instances": [],
            }]
        });

        let models = parse_lmstudio_v1(&body);

        assert!(!models[0].loaded_in_vram);
        assert_eq!(
            models[0].context_window(ContextWindowSource::Loaded),
            Some(131_072)
        );
    }

    #[test]
    fn embedding_models_are_not_offered_as_chat_models() {
        let body = serde_json::json!({
            "models": [
                { "type": "embedding", "key": "nomic-embed", "max_context_length": 2048 },
                { "type": "llm", "key": "real-one", "max_context_length": 4096 },
            ]
        });

        let models = parse_lmstudio_v1(&body);

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "real-one");
    }

    #[test]
    fn the_v0_listing_still_reports_a_window_and_a_load_state() {
        let body = serde_json::json!({
            "object": "list",
            "data": [{
                "id": "meta-llama-3.1-8b-instruct",
                "type": "llm",
                "arch": "llama",
                "quantization": "Q4_K_M",
                "state": "loaded",
                "max_context_length": 131072,
            }]
        });

        let models = parse_lmstudio_v0(&body);

        assert_eq!(models.len(), 1);
        assert!(models[0].loaded_in_vram);
        assert_eq!(models[0].max_context, Some(131_072));
        assert_eq!(models[0].description.as_deref(), Some("llama · Q4_K_M"),);
    }

    #[test]
    fn a_reasoning_menu_keeps_only_the_levels_this_client_can_send() {
        let options = reasoning_efforts_from_levels(&[
            "off".to_owned(),
            "on".to_owned(),
            "ludicrous".to_owned(),
        ]);

        let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["off", "on"],
            "a level with no ReasoningEffort behind it is skipped, never guessed at"
        );
    }
}
