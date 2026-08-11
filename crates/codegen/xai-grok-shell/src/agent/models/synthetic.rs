//! Synthetic-provider `/v1/models` listing normalization.
//!
//! Synthetic's OpenAI-compatible listing uses non-standard key names for three
//! fields that the generic cross-provider [`crate::remote::parse_remote_model_value`]
//! already resolves from the usual OpenAI/Anthropic keys:
//!
//! * `context_length` → the model's input context window (e.g. `524288` for
//!   `syn:large:text`), where other providers use `contextWindow` /
//!   `context_window` / `max_input_tokens`;
//! * `max_output_length` → the max completion-token budget (e.g. `65536`),
//!   where other providers use `maxCompletionTokens` / `max_completion_tokens`;
//! * `reasoning_parameters.efforts` → the reasoning-effort menu (an array of
//!   canonical strings like `["none","high","max"]`), where other providers use
//!   `reasoningEfforts` / `reasoning_efforts`.
//!
//! These are deliberately NOT folded into the generic parser (so its fallback
//! chains stay provider-agnostic). Instead this module scopes the Synthetic
//! keys to Synthetic-shaped entries: it starts from the generic parse and then
//! applies only the Synthetic-specific fields that are present. All functions
//! here are pure and side-effect free so the mapping is unit-testable with one
//! entry as input.

use crate::agent::config::ModelEntryConfig;
use crate::remote::client::parse_remote_model_value;

fn get_u64(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(|v| v.as_u64())
}

/// True when a `/v1/models` entry is shaped by Synthetic's schema.
///
/// An entry is considered Synthetic when it carries any of the Synthetic-only
/// markers:
/// * a `provider` of `synthetic`,
/// * a `reasoning_parameters` object with an `efforts` array,
/// * a `context_length` field,
/// * a `syn:`-prefixed routing slug (e.g. `syn:large:text`).
///
/// This is intentionally narrow (none of these appear on standard xAI/OpenAI
/// or Anthropic-style listings), so a Synthetic entry is routed through
/// [`parse_synthetic_model_entry`] while ordinary entries are left to the
/// generic parser.
pub(crate) fn is_synthetic_listing(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let provider_is_synthetic = obj
        .get("provider")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("synthetic"));
    let has_reasoning_parameters_efforts = obj
        .get("reasoning_parameters")
        .and_then(|rp| rp.as_object())
        .and_then(|rp| rp.get("efforts"))
        .is_some();
    // Synthetic model routing slugs are `syn:`-prefixed, e.g. `syn:large:text`.
    let has_synthetic_routing_slug = obj
        .get("id")
        .or_else(|| obj.get("model"))
        .or_else(|| obj.get("modelId"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.starts_with("syn:"));
    provider_is_synthetic
        || has_reasoning_parameters_efforts
        || obj.get("context_length").is_some()
        || has_synthetic_routing_slug
}

/// Parse a Synthetic-shaped `/v1/models` entry into a [`ModelEntryConfig`].
///
/// Starts from the generic cross-provider parse (provider-agnostic keys only)
/// and then applies the Synthetic-specific fields — `context_length` as the
/// context window, `max_output_length` as the max completion-token budget, and
/// `reasoning_parameters.efforts` as the reasoning-effort menu — when present.
pub(crate) fn parse_synthetic_model_entry(
    value: &serde_json::Value,
    default_base_url: &str,
) -> Option<ModelEntryConfig> {
    let parsed = parse_remote_model_value(value, default_base_url)?;
    Some(apply_synthetic_schema(value, parsed))
}

