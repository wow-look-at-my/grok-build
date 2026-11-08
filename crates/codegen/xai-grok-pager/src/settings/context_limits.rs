//! The "Context window limit" row: a chooser of catalog models.

use std::collections::BTreeMap;

use agent_client_protocol as acp;

use super::registry::{OwnedEnumChoice, PagerLocalSnapshot, model_choice_label};

/// The setting key of the row.
pub const CONTEXT_WINDOW_LIMIT_KEY: &str = "model_context_window";

/// The smallest limit the editor accepts. A smaller number is almost always a missing `k`.
pub const MIN_CONTEXT_LIMIT: u64 = 1_000;

/// The largest limit the editor accepts.
pub const MAX_CONTEXT_LIMIT: u64 = 100_000_000;

/// Words that remove the limit, in addition to an empty input and `0`.
const CLEAR_WORDS: &[&str] = &["none", "default", "off", "clear", "unset", "reset"];

/// The model that the open editor sets the limit for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLimitTarget {
    pub model_id: String,
    pub label: String,
    /// The window the catalog reports now. With a limit set, this is the limit.
    pub window: Option<u64>,
    /// The limit `config.toml` holds for this model.
    pub limit: Option<u64>,
}

/// Parse an editor input. `Ok(None)` removes the limit.
pub fn parse_token_amount(input: &str) -> Result<Option<u64>, String> {
    let text: String = input
        .trim()
        .chars()
        .filter(|c| !matches!(c, '_' | ',' | ' '))
        .collect::<String>()
        .to_ascii_lowercase();
    if text.is_empty() || text == "0" || CLEAR_WORDS.contains(&text.as_str()) {
        return Ok(None);
    }
    let (number, multiplier) = if let Some(rest) = text.strip_suffix('k') {
        (rest, 1_000.0)
    } else if let Some(rest) = text.strip_suffix('m') {
        (rest, 1_000_000.0)
    } else {
        (text.as_str(), 1.0)
    };
    let tokens = if multiplier == 1.0 {
        number
            .parse::<u64>()
            .map_err(|_| format!("Not a token count: \"{}\"", input.trim()))?
    } else {
        let value: f64 = number
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v > 0.0)
            .ok_or_else(|| format!("Not a token count: \"{}\"", input.trim()))?;
        (value * multiplier).round() as u64
    };
    if tokens < MIN_CONTEXT_LIMIT {
        return Err(format!(
            "Too small: {tokens} tokens. Did you mean {tokens}k?"
        ));
    }
    if tokens > MAX_CONTEXT_LIMIT {
        return Err(format!("Too large: {} tokens", short_tokens(tokens)));
    }
    Ok(Some(tokens))
}

/// A token count in short form: `1M`, `350k`, `1.5M`, `131.1k`.
pub fn short_tokens(tokens: u64) -> String {
    fn trimmed(value: f64, decimals: usize) -> String {
        let text = format!("{value:.decimals$}");
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text
        }
    }
    if tokens >= 1_000_000 {
        format!("{}M", trimmed(tokens as f64 / 1_000_000.0, 2))
    } else if tokens >= 1_000 {
        format!("{}k", trimmed(tokens as f64 / 1_000.0, 1))
    } else {
        tokens.to_string()
    }
}

/// The text the editor starts with for `limit`.
pub fn editor_text(limit: Option<u64>) -> String {
    limit.map(short_tokens).unwrap_or_default()
}

/// The window of each catalog model, read from the ACP `totalContextTokens` meta.
pub fn catalog_windows(
    available: &indexmap::IndexMap<acp::ModelId, acp::ModelInfo>,
) -> BTreeMap<String, u64> {
    available
        .iter()
        .filter_map(|(id, info)| {
            let tokens = info.meta.as_ref()?.get("totalContextTokens")?.as_u64()?;
            Some((id.0.to_string(), tokens))
        })
        .collect()
}

/// Where each catalog model routes, by model id: its provider, else its endpoint host.
pub fn catalog_routes(
    available: &indexmap::IndexMap<acp::ModelId, acp::ModelInfo>,
) -> BTreeMap<String, String> {
    available
        .iter()
        .filter_map(|(id, info)| {
            let meta = info.meta.as_ref();
            let route = xai_grok_sampling_types::provider_meta(meta)
                .or_else(|| xai_grok_sampling_types::endpoint_meta(meta))?;
            Some((id.0.to_string(), route.to_owned()))
        })
        .collect()
}

