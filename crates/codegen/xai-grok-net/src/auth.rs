// auth.rs holds the auth code.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::AuthRefusal;
use crate::session;

/// Bifrost reads its virtual key only from this header, not from `Authorization`.
const VIRTUAL_KEY: &str = "x-bf-vk";
const X_API_KEY: &str = "x-api-key";

/// The credential a request carries. Every request names one, `Auth::None` included.
#[derive(Clone)]
pub enum Auth {
    /// No credential.
    None,
    /// A key that belongs to the endpoint: a provider key, a gateway key, an xAI API key.
    ApiKey { key: String, scheme: KeyScheme },
    /// The xAI login token. Sent only to an xAI host; refused for any other.
    XaiSession(String),
    /// Headers the user wrote in config for this endpoint, sent as written.
    Configured(HeaderMap),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::None => "None",
            Self::ApiKey { .. } => "ApiKey",
            Self::XaiSession(_) => "XaiSession",
            Self::Configured(_) => "Configured",
        };
        f.write_str(kind)
    }
}

/// Which header carries an [`Auth::ApiKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KeyScheme {
    /// `Authorization: Bearer <key>`.
    #[default]
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
}

/// True for a host xAI operates: `x.ai`, `grok.com`, or a subdomain of either, over https.
/// A loopback mock counts only in a test build that turns on `test-loopback-session`.
pub fn is_xai_host(url: &reqwest::Url) -> bool {
    if cfg!(feature = "test-loopback-session") && is_loopback(url) {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    ["x.ai", "grok.com"].iter().any(|apex| {
        host == *apex
            || host
                .strip_suffix(apex)
                .is_some_and(|sub| sub.ends_with('.'))
    })
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// True for a header name that carries a credential. Only [`Auth`] may set one.
pub fn is_credential_header(name: &HeaderName) -> bool {
    name == reqwest::header::AUTHORIZATION
        || name == reqwest::header::PROXY_AUTHORIZATION
        || name == reqwest::header::COOKIE
        || name.as_str() == X_API_KEY
        || name.as_str() == VIRTUAL_KEY
}

/// Write `auth` into `headers` for a request to `url`.
pub(crate) fn apply(
    auth: &Auth,
    url: &reqwest::Url,
    headers: &mut HeaderMap,
) -> Result<(), AuthRefusal> {
    match auth {
        Auth::None => {}
        Auth::XaiSession(token) => {
            session::register_session_token(token);
            if !is_xai_host(url) {
                return Err(AuthRefusal::SessionOffXaiHost(host_of(url)));
            }
            insert(
                headers,
                reqwest::header::AUTHORIZATION,
                &format!("Bearer {token}"),
            )?;
        }
        Auth::ApiKey { key, scheme } => {
            match scheme {
                KeyScheme::Bearer => {
                    insert(
                        headers,
                        reqwest::header::AUTHORIZATION,
                        &format!("Bearer {key}"),
                    )?;
                }
                KeyScheme::XApiKey => {
                    insert(headers, HeaderName::from_static(X_API_KEY), key)?;
                }
            }
            insert(headers, HeaderName::from_static(VIRTUAL_KEY), key)?;
        }
        Auth::Configured(configured) => {
            for (name, value) in configured {
                headers.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(())
}

/// The last check before a request leaves: no header may carry a session token to a
/// host xAI does not operate. This covers a token passed as an API key or a configured header.
pub(crate) fn check_outgoing(url: &reqwest::Url, headers: &HeaderMap) -> Result<(), AuthRefusal> {
    if is_xai_host(url) {
        return Ok(());
    }
    for (name, value) in headers {
        if session::carries_session_token(value.as_bytes()) {
            return Err(AuthRefusal::SessionTokenInHeader {
                header: name.as_str().to_owned(),
                host: host_of(url),
            });
        }
    }
    Ok(())
}

fn insert(headers: &mut HeaderMap, name: HeaderName, value: &str) -> Result<(), AuthRefusal> {
    let value = HeaderValue::from_str(value).map_err(|_| AuthRefusal::InvalidValue)?;
    headers.insert(name, value);
    Ok(())
}

pub(crate) fn host_of(url: &reqwest::Url) -> String {
    url.host_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).unwrap()
    }

    #[test]
    fn xai_hosts_are_x_ai_and_grok_com_over_https() {
        for yes in [
            "https://api.x.ai/v1",
            "https://x.ai",
            "https://cli-chat-proxy.grok.com/v1",
            "https://grok.com",
        ] {
            assert!(is_xai_host(&url(yes)), "{yes}");
        }
        for no in [
            "https://gateway.pazer.ai/v1",
            "http://api.x.ai/v1",
            "https://evilx.ai",
            "https://x.ai.example.com",
            "https://notgrok.com",
            "https://openrouter.ai/api/v1",
        ] {
            assert!(!is_xai_host(&url(no)), "{no}");
        }
    }

    #[test]
    fn session_is_refused_off_an_xai_host() {
        let mut headers = HeaderMap::new();
        let refused = apply(
            &Auth::XaiSession("session-a".into()),
            &url("https://gateway.pazer.ai/v1"),
            &mut headers,
        );
        assert!(matches!(refused, Err(AuthRefusal::SessionOffXaiHost(_))));
        assert!(headers.is_empty());
    }

    #[test]
    fn api_key_goes_in_its_header_and_the_virtual_key_header() {
        for (scheme, header, value) in [
            (KeyScheme::Bearer, "authorization", "Bearer sk-1"),
            (KeyScheme::XApiKey, "x-api-key", "sk-1"),
        ] {
            let mut headers = HeaderMap::new();
            apply(
                &Auth::ApiKey {
                    key: "sk-1".into(),
                    scheme,
                },
                &url("https://gateway.pazer.ai/v1"),
                &mut headers,
            )
            .unwrap();
            assert_eq!(headers[header], value);
            assert_eq!(headers["x-bf-vk"], "sk-1");
        }
    }

    #[test]
    fn a_session_token_passed_as_a_key_is_refused_off_an_xai_host() {
        session::register_session_token("session-b");
        let mut headers = HeaderMap::new();
        apply(
            &Auth::ApiKey {
                key: "session-b".into(),
                scheme: KeyScheme::Bearer,
            },
            &url("https://gateway.pazer.ai/v1"),
            &mut headers,
        )
        .unwrap();
        let refused = check_outgoing(&url("https://gateway.pazer.ai/v1"), &headers);
        assert!(matches!(
            refused,
            Err(AuthRefusal::SessionTokenInHeader { .. })
        ));
        assert!(check_outgoing(&url("https://api.x.ai/v1"), &headers).is_ok());
    }
}
