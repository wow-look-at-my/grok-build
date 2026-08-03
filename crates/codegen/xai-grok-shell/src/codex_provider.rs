//! Additive ChatGPT/Codex provider support.
//!
//! The official `codex app-server` owns OAuth, refresh-token rotation, and
//! model discovery. Grok Build only reads the short-lived access token that
//! app-server writes to its mode-0600 `auth.json`, keeps it in memory, and
//! routes the discovered models through the existing Responses API sampler.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::agent::config::{ModelEntry, ModelInfo};
use xai_grok_sampler::{AuthScheme, BearerResolver, HeaderInjector};
use xai_grok_sampling_types::{ApiBackend, ReasoningEffort, ReasoningEffortOption};

pub const AUTH_METHOD_ID: &str = "openai.codex";
pub const MODEL_ID_PREFIX: &str = "codex/";
pub const BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

const MANAGED_BEARER_PLACEHOLDER: &str = "codex-managed-by-app-server";
const RPC_TIMEOUT: Duration = Duration::from_secs(15);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
struct Credentials {
    access_token: String,
    account_id: String,
    installation_id: Option<String>,
}

static CREDENTIALS: OnceLock<RwLock<Option<Credentials>>> = OnceLock::new();
static RPC_GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn credentials_cell() -> &'static RwLock<Option<Credentials>> {
    CREDENTIALS.get_or_init(|| RwLock::new(None))
}

fn rpc_gate() -> &'static tokio::sync::Mutex<()> {
    RPC_GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub fn auth_method() -> agent_client_protocol::AuthMethod {
    use agent_client_protocol as acp;
    acp::AuthMethod::Agent(
        acp::AuthMethodAgent::new(
            acp::AuthMethodId::new(AUTH_METHOD_ID),
            "Codex (ChatGPT)".to_string(),
        )
        .description(Some(
            "Sign in with Codex and add your ChatGPT Codex models".to_string(),
        )),
    )
}

pub fn is_codex_backend(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("chatgpt.com")
        && url.path().trim_end_matches('/') == "/backend-api/codex"
}

/// Discover models for an existing Codex login.
///
/// `Ok(None)` is the normal signed-out state. Startup remains fully usable
/// with every other configured provider.
pub async fn discover_existing_models() -> anyhow::Result<Option<IndexMap<String, ModelEntry>>> {
    let _guard = rpc_gate().lock().await;
    let mut rpc = CodexRpc::start().await?;
    let account = rpc
        .request(
            "account/read",
            json!({ "refreshToken": false }),
            RPC_TIMEOUT,
        )
        .await?;
    if !is_chatgpt_account(&account) {
        clear_credentials();
        rpc.shutdown().await;
        return Ok(None);
    }
    install_credentials(load_credentials()?);
    let models = fetch_all_models(&mut rpc).await?;
    rpc.shutdown().await;
    Ok(Some(models))
}

/// Refresh the official Codex auth snapshot before a Codex-backed turn.
///
/// Near expiry, `account/read` with `refreshToken` delegates rotation to
/// app-server. If app-server is temporarily unavailable, the last in-memory
/// token remains installed so an otherwise-fresh session does not get
/// destroyed.
pub async fn refresh_credentials() -> anyhow::Result<()> {
    let token_is_fresh = credentials_cell()
        .read()
        .ok()
        .and_then(|credentials| credentials.as_ref().map(|c| c.access_token.clone()))
        .is_some_and(|token| access_token_is_fresh(&token));
    if token_is_fresh {
        return Ok(());
    }

    let _guard = rpc_gate().lock().await;
    let mut rpc = CodexRpc::start().await?;
    let account = rpc
        .request("account/read", json!({ "refreshToken": true }), RPC_TIMEOUT)
        .await?;
    rpc.shutdown().await;
    if !is_chatgpt_account(&account) {
        clear_credentials();
        bail!("Codex is not signed in with ChatGPT");
    }
    install_credentials(load_credentials()?);
    Ok(())
}

fn access_token_is_fresh(token: &str) -> bool {
    const REFRESH_THRESHOLD: chrono::Duration = chrono::Duration::minutes(5);
    crate::auth::parse_jwt_expiration(token)
        .is_some_and(|expires_at| expires_at > chrono::Utc::now() + REFRESH_THRESHOLD)
}

