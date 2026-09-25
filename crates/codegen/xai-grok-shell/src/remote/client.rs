use crate::session::export::{ExportedMessage, ExportedMetadata, ExportedSession};
use indexmap::IndexMap;
use prod_mc_cli_chat_proxy_types::SubagentBundle;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use xai_grok_login::backend::{ActiveAuthBackend, AuthBackend};
use xai_grok_login::{GrokAuth, GrokComConfig};
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const GROK_CODE_WEB_URL: &str = "https://grok.com";
pub fn share_url(permission_id: &str) -> String {
    let web_url =
        std::env::var("GROK_CODE_WEB_URL").unwrap_or_else(|_| GROK_CODE_WEB_URL.to_string());
    format!("{}/build/share/{}", web_url, permission_id)
}
fn add_cli_chat_proxy_headers_blocking(
    builder: reqwest::blocking::RequestBuilder,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
    url: &str,
) -> reqwest::blocking::RequestBuilder {
    let mut builder = builder
        .header("Authorization", format!("Bearer {}", &auth.key))
        .header("X-XAI-Token-Auth", GrokComConfig::default().token_header)
        .header("x-userid", &auth.user_id)
        .header("x-grok-client-version", xai_grok_version::version());
    if let Some(email) = &auth.email {
        builder = builder.header("x-email", email);
    }
    let _ = (alpha_test_key, url);
    builder
        .header(
            "x-grok-client-identifier",
            crate::http::process_client_identifier(),
        )
        .header(
            crate::http::CLIENT_MODE_HEADER,
            crate::http::process_client_mode(),
        )
}
async fn parse_json_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, BackendError> {
    let bytes = response.bytes().await?;
    serde_json::from_slice(&bytes).map_err(BackendError::from)
}
async fn add_bundle_fetch_headers(
    builder: reqwest::RequestBuilder,
    auth_manager: Option<&std::sync::Arc<xai_grok_login::AuthManager>>,
    deployment_key: Option<&str>,
    alpha_test_key: Option<&str>,
    url: &str,
) -> reqwest::RequestBuilder {
    let resolved_auth = match auth_manager {
        Some(am) if ActiveAuthBackend::default().is_xai_authority() => am.auth().await.ok(),
        _ => None,
    };
    let mut credentials = crate::util::grok_auth_credentials::GrokAuthCredentials::new(
        resolved_auth.as_ref().map(|auth| auth.key.clone()),
    );
    credentials.deployment_key = deployment_key.map(str::to_owned);
    credentials.alpha_test_key = alpha_test_key.map(str::to_owned);
    let mut builder = credentials
        .apply(builder, url)
        .header("x-grok-client-version", xai_grok_version::version());
    if deployment_key.is_none()
        && let Some(auth) = &resolved_auth
    {
        builder = builder.header("x-userid", &auth.user_id);
        if let Some(email) = &auth.email {
            builder = builder.header("x-email", email);
        }
    }
    builder = builder
        .header(
            "x-grok-client-identifier",
            crate::http::process_client_identifier(),
        )
        .header(
            crate::http::CLIENT_MODE_HEADER,
            crate::http::process_client_mode(),
        );
    xai_grok_otel::inject_trace_context_into_request(builder)
}
/// Fetch the bundled subagent cache payload from cli-chat-proxy `GET /v1/subagents/bundle`.
///
/// Uses the shell's standard auth for proxy requests: a configured deployment key takes precedence; otherwise the user-session token is used.
pub async fn fetch_subagent_bundle(
    cli_chat_proxy_base_url: &str,
    auth_manager: Option<&std::sync::Arc<xai_grok_login::AuthManager>>,
    deployment_key: Option<&str>,
    alpha_test_key: Option<&str>,
) -> Result<SubagentBundle, BackendError> {
    let url = format!("{}/subagents/bundle", cli_chat_proxy_base_url);
    let response = add_bundle_fetch_headers(
        crate::http::shared_client()
            .get(&url)
            .timeout(std::time::Duration::from_secs(10)),
        auth_manager,
        deployment_key,
        alpha_test_key,
        &url,
    )
    .await
    .send()
    .await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(BackendError::RequestFailed { status, body });
    }
    let bundle: SubagentBundle = parse_json_response(response).await?;
    tracing::debug!(
        version = %bundle.version,
        personas = bundle.personas.len(),
        roles = bundle.roles.len(),
        agents = bundle.agents.len(),
        "Fetched subagent bundle from cli-chat-proxy"
    );
    Ok(bundle)
}
/// The result of fetching a bundle: either raw tar.gz bytes from the new archive endpoint, or a parsed JSON bundle from the legacy endpoint.
#[derive(Debug)]
pub enum FetchedBundle {
    Archive(Vec<u8>),
    Legacy(SubagentBundle),
}
/// Fetch a bundle, trying the archive endpoint first and falling back to legacy JSON on any non-success HTTP status.
pub async fn fetch_bundle(
    cli_chat_proxy_base_url: &str,
    auth_manager: Option<&std::sync::Arc<xai_grok_login::AuthManager>>,
    deployment_key: Option<&str>,
    alpha_test_key: Option<&str>,
) -> Result<FetchedBundle, BackendError> {
    fetch_bundle_inner(
        cli_chat_proxy_base_url,
        auth_manager,
        deployment_key,
        alpha_test_key,
    )
    .await
}
async fn fetch_bundle_inner(
    cli_chat_proxy_base_url: &str,
    auth_manager: Option<&std::sync::Arc<xai_grok_login::AuthManager>>,
    deployment_key: Option<&str>,
    alpha_test_key: Option<&str>,
) -> Result<FetchedBundle, BackendError> {
    let archive_url = format!("{}/bundle/archive", cli_chat_proxy_base_url);
    let raw_client = crate::http::shared_client();
    let client: reqwest_middleware::ClientWithMiddleware = if let Some(am) = auth_manager {
        let provider: std::sync::Arc<dyn xai_grok_auth::AuthCredentialProvider> = std::sync::Arc::new(
            xai_grok_login::credential_provider::ShellAuthCredentialProvider::with_deployment_id_resolver(
                am.clone(),
                deployment_key.map(str::to_owned),
                alpha_test_key.map(str::to_owned),
                std::sync::Arc::new(crate::managed_config::resolve_deployment_id),
            ),
        );
        crate::http::with_auth_retry(raw_client, provider)
    } else {
        reqwest_middleware::ClientBuilder::new(raw_client).build()
    };
    let mut request = client
        .get(&archive_url)
        .timeout(std::time::Duration::from_secs(30))
        .header("x-grok-client-version", xai_grok_version::version())
        .header(
            crate::http::CLIENT_MODE_HEADER,
            crate::http::process_client_mode(),
        );
    if deployment_key.is_none()
        && let Some(am) = auth_manager
        && let Some(auth) = am.current()
    {
        request = request.header("x-userid", &auth.user_id);
        if let Some(ref email) = auth.email {
            request = request.header("x-email", email);
        }
    }
    let archive_response = request.send().await.map_err(|e| match e {
        reqwest_middleware::Error::Reqwest(e) => BackendError::Network(e),
        reqwest_middleware::Error::Middleware(e) => BackendError::Auth(e.to_string()),
    })?;
    if archive_response.status().is_success() {
        let bytes = archive_response.bytes().await?;
        return Ok(FetchedBundle::Archive(bytes.to_vec()));
    }
    if archive_response.status() == reqwest::StatusCode::UNAUTHORIZED {
        let body = archive_response.text().await.unwrap_or_default();
        return Err(BackendError::RequestFailed { status: 401, body });
    }
    tracing::debug!(
        status = %archive_response.status(),
        "archive endpoint unavailable, falling back to legacy JSON"
    );
    let bundle = fetch_subagent_bundle(
        cli_chat_proxy_base_url,
        auth_manager,
        deployment_key,
        alpha_test_key,
    )
    .await?;
    Ok(FetchedBundle::Legacy(bundle))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareResponse {
    pub permission_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LoadDataResponse {
    pub messages: Option<Vec<LoadedMessage>>,
    pub session: Option<SessionInfo>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LoadedMessage {
    pub id: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub status: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SaveDataRequest {
    pub messages: Vec<ExportedMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpsertSessionRequest {
    pub session: SessionUpdate,
    pub agent_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Request failed: {status} - {body}")]
    RequestFailed { status: u16, body: String },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },
    #[error("Hydration I/O error at {path}: {source}")]
    Hydration {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("Auth error: {0}")]
    Auth(String),
    #[error("{0}")]
    EndpointNotAllowed(String),
}
/// Refuse a model request to an endpoint missing from `[endpoints] allowed_endpoints`.
pub(crate) fn allow_endpoint(url: &str) -> Result<(), BackendError> {
    xai_grok_extra_ca::endpoint_allowlist::check(url)
        .map_err(|refusal| BackendError::EndpointNotAllowed(refusal.to_string()))
}
pub struct BackendClient {
    reqwest_client: reqwest::Client,
    client: reqwest_middleware::ClientWithMiddleware,
    base_url: String,
    pub(crate) auth_manager: Option<std::sync::Arc<xai_grok_login::AuthManager>>,
}
impl Default for BackendClient {
    fn default() -> Self {
        Self::new()
    }
}
impl BackendClient {
    fn build_default_client() -> reqwest::Client {
        xai_grok_extra_ca::build_reqwest_client(|builder| {
                builder.connect_timeout(Duration::from_secs(10)).timeout(DEFAULT_TIMEOUT)
            })
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to build backend HTTP client; falling back to shared client");
                crate::http::shared_client()
            })
    }
    pub fn new() -> Self {
        let reqwest_client = Self::build_default_client();
        Self {
            client: reqwest_middleware::ClientBuilder::new(reqwest_client.clone()).build(),
            reqwest_client,
            base_url: std::env::var("GROK_CODE_BACKEND_URL").unwrap_or_default(),
            auth_manager: None,
        }
    }
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let reqwest_client = Self::build_default_client();
        Self {
            client: reqwest_middleware::ClientBuilder::new(reqwest_client.clone()).build(),
            reqwest_client,
            base_url: base_url.into(),
            auth_manager: None,
        }
    }
    /// Attach a live `AuthManager` so every request resolves a fresh token instead of requiring the caller to pass `&GrokAuth`.
    pub(crate) fn with_auth_manager(
        mut self,
        manager: std::sync::Arc<xai_grok_login::AuthManager>,
    ) -> Self {
        let credentials: std::sync::Arc<dyn xai_grok_auth::AuthCredentialProvider> =
            std::sync::Arc::new(
                xai_grok_login::credential_provider::ShellAuthCredentialProvider::new(
                    manager.clone(),
                    None,
                    None,
                ),
            );
        self.client = crate::http::with_auth_retry(self.reqwest_client.clone(), credentials);
        self.auth_manager = Some(manager);
        self
    }
    async fn resolve_auth(&self) -> Result<GrokAuth, BackendError> {
        let manager = self
            .auth_manager
            .as_ref()
            .ok_or_else(|| BackendError::Auth("No AuthManager configured".into()))?;
        manager
            .auth()
            .await
            .map_err(|e| BackendError::Auth(format!("{e}")))
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
    /// The session data (`save_session_data`) is sent inline to the backend.
    /// If the backend responds with 413 (payload too large), the error is logged as a warning and the share continues.
    /// The caller is expected to have already uploaded the data to GCS via a signed URL as a fallback.
    pub async fn share_session(
        &self,
        session: &ExportedSession,
        agent_id: &str,
    ) -> Result<String, BackendError> {
        self.upsert_session(&session.session_id, &session.metadata, agent_id)
            .await?;
        match self
            .save_session_data(
                &session.session_id,
                &session.messages,
                Some(&session.metadata),
            )
            .await
        {
            Ok(()) => {}
            Err(BackendError::RequestFailed { status: 413, .. }) => {
                tracing::warn!(
                    session_id = %session.session_id,
                    "Backend returned 413 for save_session_data; \
                     session data should already be in GCS via signed URL"
                );
            }
            Err(e) => return Err(e),
        }
        let share_response = self.create_share_link(&session.session_id).await?;
        Ok(share_url(&share_response.permission_id))
    }
    /// Must include X-XAI-Token-Auth so nginx auth subrequest routes to OAuth.
    /// See: crates/codegen/xai-grok-shell/src/agent/app.rs:run_headless
    async fn auth_header_map(&self) -> Result<reqwest::header::HeaderMap, BackendError> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let auth = self.resolve_auth().await?;
        let mut headers = HeaderMap::new();
        let required = |value: &str, name: &str| -> Result<HeaderValue, BackendError> {
            HeaderValue::from_str(value)
                .map_err(|e| BackendError::Auth(format!("invalid {name} header: {e}")))
        };
        headers.insert(
            "X-XAI-Token-Auth",
            required(&GrokComConfig::default().token_header, "X-XAI-Token-Auth")?,
        );
        headers.insert("x-userid", required(&auth.user_id, "x-userid")?);
        if let Some(email) = &auth.email
            && let Ok(v) = HeaderValue::from_str(email)
        {
            headers.insert("x-email", v);
        }
        if let Ok(v) = HeaderValue::from_str(&crate::http::process_client_identifier()) {
            headers.insert("x-grok-client-identifier", v);
        }
        headers.insert(
            crate::http::CLIENT_MODE_HEADER,
            HeaderValue::from_static(crate::http::process_client_mode()),
        );
        headers.insert(
            "x-grok-client-version",
            HeaderValue::from_static(xai_grok_version::version()),
        );
        Ok(headers)
    }
    async fn send_with_auth(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, BackendError> {
        let headers = self.auth_header_map().await?;
        let builder = xai_grok_otel::inject_trace_context_into_request(
            builder.timeout(DEFAULT_TIMEOUT).headers(headers),
        );
        let request = builder.build()?;
        self.client.execute(request).await.map_err(|e| match e {
            reqwest_middleware::Error::Reqwest(e) => BackendError::Network(e),
            reqwest_middleware::Error::Middleware(e) => BackendError::Auth(e.to_string()),
        })
    }
    pub async fn upsert_session(
        &self,
        session_id: &str,
        metadata: &ExportedMetadata,
        agent_id: &str,
    ) -> Result<(), BackendError> {
        let url = format!("{}/sessions/{}", self.base_url, session_id);
        let request = UpsertSessionRequest {
            session: SessionUpdate {
                title: metadata.title.clone(),
                cwd: Some(metadata.cwd.clone()),
                status: Some("active".to_string()),
                metadata: serde_json::to_value(metadata).ok(),
            },
            agent_id: agent_id.to_string(),
        };
        let response = self
            .send_with_auth(self.reqwest_client.put(&url).json(&request))
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        Ok(())
    }
    pub(crate) async fn save_session_data(
        &self,
        session_id: &str,
        messages: &[ExportedMessage],
        metadata: Option<&ExportedMetadata>,
    ) -> Result<(), BackendError> {
        let url = format!("{}/sessions/{}/data", self.base_url, session_id);
        let request = SaveDataRequest {
            messages: messages.to_vec(),
            metadata: metadata.and_then(|m| serde_json::to_value(m).ok()),
        };
        let response = self
            .send_with_auth(self.reqwest_client.post(&url).json(&request))
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        Ok(())
    }
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>, BackendError> {
        let url = format!("{}/sessions", self.base_url);
        let response = self.send_with_auth(self.reqwest_client.get(&url)).await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        #[derive(Deserialize)]
        struct ListResponse {
            sessions: Vec<SessionInfo>,
        }
        let data: ListResponse = response.json().await?;
        Ok(data.sessions)
    }
    pub(crate) async fn load_session_data(
        &self,
        session_id: &str,
    ) -> Result<LoadDataResponse, BackendError> {
        let url = format!("{}/sessions/{}/data", self.base_url, session_id);
        let response = self.send_with_auth(self.reqwest_client.get(&url)).await?;
        if response.status().as_u16() == 404 {
            return Err(BackendError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        let data: LoadDataResponse = response.json().await?;
        Ok(data)
    }
    pub(crate) async fn create_share_link(
        &self,
        session_id: &str,
    ) -> Result<ShareResponse, BackendError> {
        let url = format!("{}/sessions/{}/share", self.base_url, session_id);
        let response = self.send_with_auth(self.reqwest_client.post(&url)).await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        let share_response: ShareResponse = response.json().await?;
        Ok(share_response)
    }
    pub(crate) async fn delete_session_data(&self, session_id: &str) -> Result<(), BackendError> {
        let url = format!("{}/sessions/{}/data", self.base_url, session_id);
        let response = self
            .send_with_auth(self.reqwest_client.delete(&url))
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::RequestFailed { status, body });
        }
        Ok(())
    }
}
/// Distinguishes the three cases the external-OTEL gate cares about (see [`crate::agent::mvp_agent`]).
#[derive(Debug, Clone)]
#[must_use]
#[non_exhaustive]
pub enum SettingsFetch {
    /// Settings fetched and parsed; carries the policy that resolves the gate.
    /// Boxed because `RemoteSettings` is large and the other variants are unit-sized.
    Fetched(Box<crate::util::config::RemoteSettings>),
    /// Credential unambiguously rejected (401): the remote policy will never reach this leader, so the gate may open without waiting.
    Rejected,
    /// Transient/ambiguous (network, 5xx exhausted, 403/429/other 4xx, unparseable 2xx): outcome unknown.
    /// A completed fetch that failed: the gate may open on local policy.
    Retry,
}
impl SettingsFetch {
    /// For callers that only want the settings and treat every failure alike.
    pub fn into_option(self) -> Option<crate::util::config::RemoteSettings> {
        match self {
            SettingsFetch::Fetched(s) => Some(*s),
            SettingsFetch::Rejected | SettingsFetch::Retry => None,
        }
    }
}
/// Makes up to [`crate::http::SETTINGS_FETCH_MAX_ATTEMPTS`] attempts on transient failures.
pub fn fetch_settings_blocking(
    cli_chat_proxy_base_url: &str,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
) -> SettingsFetch {
    fetch_settings_blocking_with_attempts(
        cli_chat_proxy_base_url,
        auth,
        alpha_test_key,
        crate::http::SETTINGS_FETCH_MAX_ATTEMPTS,
    )
}
/// Private so the attempt count stays out of the public API; tests use it to skip retry backoff on the transient-failure paths.
fn fetch_settings_blocking_with_attempts(
    cli_chat_proxy_base_url: &str,
    auth: &GrokAuth,
    alpha_test_key: Option<&str>,
    max_attempts: u32,
) -> SettingsFetch {
    let client = crate::http::shared_startup_blocking_client();
    let url = format!("{cli_chat_proxy_base_url}/settings");
    let max_attempts = max_attempts.max(1);
    for attempt in 0u32..max_attempts {
        if attempt > 0 {
            std::thread::sleep(crate::http::SETTINGS_RETRY_BACKOFF_STEP * attempt);
        }
        let request =
            add_cli_chat_proxy_headers_blocking(client.get(&url), auth, alpha_test_key, &url);
        match request.send() {
            Ok(resp) if resp.status().is_success() => match resp.json() {
                Ok(settings) => {
                    tracing::debug!("Fetched remote settings from cli-chat-proxy");
                    return SettingsFetch::Fetched(Box::new(settings));
                }
                Err(e) => {
                    tracing::warn!(attempt, "Failed to parse settings response: {e}");
                    return SettingsFetch::Retry;
                }
            },
            Ok(resp) if resp.status().is_server_error() => {
                tracing::warn!(
                    attempt,
                    status = resp.status().as_u16(),
                    "Settings fetch server error, retrying"
                );
                continue;
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                tracing::warn!(
                    status = resp.status().as_u16(),
                    "Settings fetch rejected (401)"
                );
                return SettingsFetch::Rejected;
            }
            Ok(resp) => {
                tracing::warn!(
                    status = resp.status().as_u16(),
                    "Settings fetch failed (non-2xx)"
                );
                return SettingsFetch::Retry;
            }
            Err(e) => {
                tracing::warn!(attempt, "Settings fetch network error: {e}");
                continue;
            }
        }
    }
    tracing::error!(max_attempts, "Settings fetch failed");
    SettingsFetch::Retry
}
/// Default context window (256k) when the remote endpoint doesn't provide one.
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 256_000;
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<serde_json::Value>,
}
/// The model-listing endpoint for an OpenAI-compatible **or** Anthropic base.
///
/// Both providers serve their model list at the same `/v1/models` shape: an
/// OpenAI-style base `https://api.openai.com/v1` lists at
/// `https://api.openai.com/v1/models`, and an Anthropic base
/// `https://api.anthropic.com/v1` lists at `https://api.anthropic.com/v1/models`.
/// Sharing one resolver (rather than a fixed OpenAI-only assumption) lets a
/// provider base of either kind compute the correct listing URL. A base that
/// already ends in `/models` is returned unchanged.
pub(crate) fn models_list_url_for_base(base: &str) -> String {
    if base.ends_with("/models") {
        base.to_owned()
    } else {
        format!("{base}/models")
    }
}

/// Fetch result: model entries + optional etag from response.
pub struct FetchModelsResult {
    pub models: Vec<crate::agent::config::ModelEntryConfig>,
    pub etag: Option<String>,
}
/// Fetch and parse a `/v1/models` listing from an arbitrary OpenAI-compatible
/// base (BYOK / custom provider, e.g. `https://gateway.pazer.ai/v1`), using a
/// Bearer API key when supplied.
///
/// This is the per-model counterpart to the `model_source` listing: that fetch
/// is driven by `EndpointsConfig` (so it only ever queries the configured
/// xAI proxy or the single `[endpoints].models_base_url`), whereas a BYOK
/// model carries its own `api_base_url` and API key that the generic prefetch
/// never targets. Without this primitive the fork can never auto-detect a
/// BYOK model's real context window (e.g. 1M) when the model's own provider
/// lists it.
pub(crate) fn fetch_models_for_api_base_blocking(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<crate::agent::config::ModelEntryConfig>, BackendError> {
    let url = models_list_url_for_base(base_url.trim_end_matches('/'));
    fetch_models_for_list_url_blocking(&url, api_key)
}

/// Fetch and parse a listing from an exact URL.
///
/// [`fetch_models_for_api_base_blocking`] derives the URL from an inference
/// base. A provider that serves its listing somewhere else
/// (`[model_providers.<id>].models_list_url`) names the URL itself, and
/// deriving one from it would append a second `/models`.
pub(crate) fn fetch_models_for_list_url_blocking(
    url: &str,
    api_key: Option<&str>,
) -> Result<Vec<crate::agent::config::ModelEntryConfig>, BackendError> {
    let client = crate::http::shared_startup_blocking_client();
    // The listing carries no base of its own, so an entry that names none falls
    // back to this. A caller that knows the inference base overwrites it.
    let base_url = url.trim_end_matches("/models").trim_end_matches('/');
    allow_endpoint(url)?;
    tracing::debug!(models_url = %url, "Fetching models for BYOK/custom API base");
    let mut request = client.get(url);
    if let Some(key) = api_key
        && !key.is_empty()
    {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    let response = request.send()?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().unwrap_or_default();
        tracing::warn!("Failed to fetch models for base: {} - {}", status, body);
        return Err(BackendError::RequestFailed { status, body });
    }
    let models_response: ModelsResponse = response.json()?;
    let mut models = Vec::with_capacity(models_response.data.len());
    for (idx, value) in models_response.data.into_iter().enumerate() {
        match parse_remote_model_value(&value, base_url) {
            Some(model) => models.push(model),
            None => {
                tracing::warn!(
                    "Skipping model at index {} (BYOK base): missing required field ('model' or 'context_window') or invalid types",
                    idx
                )
            }
        }
    }
    Ok(models)
}

/// Parse a single model entry from the /models-v2 response.
/// Used by both initial model fetch and session-resume metadata refresh.
pub(crate) fn parse_remote_model_value(
    value: &serde_json::Value,
    default_base_url: &str,
) -> Option<crate::agent::config::ModelEntryConfig> {
    let obj = value.as_object()?;
    let meta = obj.get("_meta").and_then(|v| v.as_object());
    let id = get_string(obj, "id");
    let model = get_string(obj, "model")
        .or_else(|| get_string(obj, "modelId"))
        .or_else(|| id.clone())
        .or_else(|| meta.and_then(|m| get_string(m, "model")))
        .or_else(|| meta.and_then(|m| get_string(m, "modelId")))?;
    let model_family = get_string(obj, "modelFamily")
        .or_else(|| get_string(obj, "model_family"))
        .or_else(|| meta.and_then(|m| get_string(m, "modelFamily")))
        .or_else(|| meta.and_then(|m| get_string(m, "model_family")));
    let base_url = get_string(obj, "baseUrl")
        .or_else(|| get_string(obj, "base_url"))
        .unwrap_or_else(|| default_base_url.to_owned());
    let name = get_string(obj, "name").or_else(|| Some(model.clone()));
    // Anthropic exposes its per-model input-context-window in the `/v1/models`
    // listing as `max_input_tokens` ("Maximum input context window size in
    // tokens for this model"), NOT `contextWindow`/`context_window`. Read it as
    // a last-resort fallback so an Anthropic-style listing still resolves a
    // window. A `0` (e.g. docs placeholder) is ignored so the non-zero entry
    // isn't dropped; a positive value wins only when no OpenAI-style field is
    // present.
    // OpenRouter exposes the per-model context window in `/models` as
    // `context_length` (top-level, and mirrored under `top_provider`), e.g.
    // `{"id":"deepseek/deepseek-v4-pro-0813","context_length":1000000,
    //   "top_provider":{"context_length":1000000}}`. Read it after the
    // OpenAI-style fields but before the Anthropic `max_input_tokens` fallback,
    // so a listing that emits only `context_length` still resolves a real
    // window instead of `DEFAULT_CONTEXT_WINDOW`.
    let top_provider = obj.get("top_provider").and_then(|v| v.as_object());
    let context_window = get_u64(obj, "contextWindow")
        .or_else(|| get_u64(obj, "context_window"))
        .or_else(|| meta.and_then(|m| get_u64(m, "contextWindow")))
        .or_else(|| meta.and_then(|m| get_u64(m, "totalContextTokens")))
        .or_else(|| get_u64(obj, "context_length"))
        .or_else(|| top_provider.and_then(|tp| get_u64(tp, "context_length")))
        .or_else(|| get_u64(obj, "max_model_len"))
        .or_else(|| get_u64(obj, "max_input_tokens"))
        .or_else(|| get_u64(obj, "maxInputTokens"))
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);
    let context_window = std::num::NonZeroU64::new(context_window)?;
    let agent_type = get_string(obj, "systemPromptType")
        .or_else(|| get_string(obj, "system_prompt_type"))
        .or_else(|| get_string(obj, "agent_type"))
        .or_else(|| get_string(obj, "agentType"))
        .or_else(|| meta.and_then(|m| get_string(m, "agentType")))
        .or_else(|| meta.and_then(|m| get_string(m, "agent_type")))
        .unwrap_or_else(crate::agent::config::default_agent_type);
    let api_backend = get_string(obj, "apiBackend")
        .or_else(|| get_string(obj, "api_backend"))
        .and_then(|s| match s.as_str() {
            "responses" => Some(crate::sampling::ApiBackend::Responses),
            "chat_completions" => Some(crate::sampling::ApiBackend::ChatCompletions),
            "messages" => Some(crate::sampling::ApiBackend::Messages),
            "ollama" => Some(crate::sampling::ApiBackend::Ollama),
            _ => None,
        })
        .unwrap_or_default();
    let (reasoning_efforts, reasoning_effort_server_default) = obj
        .get("reasoningEfforts")
        .or_else(|| obj.get("reasoning_efforts"))
        .or_else(|| meta.and_then(|m| m.get("reasoningEfforts")))
        .and_then(|v| v.as_array())
        .map(|arr| xai_grok_sampling_types::parse_reasoning_effort_options(arr, "reasoningEfforts"))
        .filter(|options| !options.is_empty())
        .map(|options| (options, false))
        .or_else(|| {
            obj.get("capabilities")
                .and_then(|v| v.as_object())
                .and_then(parse_capabilities_reasoning_efforts)
        })
        .unwrap_or_default();
    Some(crate::agent::config::ModelEntryConfig {
        id,
        model,
        model_family,
        base_url,
        name,
        description: get_string(obj, "description"),
        max_completion_tokens: get_u64(obj, "maxCompletionTokens")
            .or_else(|| get_u64(obj, "max_completion_tokens"))
            .and_then(|v| u32::try_from(v).ok()),
        temperature: get_f64(obj, "temperature").map(|v| v as f32),
        top_p: get_f64(obj, "topP").or_else(|| get_f64(obj, "top_p")).map(|v| v as f32),
        api_key: get_string(obj, "apiKey").or_else(|| get_string(obj, "api_key")),
        env_key: get_env_keys(obj, "envKey").or_else(|| get_env_keys(obj, "env_key")),
        api_backend,
        context_window,
        max_request_bytes: get_u64(obj, "maxRequestBytes")
            .or_else(|| get_u64(obj, "max_request_bytes"))
            .and_then(std::num::NonZeroU64::new),
        auto_compact_threshold_percent: get_u64(obj, "autoCompactThresholdPercent")
            .or_else(|| get_u64(obj, "auto_compact_threshold_percent"))
            .and_then(|v| u8::try_from(v).ok()),
        system_prompt_label: get_string(obj, "systemPromptLabel")
            .or_else(|| get_string(obj, "system_prompt_label"))
            .filter(|s| !s.trim().is_empty()),
        extra_headers: get_string_map(obj, "extraHeaders"),
        api_base_url: get_string(obj, "apiBaseUrl")
            .or_else(|| get_string(obj, "api_base_url")),
        use_concise: obj
            .get("useConcise")
            .or_else(|| obj.get("use_concise"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        agent_type,
        loaded_in_vram: None,
        inference_idle_timeout_secs: get_u64(obj, "inferenceIdleTimeoutSecs")
            .or_else(|| get_u64(obj, "inference_idle_timeout_secs")),
        max_retries: get_u64(obj, "maxRetries")
            .or_else(|| get_u64(obj, "max_retries"))
            .and_then(|v| u32::try_from(v).ok()),
        rate_limit_retry_threshold: get_u64(obj, "rateLimitRetryThreshold")
            .or_else(|| get_u64(obj, "rate_limit_retry_threshold"))
            .and_then(|v| u32::try_from(v).ok()),
        subagent_rate_limit_max_attempts: get_u64(obj, "subagentRateLimitMaxAttempts")
            .or_else(|| get_u64(obj, "subagent_rate_limit_max_attempts"))
            .and_then(|v| u32::try_from(v).ok()),
        hidden: obj
            .get("hidden")
            .or_else(|| meta.and_then(|m| m.get("hidden")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        supported_in_api: obj
            .get("supportedInApi")
            .or_else(|| obj.get("supported_in_api"))
            .or_else(|| meta.and_then(|m| m.get("supportedInApi")))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        auth_scheme: None,
        reasoning_effort: get_string(obj, "reasoningEffort")
            .or_else(|| get_string(obj, "reasoning_effort"))
            .or_else(|| meta.and_then(|m| get_string(m, "reasoningEffort")))
            .and_then(|s| s.parse().ok()),
        supports_reasoning_effort: obj
            .get("supportsReasoningEffort")
            .or_else(|| obj.get("supports_reasoning_effort"))
            .or_else(|| meta.and_then(|m| m.get("supportsReasoningEffort")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        reasoning_efforts,
        reasoning_effort_server_default,
        variants: obj
            .get("variants")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr
                    .iter()
                    .filter_map(|value| match serde_json::from_value(value.clone()) {
                        Ok(variant) => Some(variant),
                        Err(e) => {
                            tracing::warn!("Dropping a model variant this build cannot read: {e}");
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        supports_backend_search: obj
            .get("supportsBackendSearch")
            .or_else(|| obj.get("supports_backend_search"))
            .or_else(|| meta.and_then(|m| m.get("supportsBackendSearch")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        compactions_remaining: obj
            .get("compactionsRemaining")
            .or_else(|| obj.get("compactions_remaining"))
            .or_else(|| meta.and_then(|m| m.get("compactionsRemaining")))
            .and_then(parse_compactions_remaining)
            .or_else(|| {
                obj
                    .get("sendCompactionsRemaining")
                    .or_else(|| obj.get("send_compactions_remaining"))
                    .or_else(|| meta.and_then(|m| m.get("sendCompactionsRemaining")))
                    .and_then(|v| v.as_bool())
                    .map(xai_grok_sampling_types::CompactionsRemaining::Dynamic)
            }),
        compaction_at_tokens: obj
            .get("compactionAtTokens")
            .or_else(|| obj.get("compaction_at_tokens"))
            .or_else(|| meta.and_then(|m| m.get("compactionAtTokens")))
            .and_then(parse_compaction_at_tokens),
        show_model_fingerprint: obj
            .get("showModelFingerprint")
            .or_else(|| obj.get("show_model_fingerprint"))
            .or_else(|| meta.and_then(|m| m.get("showModelFingerprint")))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        stream_tool_calls: obj
            .get("streamToolCalls")
            .or_else(|| obj.get("stream_tool_calls"))
            .and_then(|v| v.as_bool()),
        strict_message_schema: obj
            .get("strictMessageSchema")
            .or_else(|| obj.get("strict_message_schema"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        reasoning_summary: obj
            .get("reasoningSummary")
            .or_else(|| obj.get("reasoning_summary"))
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        laziness_detector: get_object(obj, "lazinessDetector")
            .or_else(|| get_object(obj, "laziness_detector"))
            .or_else(|| meta.and_then(|m| get_object(m, "lazinessDetector")))
            .and_then(|v| match serde_json::from_value::<
                crate::agent::config::LazinessDetectorPerModelConfig,
            >(v.clone()) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    tracing::warn!(
                            error = %e,
                            "Failed to deserialize laziness_detector block from remote model; falling back to default"
                        );
                    None
                }
            })
            .unwrap_or_default(),
        pricing: xai_grok_sampling_types::ModelPricing::default(),
        min_output_tokens_per_sec: None, ttft_timeout_secs: None,
    })
}
fn get_string(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}
/// Effort menu from a `/v1/models` `capabilities` block: `reasoning_effort` is a bare-string list, `default_reasoning_effort` names the marked entry.
/// The second value is true when no entry got marked, so the request omits the effort and the server applies its own.
fn parse_capabilities_reasoning_efforts(
    caps: &serde_json::Map<String, serde_json::Value>,
) -> Option<(Vec<xai_grok_sampling_types::ReasoningEffortOption>, bool)> {
    let arr = caps.get("reasoning_effort")?.as_array()?;
    let mut options = xai_grok_sampling_types::parse_reasoning_effort_options(
        arr,
        "capabilities.reasoning_effort",
    );
    let mut marked = false;
    if let Some(default) = caps
        .get("default_reasoning_effort")
        .filter(|v| !v.is_null())
    {
        let level = default
            .as_str()
            .and_then(|s| s.parse::<xai_grok_sampling_types::ReasoningEffort>().ok());
        for option in &mut options {
            option.default = level == Some(option.value);
            marked |= option.default;
        }
        if !marked {
            tracing::warn!(
                value = %default,
                "capabilities.default_reasoning_effort is not a listed level; leaving the server default"
            );
        }
    }
    Some((options, !marked))
}
/// Parse `env_key` / `envKey` as a single string or a string array.
fn get_env_keys(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<crate::agent::config::EnvKeys> {
    let v = obj.get(key)?;
    if let Some(s) = v.as_str() {
        return Some(crate::agent::config::EnvKeys::single(s));
    }
    if let Some(arr) = v.as_array() {
        let mut names = Vec::with_capacity(arr.len());
        for item in arr {
            let Some(s) = item.as_str() else {
                tracing::warn!(
                    key,
                    "env_key array has a non-string element; ignoring env_key"
                );
                return None;
            };
            if !s.is_empty() {
                names.push(s.to_owned());
            }
        }
        if names.is_empty() {
            return None;
        }
        return Some(crate::agent::config::EnvKeys::new(names));
    }
    None
}
fn parse_compaction_at_tokens(
    v: &serde_json::Value,
) -> Option<xai_grok_sampling_types::CompactionAtTokens> {
    use xai_grok_sampling_types::CompactionAtTokens;
    v.as_bool()
        .map(CompactionAtTokens::Enabled)
        .or_else(|| v.as_u64().map(CompactionAtTokens::Fixed))
}
fn parse_compactions_remaining(
    v: &serde_json::Value,
) -> Option<xai_grok_sampling_types::CompactionsRemaining> {
    use xai_grok_sampling_types::CompactionsRemaining;
    v.as_bool().map(CompactionsRemaining::Dynamic).or_else(|| {
        v.as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(CompactionsRemaining::Fixed)
    })
}
fn get_u64(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(|v| v.as_u64())
}
fn get_f64(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| v.as_f64())
}
fn get_object<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    obj.get(key).filter(|v| v.is_object())
}
fn get_string_map(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> IndexMap<String, String> {
    obj.get(key)
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}
#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
