//! How an upstream provider is drawn: its name, its colour and its mark.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use ratatui::style::Color;

/// A provider's mark, ready for the terminal's graphics protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIcon {
    /// The graphics image id. One per mark, so a transmit is reused.
    pub id: u32,
    pub png: Arc<[u8]>,
}

/// How one provider is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderBrand {
    pub name: String,
    pub color: Color,
    pub icon: Option<ProviderIcon>,
}

struct Known {
    /// Normalized names this entry answers to. The first is its icon key.
    keys: &'static [&'static str],
    name: &'static str,
    rgb: (u8, u8, u8),
    icon: Option<&'static [u8]>,
}

macro_rules! icon {
    ($file:literal) => {
        Some(include_bytes!(concat!("../../assets/provider-icons/", $file)).as_slice())
    };
}

const NEUTRAL: (u8, u8, u8) = (0x9C, 0xA3, 0xAF);

static KNOWN: &[Known] = &[
    Known {
        keys: &["anthropic", "claude"],
        name: "Anthropic",
        rgb: (0xD9, 0x77, 0x57),
        icon: icon!("anthropic.png"),
    },
    Known {
        keys: &["openai", "azureopenai"],
        name: "OpenAI",
        rgb: (0x74, 0xAA, 0x9C),
        icon: None,
    },
    Known {
        keys: &["googleaistudio", "gemini", "googlegemini"],
        name: "Google AI Studio",
        rgb: (0x8E, 0x75, 0xB2),
        icon: icon!("googlegemini.png"),
    },
    Known {
        keys: &["googlevertex", "vertex", "vertexai", "googlecloud"],
        name: "Google Vertex",
        rgb: (0x42, 0x85, 0xF4),
        icon: icon!("googlecloud.png"),
    },
    Known {
        keys: &["google"],
        name: "Google",
        rgb: (0x42, 0x85, 0xF4),
        icon: icon!("google.png"),
    },
    Known {
        keys: &["mistral", "mistralai"],
        name: "Mistral",
        rgb: (0xFA, 0x52, 0x0F),
        icon: icon!("mistralai.png"),
    },
    Known {
        keys: &["meta", "metallama", "metaai"],
        name: "Meta",
        rgb: (0x04, 0x67, 0xDF),
        icon: icon!("meta.png"),
    },
    Known {
        keys: &["deepseek"],
        name: "DeepSeek",
        rgb: (0x57, 0x86, 0xFE),
        icon: icon!("deepseek.png"),
    },
    Known {
        keys: &["perplexity"],
        name: "Perplexity",
        rgb: (0x1F, 0xB8, 0xCD),
        icon: icon!("perplexity.png"),
    },
    Known {
        keys: &["nvidia"],
        name: "NVIDIA",
        rgb: (0x76, 0xB9, 0x00),
        icon: icon!("nvidia.png"),
    },
    Known {
        keys: &["huggingface"],
        name: "Hugging Face",
        rgb: (0xFF, 0xD2, 0x1E),
        icon: icon!("huggingface.png"),
    },
    Known {
        keys: &["ollama"],
        name: "Ollama",
        rgb: NEUTRAL,
        icon: icon!("ollama.png"),
    },
    Known {
        keys: &["alibaba", "alibabacloud", "qwen"],
        name: "Alibaba",
        rgb: (0xFF, 0x6A, 0x00),
        icon: icon!("alibabacloud.png"),
    },
    Known {
        keys: &["minimax"],
        name: "MiniMax",
        rgb: (0xE7, 0x35, 0x62),
        icon: icon!("minimax.png"),
    },
    Known {
        keys: &["openrouter"],
        name: "OpenRouter",
        rgb: (0x94, 0xA3, 0xB8),
        icon: icon!("openrouter.png"),
    },
    Known {
        keys: &["baidu"],
        name: "Baidu",
        rgb: (0x4E, 0x6E, 0xF2),
        icon: icon!("baidu.png"),
    },
    Known {
        keys: &["cloudflare"],
        name: "Cloudflare",
        rgb: (0xF3, 0x80, 0x20),
        icon: icon!("cloudflare.png"),
    },
    Known {
        keys: &["replicate"],
        name: "Replicate",
        rgb: NEUTRAL,
        icon: icon!("replicate.png"),
    },
    Known {
        keys: &["xai", "grok"],
        name: "xAI",
        rgb: NEUTRAL,
        icon: None,
    },
    Known {
        keys: &["groq"],
        name: "Groq",
        rgb: (0xF5, 0x50, 0x36),
        icon: None,
    },
    Known {
        keys: &["cerebras"],
        name: "Cerebras",
        rgb: (0xF1, 0x5A, 0x29),
        icon: None,
    },
    Known {
        keys: &["together", "togetherai"],
        name: "Together",
        rgb: (0x3B, 0x82, 0xF6),
        icon: None,
    },
    Known {
        keys: &["fireworks", "fireworksai"],
        name: "Fireworks",
        rgb: (0x8B, 0x5C, 0xF6),
        icon: None,
    },
    Known {
        keys: &["deepinfra"],
        name: "DeepInfra",
        rgb: (0x38, 0xBD, 0xF8),
        icon: None,
    },
    Known {
        keys: &["amazonbedrock", "bedrock", "amazon", "aws"],
        name: "Amazon Bedrock",
        rgb: (0xFF, 0x99, 0x00),
        icon: None,
    },
    Known {
        keys: &["azure", "microsoft"],
        name: "Azure",
        rgb: (0x2B, 0x8C, 0xE6),
        icon: None,
    },
    Known {
        keys: &["cohere"],
        name: "Cohere",
        rgb: (0xFF, 0x77, 0x59),
        icon: None,
    },
    Known {
        keys: &["sambanova"],
        name: "SambaNova",
        rgb: (0xEE, 0x76, 0x24),
        icon: None,
    },
    Known {
        keys: &["novita", "novitaai"],
        name: "Novita",
        rgb: (0x23, 0xD5, 0x7C),
        icon: None,
    },
    Known {
        keys: &["moonshotai", "moonshot"],
        name: "Moonshot AI",
        rgb: (0x5B, 0x8D, 0xEF),
        icon: None,
    },
    Known {
        keys: &["zai", "zhipu", "zhipuai"],
        name: "Z.AI",
        rgb: (0x5A, 0x7B, 0xFF),
        icon: None,
    },
];