/// Run the browser OAuth flow and return the newly-authorized model catalog.
pub async fn login(
    url_tx: tokio::sync::oneshot::Sender<crate::auth::AuthUrlInfo>,
) -> anyhow::Result<IndexMap<String, ModelEntry>> {
    let _guard = rpc_gate().lock().await;
    let mut rpc = CodexRpc::start().await?;
    let response = rpc
        .request(
            "account/login/start",
            json!({
                "type": "chatgpt",
                "useHostedLoginSuccessPage": true,
                "appBrand": "codex"
            }),
            RPC_TIMEOUT,
        )
        .await?;
    let result = response
        .get("result")
        .context("Codex login response did not contain a result")?;
    let login_id = result
        .get("loginId")
        .and_then(Value::as_str)
        .context("Codex login response did not contain a login id")?
        .to_string();
    let auth_url = result
        .get("authUrl")
        .and_then(Value::as_str)
        .context("Codex login response did not contain an auth URL")?
        .to_string();

    let _ = url_tx.send(crate::auth::AuthUrlInfo {
        url: auth_url.clone(),
        mode: crate::auth::AuthUrlMode::Loopback,
    });
    if let Err(error) = webbrowser::open(&auth_url) {
        tracing::debug!(%error, "could not open Codex login URL automatically");
    }

    rpc.wait_for_login(&login_id).await?;
    install_credentials(load_credentials()?);
    let models = fetch_all_models(&mut rpc).await?;
    rpc.shutdown().await;
    Ok(models)
}

pub fn clear_credentials() {
    if let Ok(mut slot) = credentials_cell().write() {
        *slot = None;
    }
}

fn install_credentials(credentials: Credentials) {
    if let Ok(mut slot) = credentials_cell().write() {
        *slot = Some(credentials);
    }
}

#[derive(Debug)]
pub struct CodexBearerResolver;

impl BearerResolver for CodexBearerResolver {
    fn current_bearer(&self) -> Option<String> {
        credentials_cell()
            .read()
            .ok()
            .and_then(|credentials| credentials.as_ref().map(|c| c.access_token.clone()))
    }
}

pub struct CodexHeaderInjector {
    session_id: String,
}

impl CodexHeaderInjector {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }
}

impl std::fmt::Debug for CodexHeaderInjector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexHeaderInjector")
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl HeaderInjector for CodexHeaderInjector {
    fn inject(&self, headers: &mut reqwest::header::HeaderMap) {
        use reqwest::header::{HeaderName, HeaderValue};

        let credentials = credentials_cell()
            .read()
            .ok()
            .and_then(|credentials| credentials.clone());
        let Some(credentials) = credentials else {
            return;
        };
        if let Ok(value) = HeaderValue::from_str(&credentials.account_id) {
            headers.insert(HeaderName::from_static("chatgpt-account-id"), value);
        }
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_static("grok_build"),
        );
        if let Ok(value) = HeaderValue::from_str(&self.session_id) {
            headers.insert(HeaderName::from_static("session-id"), value.clone());
            headers.insert(HeaderName::from_static("thread-id"), value.clone());
            headers.insert(HeaderName::from_static("x-client-request-id"), value);
        }
        if let Some(installation_id) = credentials.installation_id
            && let Ok(value) = HeaderValue::from_str(&installation_id)
        {
            headers.insert(HeaderName::from_static("x-codex-installation-id"), value);
        }
        if let Some(traceparent) = xai_file_utils::trace_context::current_traceparent()
            && let Ok(value) = HeaderValue::from_str(&traceparent)
        {
            headers.insert(HeaderName::from_static("traceparent"), value);
        }
    }
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<AuthTokens>,
}

#[derive(Deserialize)]
struct AuthTokens {
    access_token: String,
    account_id: Option<String>,
}

