//! Credentials for outbound HTTP made by the data-collector.
//! Shell installs `ShellAuthCredentialProvider` wrapping `AuthManager` + `TokenRefresher`.
//! Data-collector code holds an `Arc<dyn AuthCredentialProvider>`.

use std::sync::Arc;

use xai_grok_net::{Auth, KeyScheme};

use crate::visibility::{HttpAuth, RequestAuth};

/// Snapshot of the currently effective credentials.
/// Used by callers that build their own header maps (the OTel OTLP exporter) or that need the bearer prefix for 401-attribution telemetry.
#[derive(Clone, Debug, Default)]
pub struct CredentialSnapshot {
    /// Bearer token. `None` when no auth is configured (CI / `--api-key` headless).
    pub token: Option<String>,
    /// User identifier matching the bearer token's owner.
    /// `None` when no auth is configured or when the underlying provider has no concept of user identity (`StaticAuthCredentialProvider`).
    /// Read by the OTel layer to populate the `user.id` resource attribute.
    pub user_id: Option<String>,
    /// Team identifier from OAuth. `None` for personal accounts or when no auth is configured.
    pub team_id: Option<String>,
    /// `uuidv5(NAMESPACE_OID, api_key)`, set only for `AuthMode::ApiKey`.
    pub api_key_id: Option<String>,
    /// Org id from the OIDC `organizationId` claim; `None` for personal auth.
    pub organization_id: Option<String>,
}

/// Source of truth for outbound auth on data-collector requests.
/// Supertrait of `HttpAuth` so one impl covers refresh-aware snapshot, 401 recovery, and header construction.
/// Callers get the credential via `HttpAuth::request_auth`.
#[async_trait::async_trait]
pub trait AuthCredentialProvider: HttpAuth + Send + Sync + 'static {
    /// Implementations should issue a cheap disk re-read (`AuthManager::refresh`) before snapshotting.
    /// This lets callers see updates from sibling processes (`grok-desktop`, `grok login`).
    /// The `token` field MUST mirror the bearer that `HttpAuth::request_auth` would send on the wire so 401-attribution prefixes match the actual request.
    fn snapshot(&self) -> CredentialSnapshot;

    /// Attempt to obtain a fresh token.
    /// Returns `true` if a different token was obtained; the caller should retry the failed request once.
    /// Returns `false` if no refresher is configured or refresh failed.
    async fn refresh_after_unauthorized(&self) -> bool;

    /// Whether `X-XAI-Token-Auth` should be sent with the bearer token.
    /// `false` for a bare Bearer, `true` for user/OAuth tokens.
    /// See `GrokAuthCredentials::apply()` for the wire format contract.
    fn needs_token_auth_header(&self) -> bool {
        true
    }

    /// Whether the provider holds a credential worth a real outbound attempt: an unexpired token (in memory or on disk), or a static key.
    /// The default `true` always attempts.
    fn has_usable_credential(&self) -> bool {
        true
    }

    /// The credential to send now. It names the same token as `snapshot().token`.
    /// The default reads the snapshot: a token with an `api_key_id` is an xAI API key. Any other token is the xAI session.
    fn current_auth(&self) -> Auth {
        let snapshot = self.snapshot();
        match snapshot.token {
            None => Auth::None,
            Some(key) if snapshot.api_key_id.is_some() => Auth::ApiKey {
                key,
                scheme: KeyScheme::Bearer,
            },
            Some(token) => Auth::XaiSession(token),
        }
    }
}

struct ProviderSource(Arc<dyn AuthCredentialProvider>);

#[async_trait::async_trait]
impl xai_grok_net::CredentialSource for ProviderSource {
    fn current(&self) -> Auth {
        self.0.current_auth()
    }

    async fn refresh_after_unauthorized(&self) -> bool {
        self.0.refresh_after_unauthorized().await
    }
}

/// `provider` as a credential source for `xai-grok-net`.
pub fn credential_source(
    provider: Arc<dyn AuthCredentialProvider>,
) -> Arc<dyn xai_grok_net::CredentialSource> {
    Arc::new(ProviderSource(provider))
}

/// The credential for requests that read `provider` at send time. The plain headers come from
/// `HttpAuth::request_auth`.
pub fn retrying_auth(
    provider: Arc<dyn AuthCredentialProvider>,
    base_url: &str,
    retries_on_401: u32,
) -> RequestAuth {
    let headers = provider.request_auth(base_url).headers;
    RequestAuth {
        auth: Auth::Source {
            source: credential_source(provider),
            retries_on_401,
        },
        headers,
    }
}

/// The tail of the bearer that produced `response`, when one was sent.
pub fn sent_bearer_suffix(
    response: &xai_grok_net::Response,
) -> Option<xai_grok_net::SentBearerSuffix> {
    response
        .extensions()
        .get::<xai_grok_net::SentBearerSuffix>()
        .cloned()
}

/// Static credential provider for tests and callers with a raw token and no `AuthManager`.
/// `bearer` is the wire bearer; stored so `snapshot().token` matches what goes out (401-attribution).
/// `None` when no bearer is configured.
pub struct StaticAuthCredentialProvider {
    inner: Box<dyn HttpAuth>,
    bearer: Option<String>,
    /// True when `bearer` is a key that belongs to the endpoint, not the xAI session.
    api_key: bool,
}

impl StaticAuthCredentialProvider {
    /// Wrap `inner` so callers see it as an `AuthCredentialProvider`.
    pub fn new(inner: Box<dyn HttpAuth>, bearer: Option<String>) -> Self {
        Self {
            inner,
            bearer,
            api_key: false,
        }
    }

    /// A provider that sends `key` as `Authorization: Bearer`. The key belongs to the endpoint.
    pub fn api_key(key: Option<String>) -> Self {
        Self {
            inner: Box::new(BearerKey(key.clone())),
            bearer: key,
            api_key: true,
        }
    }
}

/// `HttpAuth` for a fixed endpoint key.
struct BearerKey(Option<String>);

impl HttpAuth for BearerKey {
    fn request_auth(&self, _base_url: &str) -> RequestAuth {
        let auth = match &self.0 {
            Some(key) => Auth::ApiKey {
                key: key.clone(),
                scheme: KeyScheme::Bearer,
            },
            None => Auth::None,
        };
        RequestAuth {
            auth,
            headers: Default::default(),
        }
    }
}

impl std::fmt::Debug for StaticAuthCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticAuthCredentialProvider")
            .field("has_bearer", &self.bearer.is_some())
            .finish()
    }
}

impl HttpAuth for StaticAuthCredentialProvider {
    fn request_auth(&self, base_url: &str) -> RequestAuth {
        self.inner.request_auth(base_url)
    }
}

#[async_trait::async_trait]
impl AuthCredentialProvider for StaticAuthCredentialProvider {
    fn snapshot(&self) -> CredentialSnapshot {
        CredentialSnapshot {
            token: self.bearer.clone(),
            ..Default::default()
        }
    }

    async fn refresh_after_unauthorized(&self) -> bool {
        false
    }

    fn current_auth(&self) -> Auth {
        match &self.bearer {
            None => Auth::None,
            Some(key) if self.api_key => Auth::ApiKey {
                key: key.clone(),
                scheme: KeyScheme::Bearer,
            },
            Some(token) => Auth::XaiSession(token.clone()),
        }
    }
}