/// Colours for a provider this table does not know. The name picks one, so a provider keeps its colour.
const FALLBACK: &[(u8, u8, u8)] = &[
    (0x7A, 0xA2, 0xF7),
    (0xBB, 0x9A, 0xF7),
    (0x9E, 0xCE, 0x6A),
    (0xE0, 0xAF, 0x68),
    (0x7D, 0xCF, 0xFF),
    (0xF7, 0x76, 0x8E),
    (0x2A, 0xC3, 0xDE),
    (0xFF, 0x9E, 0x64),
];

/// The first graphics image id of the bundled marks. Inline media counts up from a low id.
const BUNDLED_ICON_ID_BASE: u32 = 0x7000_0000;
/// The first graphics image id of the marks a user supplied.
const USER_ICON_ID_BASE: u32 = 0x7100_0000;

/// A name as the table keys it: lower case, letters and digits only.
pub fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn known(key: &str) -> Option<(usize, &'static Known)> {
    KNOWN
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.keys.contains(&key))
}

fn fallback_color(key: &str) -> (u8, u8, u8) {
    let hash = key.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    FALLBACK
        .get(hash as usize % FALLBACK.len())
        .copied()
        .unwrap_or((0x9C, 0xA3, 0xAF))
}

/// The bundled marks, shared: a frame that draws a mark clones a pointer, not the PNG.
static BUNDLED: LazyLock<Vec<Option<Arc<[u8]>>>> = LazyLock::new(|| {
    KNOWN
        .iter()
        .map(|entry| entry.icon.map(Arc::from))
        .collect()
});

static USER_ICONS: LazyLock<Mutex<HashMap<String, Option<Arc<[u8]>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The PNG a user put at `$GROK_HOME/provider-icons/<key>.png`. Read one time per key.
fn user_icon(key: &str) -> Option<Arc<[u8]>> {
    let mut cache = USER_ICONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.get(key) {
        return cached.clone();
    }
    let path = xai_dirs::grok_home()
        .join("provider-icons")
        .join(format!("{key}.png"));
    let icon = match std::fs::read(&path) {
        Ok(bytes) if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some(Arc::from(bytes)),
        Ok(_) => {
            tracing::error!(path = %path.display(), "provider icon is not a PNG; it is not drawn");
            None
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::error!(path = %path.display(), %error, "provider icon could not be read");
            None
        }
    };
    cache.insert(key.to_owned(), icon.clone());
    icon
}

/// How to draw the provider a gateway named, or a model vendor named in a model id.
pub fn brand_for(name: &str) -> ProviderBrand {
    let key = normalize(name);
    let entry = known(&key);
    let rgb = entry.map_or_else(|| fallback_color(&key), |(_, entry)| entry.rgb);
    let user = (!key.is_empty()).then(|| user_icon(&key)).flatten();
    let icon = match (user, entry) {
        (Some(png), _) => {
            let hash = key.bytes().fold(0u32, |hash, byte| {
                hash.wrapping_mul(31).wrapping_add(u32::from(byte))
            });
            Some(ProviderIcon {
                id: USER_ICON_ID_BASE + (hash & 0xFFFF),
                png,
            })
        }
        (None, Some((index, _))) => BUNDLED
            .get(index)
            .cloned()
            .flatten()
            .map(|png| ProviderIcon {
                id: BUNDLED_ICON_ID_BASE + index as u32,
                png,
            }),
        (None, None) => None,
    };
    ProviderBrand {
        name: entry.map_or_else(
            || name.trim().to_owned(),
            |(_, entry)| entry.name.to_owned(),
        ),
        color: Color::Rgb(rgb.0, rgb.1, rgb.2),
        icon,
    }
}