/// The labels of the chooser rows, by model id. Several providers can serve
/// one model under one name. Each such row gets its route, or its id when
/// the route does not tell the rows apart. So no rows read the same.
pub fn row_labels(snapshot: &PagerLocalSnapshot) -> BTreeMap<String, String> {
    let route_of = |id: &acp::ModelId| snapshot.model_routes.get(id.0.as_ref()).cloned();
    let mut by_name: BTreeMap<String, Vec<&acp::ModelId>> = BTreeMap::new();
    for (name, id) in &snapshot.available_models {
        by_name
            .entry(model_choice_label(name, id).to_lowercase())
            .or_default()
            .push(id);
    }
    snapshot
        .available_models
        .iter()
        .map(|(name, id)| {
            let label = model_choice_label(name, id);
            let others: Vec<&acp::ModelId> = by_name
                .get(&label.to_lowercase())
                .map(|ids| ids.iter().copied().filter(|other| *other != id).collect())
                .unwrap_or_default();
            if others.is_empty() {
                return (id.0.to_string(), label);
            }
            let route = route_of(id)
                .filter(|route| {
                    others
                        .iter()
                        .all(|other| route_of(other).as_ref() != Some(route))
                })
                .unwrap_or_else(|| id.0.to_string());
            (id.0.to_string(), format!("{label} ({route})"))
        })
        .collect()
}

/// The label of the chooser row of `model_id`, or the id when the catalog
/// does not hold it.
pub fn row_label(snapshot: &PagerLocalSnapshot, model_id: &str) -> String {
    row_labels(snapshot)
        .remove(model_id)
        .unwrap_or_else(|| model_id.to_owned())
}

/// The limits the user `config.toml` sets, by model key. A config that does not
/// load gives no limits, and the log says why.
pub fn configured_limits() -> BTreeMap<String, u64> {
    if cfg!(test) {
        return BTreeMap::new();
    }
    let root = match xai_grok_shell::config::load_effective_config() {
        Ok(root) => root,
        Err(error) => {
            tracing::error!(
                target: "settings",
                %error,
                "config.toml did not load; the context window limits are not shown"
            );
            return BTreeMap::new();
        }
    };
    limits_from_config(&root)
}

/// The `[model."<id>"] context_window` values of a config table.
pub fn limits_from_config(root: &toml::Value) -> BTreeMap<String, u64> {
    let Some(models) = root.get("model").and_then(toml::Value::as_table) else {
        return BTreeMap::new();
    };
    models
        .iter()
        .filter_map(|(key, block)| {
            let tokens = block.get("context_window")?.as_integer()?;
            let tokens = u64::try_from(tokens).ok().filter(|t| *t > 0)?;
            Some((key.clone(), tokens))
        })
        .collect()
}

/// One chooser row per catalog model. The current model comes first, and the
/// rest sort by label.
pub fn choices(snapshot: &PagerLocalSnapshot) -> Vec<OwnedEnumChoice> {
    let limits = &snapshot.context_limits;
    let mut labels = row_labels(snapshot);
    let mut rows: Vec<(bool, OwnedEnumChoice)> = snapshot
        .available_models
        .iter()
        .map(|(name, id)| {
            let label = labels
                .remove(id.0.as_ref())
                .unwrap_or_else(|| model_choice_label(name, id));
            let window = snapshot.model_windows.get(id.0.as_ref()).copied();
            let limit = limits.get(id.0.as_ref()).copied();
            let is_current = match snapshot.current_model_id.as_deref() {
                Some(current) => current == id.0.as_ref(),
                None => snapshot.current_model_name.as_deref() == Some(name.as_str()),
            };
            (
                is_current,
                OwnedEnumChoice {
                    canonical: id.0.to_string(),
                    display: choice_display(&label, window, limit),
                    description: String::new(),
                    search_extra: label,
                },
            )
        })
        .collect();
    rows.sort_by(|(a_current, a), (b_current, b)| {
        b_current
            .cmp(a_current)
            .then_with(|| {
                super::registry::natural_cmp_ignore_case(&a.search_extra, &b.search_extra)
            })
            .then_with(|| a.canonical.cmp(&b.canonical))
    });
    rows.dedup_by(|a, b| a.1.canonical == b.1.canonical);
    rows.into_iter().map(|(_, choice)| choice).collect()
}

