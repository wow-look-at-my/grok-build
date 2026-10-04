//! Credentials for outbound HTTP made by the data-collector.

use reqwest::RequestBuilder;

use crate::visibility::HttpAuth;

/// Snapshot of the currently effective credentials.
/// Used by callers that build their own header maps (the OTel OTLP exporter) or that need the bearer prefix for 401-attribution telemetry.
#[derive(Clone, Debug, Default)]
pub struct CredentialSnapshot {
    /// Bearer token. `None` when no auth is configured (CI / `--api-key` headless).
    pub token: Option<String>,
    /// User identifier matching the bearer token's owner.
    pub user_id: Option<String>,
    /// Team identifier from OAuth. `None` for personal accounts or when no auth is configured.
    pub team_id: Option<String>,
    /// `uuidv5(NAMESPACE_OID, api_key)`, set only for `AuthMode::ApiKey`.
    pub api_key_id: Option<String>,
    /// Org id from the OIDC `organizationId` claim; `None` for personal auth.
    pub organization_id: Option<String>,
}

/// Source of truth for outbound auth on data-collector requests. Callers add headers via `HttpAuth::apply`.
#[async_trait::async_trait]
pub trait AuthCredentialProvider: HttpAuth + Send + Sync + 'static {
    /// Implementations should issue a cheap disk re-read (`AuthManager::refresh`) before snapshotting.
    fn snapshot(&self) -> CredentialSnapshot;

    /// Attempt to obtain a fresh token.
    async fn refresh_after_unauthorized(&self) -> bool;

    /// Whether `X-XAI-Token-Auth` should be sent with the bearer token.
    /// `false` for a bare Bearer, `true` for user/OAuth tokens.
    fn needs_token_auth_header(&self) -> bool {
        true
    }

    /// Whether the provider holds a credential worth a real outbound attempt:
    /// an unexpired token (in memory or on disk), or a static key.
    fn has_usable_credential(&self) -> bool {
        true
    }
}

/// Static credential provider for tests and callers with a raw token and no
/// `AuthManager`.
pub struct StaticAuthCredentialProvider {
    inner: Box<dyn HttpAuth>,
    bearer: Option<String>,
}

impl StaticAuthCredentialProvider {
    /// Wrap `inner` so callers see it as an `AuthCredentialProvider`.
    pub fn new(inner: Box<dyn HttpAuth>, bearer: Option<String>) -> Self {
        Self { inner, bearer }
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
    fn apply(&self, builder: RequestBuilder, base_url: &str) -> RequestBuilder {
        self.inner.apply(builder, base_url)
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
}