/// The vendor a model id names: the first segment of `vendor/model` that is
/// not one of `skip`. `skip` holds the ids of the hops and of the gateway, so
/// `openrouter/anthropic/claude` answers `anthropic`.
pub fn vendor_of_model_id<'a>(model_id: &'a str, skip: &[&str]) -> Option<&'a str> {
    let mut segments: Vec<&str> = model_id.split('/').collect();
    // The last segment is the model itself.
    segments.pop()?;
    segments.into_iter().map(str::trim).find(|segment| {
        !segment.is_empty()
            && !skip
                .iter()
                .any(|skipped| skipped.eq_ignore_ascii_case(segment))
    })
}

/// The provider to draw for a model call. A gateway's own answer wins, unless
/// it only repeats a link of the chain. The model id's vendor is the fallback.
pub fn serving_provider(
    reported: Option<&str>,
    model_id: &str,
    chain_ids: &[&str],
) -> Option<ProviderBrand> {
    let reported = reported
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| !chain_ids.iter().any(|id| id.eq_ignore_ascii_case(name)));
    reported
        .or_else(|| vendor_of_model_id(model_id, chain_ids))
        .map(brand_for)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_provider_has_its_name_colour_and_mark() {
        let brand = brand_for("anthropic");
        assert_eq!(brand.name, "Anthropic");
        assert_eq!(brand.color, Color::Rgb(0xD9, 0x77, 0x57));
        let icon = brand.icon.expect("the mark is bundled");
        assert!(icon.png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(brand_for("Anthropic").icon.unwrap().id, icon.id);
    }

    #[test]
    fn spacing_and_case_do_not_change_the_match() {
        assert_eq!(brand_for("Google AI Studio").name, "Google AI Studio");
        assert_eq!(brand_for("google-ai-studio").name, "Google AI Studio");
        assert_eq!(brand_for("Amazon Bedrock").name, "Amazon Bedrock");
        assert_eq!(brand_for("DEEPSEEK").name, "DeepSeek");
    }

    #[test]
    fn every_bundled_mark_is_a_png_with_its_own_id() {
        let mut ids = std::collections::HashSet::new();
        for entry in KNOWN {
            let brand = brand_for(entry.keys[0]);
            assert_eq!(brand.name, entry.name);
            if let Some(icon) = brand.icon {
                assert!(icon.png.starts_with(b"\x89PNG\r\n\x1a\n"), "{}", entry.name);
                assert!(ids.insert(icon.id), "{} shares an image id", entry.name);
            }
        }
    }

    #[test]
    fn no_two_entries_share_a_key() {
        let mut keys = std::collections::HashSet::new();
        for entry in KNOWN {
            for key in entry.keys {
                assert_eq!(*key, normalize(key), "`{key}` is not normalized");
                assert!(keys.insert(*key), "`{key}` is listed twice");
            }
        }
    }

    #[test]
    fn an_unknown_provider_keeps_its_name_and_gets_a_stable_colour() {
        let first = brand_for("Parasail");
        assert_eq!(first.name, "Parasail");
        assert_eq!(first.icon, None);
        assert_eq!(first.color, brand_for("parasail").color);
    }

    #[test]
    fn the_vendor_is_the_first_segment_that_is_not_a_link() {
        assert_eq!(
            vendor_of_model_id("anthropic/claude-sonnet", &[]),
            Some("anthropic")
        );
        assert_eq!(
            vendor_of_model_id("openrouter/deepseek/deepseek-chat", &["openrouter", "nas"]),
            Some("deepseek")
        );
        assert_eq!(vendor_of_model_id("gpt-5", &[]), None);
        assert_eq!(vendor_of_model_id("openrouter/auto", &["openrouter"]), None);
    }

    #[test]
    fn a_gateway_that_only_repeats_a_link_does_not_name_the_provider() {
        let chain = ["nas", "bifrost", "openrouter"];
        let named = serving_provider(Some("DeepInfra"), "deepseek/deepseek-chat", &chain).unwrap();
        assert_eq!(named.name, "DeepInfra");
        let repeated =
            serving_provider(Some("openrouter"), "deepseek/deepseek-chat", &chain).unwrap();
        assert_eq!(repeated.name, "DeepSeek");
        assert_eq!(serving_provider(None, "local-model", &chain), None);
    }
}