/// The row value: `Big One 350k` for one limit, `2 models` for more, empty for none.
pub fn summary(snapshot: &PagerLocalSnapshot) -> String {
    let limits = &snapshot.context_limits;
    let Some((id, tokens)) = limits.iter().next() else {
        return String::new();
    };
    if limits.len() > 1 {
        return format!("{} models", limits.len());
    }
    format!("{} {}", row_label(snapshot, id), short_tokens(*tokens))
}

/// `Name · 1M`, or `Name · 350k limit` for a model with a configured limit.
pub fn choice_display(label: &str, window: Option<u64>, limit: Option<u64>) -> String {
    match (limit, window) {
        (Some(limit), _) => format!("{label} \u{00B7} {} limit", short_tokens(limit)),
        (None, Some(window)) => format!("{label} \u{00B7} {}", short_tokens(window)),
        (None, None) => label.to_owned(),
    }
}

/// The editor header text for `target`.
pub fn editor_description(target: &ContextLimitTarget) -> String {
    let now = match (target.limit, target.window) {
        (Some(limit), _) => format!("Limit now: {}.", short_tokens(limit)),
        (None, Some(window)) => format!("Window now: {}. No limit set.", short_tokens(window)),
        (None, None) => "No limit set.".to_owned(),
    };
    format!(
        "{}: {now} Type a limit such as 350k, 1.5m or 350000, then press Enter. \
         Leave it empty, or type 0 or none, to remove the limit.",
        target.label
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_token_amount_reads_suffixes_and_plain_counts() {
        for (input, want) in [
            ("350000", 350_000),
            ("350k", 350_000),
            ("350K", 350_000),
            (" 350 k ", 350_000),
            ("1.5m", 1_500_000),
            ("1M", 1_000_000),
            ("0.5M", 500_000),
            ("350_000", 350_000),
            ("350,000", 350_000),
            ("128.5k", 128_500),
        ] {
            assert_eq!(parse_token_amount(input), Ok(Some(want)), "{input:?}");
        }
    }

    fn model(id: &str, name: &str) -> (String, acp::ModelId) {
        (name.to_owned(), acp::ModelId::new(std::sync::Arc::from(id)))
    }

    /// Providers serve "Opus". Each row names its provider, the limit
    /// shows only on its own row, and only the selected route is current.
    #[test]
    fn choices_name_the_provider_of_a_shared_model() {
        let snapshot = PagerLocalSnapshot {
            available_models: vec![
                model("alpha/opus", "Opus"),
                model("beta/opus", "Opus"),
                model("solo", "Solo"),
            ],
            model_routes: [
                ("alpha/opus".to_owned(), "alpha".to_owned()),
                ("beta/opus".to_owned(), "beta".to_owned()),
                ("solo".to_owned(), "alpha".to_owned()),
            ]
            .into(),
            model_windows: [
                ("alpha/opus".to_owned(), 1_000_000),
                ("beta/opus".to_owned(), 1_000_000),
            ]
            .into(),
            context_limits: [("beta/opus".to_owned(), 350_000)].into(),
            current_model_name: Some("Opus".to_owned()),
            current_model_id: Some("beta/opus".to_owned()),
            ..PagerLocalSnapshot::default()
        };
        let rows: Vec<(String, String)> = choices(&snapshot)
            .into_iter()
            .map(|c| (c.canonical, c.display))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "beta/opus".to_owned(),
                    "Opus (beta) \u{00B7} 350k limit".to_owned()
                ),
                (
                    "alpha/opus".to_owned(),
                    "Opus (alpha) \u{00B7} 1M".to_owned()
                ),
                ("solo".to_owned(), "Solo".to_owned()),
            ],
            "the current route first, every shared name with its provider"
        );
        assert_eq!(summary(&snapshot), "Opus (beta) 350k");
    }

    /// Ids of one name on one provider fall back to the id.
    #[test]
    fn row_labels_fall_back_to_the_id_when_the_route_is_shared() {
        let snapshot = PagerLocalSnapshot {
            available_models: vec![model("gw/opus", "Opus"), model("gw/opus-fast", "Opus")],
            model_routes: [
                ("gw/opus".to_owned(), "gw".to_owned()),
                ("gw/opus-fast".to_owned(), "gw".to_owned()),
            ]
            .into(),
            ..PagerLocalSnapshot::default()
        };
        let labels = row_labels(&snapshot);
        assert_eq!(labels["gw/opus"], "Opus (gw/opus)");
        assert_eq!(labels["gw/opus-fast"], "Opus (gw/opus-fast)");
    }

    #[test]
    fn catalog_routes_prefers_the_provider_over_the_endpoint() {
        let mut available = indexmap::IndexMap::new();
        for (id, meta) in [
            (
                "a/m",
                serde_json::json!({ "provider": "a", "endpoint": "a.example" }),
            ),
            ("b/m", serde_json::json!({ "endpoint": "b.example" })),
            ("c", serde_json::json!({})),
        ] {
            let model_id = acp::ModelId::new(std::sync::Arc::from(id));
            let serde_json::Value::Object(meta) = meta else {
                unreachable!()
            };
            available.insert(
                model_id.clone(),
                acp::ModelInfo::new(model_id, "M".to_owned()).meta(Some(meta)),
            );
        }
        let routes = catalog_routes(&available);
        assert_eq!(routes.get("a/m").map(String::as_str), Some("a"));
        assert_eq!(routes.get("b/m").map(String::as_str), Some("b.example"));
        assert!(!routes.contains_key("c"));
    }

    #[test]
    fn parse_token_amount_reads_clear_words() {
        for input in ["", "   ", "0", "none", "Default", "OFF", "clear", "unset"] {
            assert_eq!(parse_token_amount(input), Ok(None), "{input:?}");
        }
    }

    #[test]
    fn parse_token_amount_rejects_bad_input() {
        for input in [
            "abc", "k", "-5k", "1.5", "12x", "nan k", "inf m", "350", "999", "200m",
        ] {
            assert!(parse_token_amount(input).is_err(), "{input:?} must fail");
        }
        assert_eq!(
            parse_token_amount("350"),
            Err("Too small: 350 tokens. Did you mean 350k?".to_owned()),
            "a missing suffix gets a hint"
        );
    }

    #[test]
    fn short_tokens_is_compact_and_round_trips() {
        for (tokens, want) in [
            (1_000_000, "1M"),
            (350_000, "350k"),
            (1_500_000, "1.5M"),
            (131_072, "131.1k"),
            (2_097_152, "2.1M"),
            (999, "999"),
        ] {
            assert_eq!(short_tokens(tokens), want);
        }
        for tokens in [350_000, 1_000_000, 1_500_000, 64_000] {
            assert_eq!(
                parse_token_amount(&short_tokens(tokens)),
                Ok(Some(tokens)),
                "the editor text of a limit parses back to it"
            );
        }
    }

    #[test]
    fn limits_from_config_reads_only_positive_windows() {
        let root: toml::Value = toml::from_str(
            r#"
            [model."gw/big-one"]
            context_window = 350000
            [model.other]
            name = "No window"
            [model.zero]
            context_window = 0
            "#,
        )
        .unwrap();
        let limits = limits_from_config(&root);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits.get("gw/big-one"), Some(&350_000));
    }

    #[test]
    fn catalog_windows_reads_total_context_tokens_meta() {
        let mut available = indexmap::IndexMap::new();
        let id = acp::ModelId::new(std::sync::Arc::from("gw/big-one"));
        let mut meta = serde_json::Map::new();
        meta.insert("totalContextTokens".to_owned(), 1_000_000.into());
        available.insert(
            id.clone(),
            acp::ModelInfo::new(id, "Big One".to_owned()).meta(Some(meta)),
        );
        let bare = acp::ModelId::new(std::sync::Arc::from("bare"));
        available.insert(bare.clone(), acp::ModelInfo::new(bare, "Bare".to_owned()));
        let windows = catalog_windows(&available);
        assert_eq!(windows.get("gw/big-one"), Some(&1_000_000));
        assert!(!windows.contains_key("bare"));
    }
}