/// Apply Synthetic's non-standard `/v1/models` keys onto a parsed entry.
///
/// Pure and idempotent: only the fields Synthetic actually provides are
/// overridden, so passing a non-Synthetic entry (or one missing these keys)
/// is a no-op that leaves the generic parse intact.
pub(crate) fn apply_synthetic_schema(
    value: &serde_json::Value,
    mut parsed: ModelEntryConfig,
) -> ModelEntryConfig {
    let Some(obj) = value.as_object() else {
        return parsed;
    };

    // `context_length` is the input context window; a non-zero value wins.
    let context_length = get_u64(obj, "context_length")
        .or_else(|| {
            obj.get("_meta")
                .and_then(|m| m.as_object())
                .and_then(|m| get_u64(m, "context_length"))
        })
        .filter(|&v| v > 0);
    if let Some(cw) = context_length
        && let Some(nz) = std::num::NonZeroU64::new(cw)
    {
        parsed.context_window = nz;
    }

    // `max_output_length` is the max completion-token budget.
    if let Some(co) = get_u64(obj, "max_output_length").and_then(|v| u32::try_from(v).ok()) {
        parsed.max_completion_tokens = Some(co);
    }

    // `reasoning_parameters.efforts` is the reasoning-effort menu.
    if let Some(arr) = obj
        .get("reasoning_parameters")
        .and_then(|rp| rp.as_object())
        .and_then(|rp| rp.get("efforts"))
        .and_then(|v| v.as_array())
    {
        parsed.reasoning_efforts = xai_grok_sampling_types::parse_reasoning_effort_options(arr);
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sampling_types::ReasoningEffort;

    #[test]
    fn synthetic_listing_extended_schema_resolves_real_fields() {
        // Synthetic's `/v1/models` entry shape (as served live). The window is
        // `context_length`, max output is `max_output_length`, the effort menu
        // lives under `reasoning_parameters.efforts`, and modalities are
        // `input_modalities`/`output_modalities` arrays. Driving the real
        // shipped path (generic parse + Synthetic schema) must adopt all of
        // these rather than the DEFAULT_CONTEXT_WINDOW fallback.
        let value = serde_json::json!({
            "provider": "synthetic",
            "always_on": true,
            "id": "syn:large:text",
            "hugging_face_id": "zai-org/GLM-5.2",
            "name": "syn:large:text",
            "reasoning_parameters": { "efforts": ["none", "high", "max"] },
            "description": "A very strong coding and writing model",
            "input_modalities": ["text"],
            "output_modalities": ["text"],
            "context_length": 524288,
            "max_output_length": 65536
        });
        assert!(
            is_synthetic_listing(&value),
            "a Synthetic entry must be recognized as Synthetic-shaped"
        );
        let result =
            parse_synthetic_model_entry(&value, "https://api.synthetic.new/openai/v1").unwrap();
        assert_eq!(result.model, "syn:large:text");
        assert_eq!(result.id.as_deref(), Some("syn:large:text"));
        assert_eq!(result.context_window.get(), 524_288);
        assert_eq!(result.max_completion_tokens, Some(65_536));

        let values: Vec<ReasoningEffort> =
            result.reasoning_efforts.iter().map(|o| o.value).collect();
        assert_eq!(
            values,
            vec![ReasoningEffort::None, ReasoningEffort::High, ReasoningEffort::Max]
        );
    }

    #[test]
    fn synthetic_listing_missing_extended_fields_falls_back_to_generic() {
        // A Synthetic entry without the Synthetic-only keys must still parse
        // (not be dropped) and keep the generic parser's DEFAULT_CONTEXT_WINDOW
        // fallback — the schema application is a no-op for missing fields.
        let value = serde_json::json!({
            "id": "syn:minimal",
            "name": "syn:minimal",
            "input_modalities": ["text"],
            "output_modalities": ["text"]
        });
        assert!(
            is_synthetic_listing(&value),
            "the synthetic id/marker must still route through the Synthetic home"
        );
        let result =
            parse_synthetic_model_entry(&value, "https://api.synthetic.new/openai/v1").unwrap();
        assert_eq!(result.model, "syn:minimal");
        assert_eq!(
            result.context_window.get(),
            crate::remote::DEFAULT_CONTEXT_WINDOW
        );
        assert_eq!(result.max_completion_tokens, None);
        assert!(result.reasoning_efforts.is_empty());
    }

    #[test]
    fn synthetic_schema_is_a_noop_on_a_generic_style_entry() {
        // Driving the Synthetic schema over an ordinary OpenAI-style entry leaves
        // every field that the generic parser already resolved unchanged.
        let value = serde_json::json!({
            "id": "grok-4",
            "model": "grok-4",
            "name": "grok-4",
            "contextWindow": 131072,
            "maxCompletionTokens": 32768,
            "reasoningEfforts": ["none", "high"]
        });
        assert!(
            !is_synthetic_listing(&value),
            "an OpenAI-style entry must not be mis-detected as Synthetic"
        );
        let parsed = parse_remote_model_value(&value, "https://api.x.ai/v1").unwrap();
        let applied = apply_synthetic_schema(&value, parsed.clone());
        assert_eq!(applied.context_window, parsed.context_window);
        assert_eq!(applied.max_completion_tokens, parsed.max_completion_tokens);
        assert_eq!(applied.reasoning_efforts, parsed.reasoning_efforts);
    }
}