fn load_credentials() -> anyhow::Result<Credentials> {
    let home = codex_home();
    let path = home.join("auth.json");
    validate_auth_file(&path)?;
    let file =
        std::fs::File::open(&path).with_context(|| format!("could not read {}", path.display()))?;
    let auth: AuthFile = serde_json::from_reader(std::io::BufReader::new(file))
        .with_context(|| format!("could not parse {}", path.display()))?;
    let tokens = auth
        .tokens
        .context("Codex ChatGPT credentials are unavailable; run `/login codex` to sign in")?;
    if tokens.access_token.trim().is_empty() {
        bail!("Codex access token is empty");
    }
    let account_id = tokens
        .account_id
        .filter(|id| !id.trim().is_empty())
        .context("Codex credentials do not contain a ChatGPT account id")?;
    let installation_id = std::fs::read_to_string(home.join("installation_id"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(Credentials {
        access_token: tokens.access_token,
        account_id,
        installation_id,
    })
}

fn validate_auth_file(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Codex auth file is unavailable at {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("Codex auth path is not a regular file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "Codex auth file permissions are too broad; expected mode 0600 at {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn codex_command() -> std::ffi::OsString {
    std::env::var_os("GROK_CODEX_COMMAND").unwrap_or_else(|| "codex".into())
}

fn is_chatgpt_account(response: &Value) -> bool {
    response
        .pointer("/result/account/type")
        .and_then(Value::as_str)
        == Some("chatgpt")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListResult {
    data: Vec<CodexModel>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModel {
    id: String,
    model: String,
    display_name: String,
    description: String,
    hidden: bool,
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    default_reasoning_effort: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffort {
    reasoning_effort: String,
    description: String,
}

async fn fetch_all_models(rpc: &mut CodexRpc) -> anyhow::Result<IndexMap<String, ModelEntry>> {
    let mut cursor: Option<String> = None;
    let mut catalog = IndexMap::new();
    loop {
        let response = rpc
            .request(
                "model/list",
                json!({
                    "cursor": cursor,
                    "limit": 100,
                    "includeHidden": false
                }),
                RPC_TIMEOUT,
            )
            .await?;
        let result: ModelListResult = serde_json::from_value(
            response
                .get("result")
                .cloned()
                .context("Codex model/list response did not contain a result")?,
        )
        .context("Codex model/list response had an unsupported shape")?;
        for model in result.data {
            if model.hidden {
                continue;
            }
            let key = format!("{MODEL_ID_PREFIX}{}", model.id);
            catalog.insert(key.clone(), model_entry(key, model));
        }
        cursor = result.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(catalog)
}

fn model_entry(key: String, model: CodexModel) -> ModelEntry {
    let default_effort = parse_effort(&model.default_reasoning_effort);
    let reasoning_efforts = model
        .supported_reasoning_efforts
        .into_iter()
        .filter_map(|effort| {
            let value = parse_effort(&effort.reasoning_effort)?;
            Some(ReasoningEffortOption {
                id: effort.reasoning_effort.clone(),
                value,
                label: humanize(&effort.reasoning_effort),
                description: Some(effort.description),
                default: default_effort == Some(value),
            })
        })
        .collect::<Vec<_>>();
    let mut info = ModelInfo::fallback(&model.model);
    info.id = Some(key);
    info.base_url = BACKEND_BASE_URL.to_string();
    info.name = Some(format!("Codex · {}", model.display_name));
    info.description = Some(if model.description.trim().is_empty() {
        "Available through your Codex sign-in".to_string()
    } else {
        format!("{} (Codex)", model.description)
    });
    info.api_backend = ApiBackend::Responses;
    info.auth_scheme = AuthScheme::Bearer;
    info.context_window = std::num::NonZeroU64::new(256_000).expect("non-zero");
    info.supports_reasoning_effort = !reasoning_efforts.is_empty();
    info.reasoning_effort = default_effort;
    info.reasoning_efforts = reasoning_efforts;
    info.supported_in_api = true;
    info.hidden = false;
    ModelEntry {
        info,
        // Marks this entry as provider-owned so credential resolution never
        // inherits an xAI session token. The live resolver replaces it before
        // every HTTP request.
        api_key: Some(MANAGED_BEARER_PLACEHOLDER.to_string()),
        env_key: None,
        auth_provider: None,
        api_base_url: None,
    }
}

fn parse_effort(value: &str) -> Option<ReasoningEffort> {
    match value {
        "none" => Some(ReasoningEffort::None),
        "minimal" => Some(ReasoningEffort::Minimal),
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        "high" => Some(ReasoningEffort::High),
        "xhigh" => Some(ReasoningEffort::Xhigh),
        // Grok's sampler does not yet have distinct wire variants for the
        // newer Codex-only max/ultra settings. Do not mislabel them as xhigh.
        _ => None,
    }
}

fn humanize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

struct CodexRpc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl CodexRpc {
    async fn start() -> anyhow::Result<Self> {
        let mut child = Command::new(codex_command())
            .arg("app-server")
            .arg("--stdio")
            .arg("-c")
            .arg("cli_auth_credentials_store=\"file\"")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context(
                "could not start `codex app-server`; install the Codex CLI or set GROK_CODEX_COMMAND",
            )?;
        let stdin = child
            .stdin
            .take()
            .context("Codex app-server has no stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex app-server has no stdout")?;
        let mut rpc = Self {
            child,
            stdin: Some(stdin),
            lines: BufReader::new(stdout).lines(),
            next_id: 1,
        };
        rpc.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "grok_build",
                    "title": "Grok Build",
                    "version": xai_grok_version::VERSION
                }
            }),
            RPC_TIMEOUT,
        )
        .await?;
        rpc.send(json!({ "method": "initialized", "params": {} }))
            .await?;
        Ok(rpc)
    }

    async fn send(&mut self, message: Value) -> anyhow::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("Codex app-server stdin is closed")?;
        let mut encoded = serde_json::to_vec(&message)?;
        encoded.push(b'\n');
        stdin.write_all(&encoded).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "method": method, "id": id, "params": params }))
            .await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let message = self.read_before(deadline).await?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown app-server error");
                bail!("Codex app-server rejected {method}: {text}");
            }
            return Ok(message);
        }
    }

    async fn wait_for_login(&mut self, login_id: &str) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
        loop {
            let message = self.read_before(deadline).await?;
            if message.get("method").and_then(Value::as_str) != Some("account/login/completed") {
                continue;
            }
            let params = message
                .get("params")
                .context("login completion has no params")?;
            if params.get("loginId").and_then(Value::as_str) != Some(login_id) {
                continue;
            }
            if params
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            let error = params
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Codex sign-in was not completed");
            bail!("{error}");
        }
    }

    async fn read_before(&mut self, deadline: tokio::time::Instant) -> anyhow::Result<Value> {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| anyhow!("timed out waiting for Codex app-server"))?;
        let line = tokio::time::timeout(remaining, self.lines.next_line())
            .await
            .map_err(|_| anyhow!("timed out waiting for Codex app-server"))??
            .context("Codex app-server closed unexpectedly")?;
        serde_json::from_str(&line).context("Codex app-server emitted invalid JSON")
    }

    async fn shutdown(&mut self) {
        self.stdin.take();
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_model() -> CodexModel {
        CodexModel {
            id: "gpt-test".into(),
            model: "gpt-test".into(),
            display_name: "GPT Test".into(),
            description: "Test model".into(),
            hidden: false,
            supported_reasoning_efforts: vec![
                CodexReasoningEffort {
                    reasoning_effort: "low".into(),
                    description: "Fast".into(),
                },
                CodexReasoningEffort {
                    reasoning_effort: "ultra".into(),
                    description: "Delegates".into(),
                },
            ],
            default_reasoning_effort: "low".into(),
        }
    }

    #[test]
    fn model_ids_are_provider_qualified_and_route_original_slug() {
        let entry = model_entry("codex/gpt-test".into(), raw_model());
        assert_eq!(entry.info.id.as_deref(), Some("codex/gpt-test"));
        assert_eq!(entry.info.model, "gpt-test");
        assert_eq!(entry.info.name.as_deref(), Some("Codex · GPT Test"));
        assert_eq!(entry.info.base_url, BACKEND_BASE_URL);
        assert_eq!(entry.info.api_backend, ApiBackend::Responses);
        assert_eq!(entry.info.auth_scheme, AuthScheme::Bearer);
        assert!(entry.has_own_credentials());
    }

    #[test]
    fn unsupported_codex_efforts_are_not_misrepresented() {
        let entry = model_entry("codex/gpt-test".into(), raw_model());
        assert_eq!(entry.info.reasoning_efforts.len(), 1);
        assert_eq!(entry.info.reasoning_efforts[0].id, "low");
        assert!(entry.info.reasoning_efforts[0].default);
    }

    #[test]
    fn codex_backend_match_is_host_and_path_scoped() {
        assert!(is_codex_backend(BACKEND_BASE_URL));
        assert!(is_codex_backend("https://chatgpt.com/backend-api/codex/"));
        assert!(!is_codex_backend("https://example.com/backend-api/codex"));
        assert!(!is_codex_backend("http://chatgpt.com/backend-api/codex"));
    }

    #[test]
    fn codex_headers_are_provider_scoped_and_never_contain_the_token() {
        install_credentials(Credentials {
            access_token: "secret-token".into(),
            account_id: "account-123".into(),
            installation_id: Some("install-123".into()),
        });
        let mut headers = reqwest::header::HeaderMap::new();
        CodexHeaderInjector::new("session-123").inject(&mut headers);
        clear_credentials();

        assert_eq!(headers["chatgpt-account-id"], "account-123");
        assert_eq!(headers["originator"], "grok_build");
        assert_eq!(headers["session-id"], "session-123");
        assert_eq!(headers["thread-id"], "session-123");
        assert_eq!(headers["x-client-request-id"], "session-123");
        assert_eq!(headers["x-codex-installation-id"], "install-123");
        assert!(
            headers
                .values()
                .all(|value| value.as_bytes() != b"secret-token")
        );
    }

    #[test]
    fn opaque_tokens_are_refreshed_instead_of_assumed_fresh() {
        assert!(!access_token_is_fresh("opaque-access-token"));
    }
}
