//! The endpoints a model request may reach. Nothing is allowed until the user
//! lists it, in `[endpoints] allowed_endpoints` or in `GROK_ALLOWED_ENDPOINTS`.
//!
//! An entry is a host (`api.example.com`), a host and port (`localhost:11434`),
//! every subdomain of a host (`*.example.com`), or a URL
//! (`https://gateway.example.com/v1`). A URL entry also pins the scheme, the
//! port and a path prefix.

use std::sync::RwLock;

/// Comma-separated entries, added to what the config lists.
pub const ENV_GROK_ALLOWED_ENDPOINTS: &str = "GROK_ALLOWED_ENDPOINTS";

static CONFIGURED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Replace the entries that came from config. The environment adds to them.
pub fn set_configured(entries: Vec<String>) {
    let mut guard = CONFIGURED.write().unwrap_or_else(|p| p.into_inner());
    *guard = entries;
}

/// Every entry now in force: config first, then the environment.
pub fn allowed_endpoints() -> Vec<String> {
    let mut entries = CONFIGURED.read().unwrap_or_else(|p| p.into_inner()).clone();
    if let Ok(env) = std::env::var(ENV_GROK_ALLOWED_ENDPOINTS) {
        entries.extend(env.split(',').map(str::to_owned));
    }
    entries.retain(|e| !e.trim().is_empty());
    entries
}

/// Why a request was refused before it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointRefusal {
    /// The URL is blank or cannot be parsed, so it names no endpoint.
    NoUrl { url: String },
    /// The URL names an endpoint the user has not listed.
    NotAllowed { url: String, origin: String },
}

impl std::fmt::Display for EndpointRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoUrl { url } if url.trim().is_empty() => write!(
                f,
                "No URL is configured for this request, so nothing was sent. \
                 Set the URL in config.toml."
            ),
            Self::NoUrl { url } => write!(f, "\"{url}\" is not a valid URL, so nothing was sent."),
            Self::NotAllowed { origin, .. } => write!(
                f,
                "{origin} is not an allowed endpoint, so nothing was sent. \
                 Add \"{origin}\" to [endpoints] allowed_endpoints in config.toml \
                 to allow it."
            ),
        }
    }
}

impl std::error::Error for EndpointRefusal {}

/// Refuse `url` unless an allowed entry covers it.
pub fn check(url: &str) -> Result<(), EndpointRefusal> {
    check_against(url, &allowed_endpoints())
}

/// [`check`] against an explicit list, for callers and tests that hold one.
pub fn check_against(url: &str, entries: &[String]) -> Result<(), EndpointRefusal> {
    let Ok(target) = reqwest::Url::parse(url.trim()) else {
        return Err(EndpointRefusal::NoUrl {
            url: url.to_owned(),
        });
    };
    let Some(host) = target.host_str() else {
        return Err(EndpointRefusal::NoUrl {
            url: url.to_owned(),
        });
    };
    if entries
        .iter()
        .any(|entry| entry_covers(entry.trim(), &target))
    {
        return Ok(());
    }
    let origin = match target.port() {
        Some(port) => format!("{}://{host}:{port}", target.scheme()),
        None => format!("{}://{host}", target.scheme()),
    };
    Err(EndpointRefusal::NotAllowed {
        url: url.to_owned(),
        origin,
    })
}

fn entry_covers(entry: &str, target: &reqwest::Url) -> bool {
    let Some(target_host) = target.host_str() else {
        return false;
    };
    if entry.contains("://") {
        let Ok(allowed) = reqwest::Url::parse(entry) else {
            return false;
        };
        let path = allowed.path().trim_end_matches('/');
        return allowed.scheme() == target.scheme()
            && allowed
                .host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(target_host))
            && allowed.port_or_known_default() == target.port_or_known_default()
            && (path.is_empty()
                || target.path() == path
                || target.path().starts_with(&format!("{path}/")));
    }
    if let Some(suffix) = entry.strip_prefix("*.") {
        let host = target_host.to_ascii_lowercase();
        let suffix = suffix.to_ascii_lowercase();
        return host.len() > suffix.len() + 1
            && host.ends_with(&suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.';
    }
    let Ok(allowed) = reqwest::Url::parse(&format!("any://{entry}")) else {
        return false;
    };
    if !allowed
        .host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case(target_host))
    {
        return false;
    }
    match allowed.port() {
        Some(port) => target.port_or_known_default() == Some(port),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| (*e).to_owned()).collect()
    }

    #[test]
    fn nothing_is_allowed_by_an_empty_list() {
        for url in [
            "https://cli-chat-proxy.grok.com/v1",
            "https://api.x.ai/v1",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
        ] {
            assert!(
                matches!(
                    check_against(url, &[]),
                    Err(EndpointRefusal::NotAllowed { .. })
                ),
                "{url} must be refused with no entries"
            );
        }
    }

    #[test]
    fn a_blank_or_broken_url_names_no_endpoint() {
        let all = list(&["example.com"]);
        assert!(matches!(
            check_against("", &all),
            Err(EndpointRefusal::NoUrl { .. })
        ));
        assert!(matches!(
            check_against("/v1/models", &all),
            Err(EndpointRefusal::NoUrl { .. })
        ));
    }

    #[test]
    fn a_host_entry_covers_every_port_and_path_on_that_host() {
        let entries = list(&["API.example.com"]);
        assert_eq!(
            check_against("https://api.example.com/v1/chat", &entries),
            Ok(())
        );
        assert_eq!(
            check_against("http://api.example.com:8080", &entries),
            Ok(())
        );
        assert!(check_against("https://evil.example.com/v1", &entries).is_err());
        assert!(check_against("https://api.example.com.evil.test/v1", &entries).is_err());
    }

    #[test]
    fn a_wildcard_entry_covers_subdomains_only() {
        let entries = list(&["*.example.com"]);
        assert_eq!(
            check_against("https://api.example.com/v1", &entries),
            Ok(())
        );
        assert_eq!(check_against("https://a.b.example.com", &entries), Ok(()));
        assert!(check_against("https://example.com", &entries).is_err());
        assert!(check_against("https://badexample.com", &entries).is_err());
    }

    #[test]
    fn a_host_and_port_entry_pins_the_port() {
        let entries = list(&["localhost:11434"]);
        assert_eq!(check_against("http://localhost:11434/v1", &entries), Ok(()));
        assert!(check_against("http://localhost:1234/v1", &entries).is_err());
    }

    #[test]
    fn a_url_entry_pins_scheme_port_and_path_prefix() {
        let entries = list(&["https://gw.example.com/team-a/"]);
        assert_eq!(
            check_against("https://gw.example.com/team-a/v1/messages", &entries),
            Ok(())
        );
        assert_eq!(
            check_against("https://gw.example.com/team-a", &entries),
            Ok(())
        );
        assert!(check_against("http://gw.example.com/team-a/v1", &entries).is_err());
        assert!(check_against("https://gw.example.com/team-b/v1", &entries).is_err());
        assert!(check_against("https://gw.example.com/team-abc", &entries).is_err());
    }

    #[test]
    fn the_refusal_names_the_entry_to_add() {
        let err = check_against("https://api.x.ai/v1/responses", &[]).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("\"https://api.x.ai\""), "{text}");
        assert!(text.contains("allowed_endpoints"), "{text}");
    }
}
