//! The `external_account` (workload identity federation) token exchange over
//! `xai_grok_net`.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use base64::Engine as _;
use gcloud_storage::client::google_cloud_auth::credentials::CredentialsFile;
use hmac::Mac;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};
use xai_grok_net::header::{self, HeaderMap, HeaderName, HeaderValue};
use xai_grok_net::{Auth, HttpClient, Url};

use crate::gcs_net::{TOKEN_TIMEOUT, check_status, read_token};

const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const ALLOW_EXECUTABLES_ENV: &str = "GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES";
const DEFAULT_EXECUTABLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_URL_SUBJECT_TOKEN: usize = 1 << 20;

#[derive(serde::Deserialize, Default)]
struct ExternalFields {
    credential_source: Option<CredentialSource>,
    service_account_impersonation: Option<ImpersonationInfo>,
}

#[derive(serde::Deserialize, Clone, Default)]
pub(crate) struct CredentialSource {
    file: Option<String>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
    executable: Option<Executable>,
    environment_id: Option<String>,
    region_url: Option<String>,
    regional_cred_verification_url: Option<String>,
    imdsv2_session_token_url: Option<String>,
    format: Option<Format>,
}

#[derive(serde::Deserialize, Clone, Default)]
struct Format {
    #[serde(rename = "type")]
    tp: Option<String>,
    subject_token_field_name: Option<String>,
}

#[derive(serde::Deserialize, Clone)]
struct Executable {
    command: String,
    timeout_millis: Option<u64>,
    output_file: Option<String>,
}

#[derive(serde::Deserialize)]
struct ImpersonationInfo {
    token_lifetime_seconds: Option<i64>,
}

/// The access token for an `external_account` credential. `raw_json` is the file `cred`
/// was parsed from. `scopes` is space-separated.
pub(crate) async fn external_account_token(
    client: &HttpClient,
    cred: &CredentialsFile,
    raw_json: &str,
    scopes: &str,
) -> anyhow::Result<String> {
    let fields: ExternalFields =
        serde_json::from_str(raw_json).context("parse the external_account credential")?;
    let source = fields
        .credential_source
        .context("external_account credential has no credential_source")?;
    let subject_token_type = cred
        .subject_token_type
        .as_deref()
        .context("external_account credential has no subject_token_type")?;
    let token_url = cred
        .token_url_external
        .as_deref()
        .context("external_account credential has no token_url")?;
    let audience = cred.audience.as_deref().unwrap_or_default();
    let impersonation_url = cred.service_account_impersonation_url.as_deref();

    let subject = subject_token(
        client,
        &source,
        audience,
        subject_token_type,
        impersonation_url,
    )
    .await
    .context("external_account subject token")?;

    let sts_auth = match (&cred.client_id, &cred.client_secret) {
        (Some(id), Some(secret)) => {
            let basic = base64::engine::general_purpose::STANDARD.encode(format!("{id}:{secret}"));
            configured_authorization(&format!("Basic {basic}"))?
        }
        _ => Auth::None,
    };
    let response = client
        .post(token_url, sts_auth)
        .timeout(TOKEN_TIMEOUT)
        .form(&[
            ("grant_type", TOKEN_EXCHANGE_GRANT),
            ("audience", audience),
            ("scope", scopes),
            ("subject_token_type", subject_token_type),
            ("subject_token", subject.as_str()),
            ("requested_token_type", ACCESS_TOKEN_TYPE),
        ])
        .send()
        .await?;
    let sts_token = read_token(response).await.context("STS token exchange")?;

    let Some(url) = impersonation_url else {
        return Ok(sts_token);
    };
    let lifetime = fields
        .service_account_impersonation
        .and_then(|i| i.token_lifetime_seconds)
        .unwrap_or(3600);
    impersonate(client, url, &sts_token, scopes, lifetime).await
}

fn configured_authorization(value: &str) -> anyhow::Result<Auth> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(value).context("credential is not a header value")?,
    );
    Ok(Auth::Configured(headers))
}

#[derive(serde::Serialize)]
struct ImpersonateRequest<'a> {
    delegates: Vec<String>,
    lifetime: String,
    scope: Vec<&'a str>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImpersonateResponse {
    access_token: String,
}

/// Trade the STS token for a service account token (`generateAccessToken`).
async fn impersonate(
    client: &HttpClient,
    url: &str,
    sts_token: &str,
    scopes: &str,
    lifetime_seconds: i64,
) -> anyhow::Result<String> {
    let mut scope: Vec<&str> = scopes.split(' ').filter(|s| !s.is_empty()).collect();
    scope.push(CLOUD_PLATFORM_SCOPE);
    let body = ImpersonateRequest {
        delegates: Vec::new(),
        lifetime: format!("{lifetime_seconds}s"),
        scope,
    };
    let response = client
        .post(
            url,
            configured_authorization(&format!("Bearer {sts_token}"))?,
        )
        .timeout(TOKEN_TIMEOUT)
        .json(&body)
        .send()
        .await?;
    let response: ImpersonateResponse = check_status(response)
        .await
        .context("service account impersonation")?
        .json()
        .await?;
    Ok(response.access_token)
}

/// Pick the subject token source in the order `gcloud-auth` uses: AWS, URL, file. An
/// executable comes last.
async fn subject_token(
    client: &HttpClient,
    source: &CredentialSource,
    audience: &str,
    subject_token_type: &str,
    impersonation_url: Option<&str>,
) -> anyhow::Result<String> {
    let environment_id = source.environment_id.as_deref().unwrap_or_default();
    if environment_id.len() > 3 && environment_id.starts_with("aws") {
        anyhow::ensure!(
            environment_id == "aws1",
            "unsupported AWS environment_id `{environment_id}`"
        );
        return aws::subject_token(client, source, audience).await;
    }
    if let Some(url) = &source.url {
        return url_subject_token(client, url, source).await;
    }
    if let Some(file) = &source.file {
        let content = tokio::fs::read_to_string(file)
            .await
            .with_context(|| format!("read the subject token file {file}"))?;
        return extract_subject_token(&content, source.format.as_ref());
    }
    if let Some(executable) = &source.executable {
        let allowed = std::env::var(ALLOW_EXECUTABLES_ENV).is_ok_and(|v| v == "1");
        return executable_subject_token(
            executable,
            allowed,
            audience,
            subject_token_type,
            impersonation_url,
        )
        .await;
    }
    anyhow::bail!("credential_source names no file, url, executable or AWS environment")
}

/// A `text` (the default) or `json` subject token.
fn extract_subject_token(content: &str, format: Option<&Format>) -> anyhow::Result<String> {
    match format.and_then(|f| f.tp.as_deref()).unwrap_or("") {
        "text" | "" => Ok(content.to_owned()),
        "json" => {
            let field = format
                .and_then(|f| f.subject_token_field_name.as_deref())
                .context("json subject token format has no subject_token_field_name")?;
            let data: serde_json::Value =
                serde_json::from_str(content).context("subject token is not JSON")?;
            data.get(field)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .with_context(|| format!("subject token JSON has no string field `{field}`"))
        }
        other => anyhow::bail!("unsupported subject token format `{other}`"),
    }
}

/// The headers in `credential_source.headers` are the user's config for that URL, so they
/// go as `Auth::Configured`.
async fn url_subject_token(
    client: &HttpClient,
    url: &str,
    source: &CredentialSource,
) -> anyhow::Result<String> {
    let url = Url::parse(url).context("credential_source.url")?;
    let mut headers = HeaderMap::new();
    for (name, value) in source.headers.iter().flatten() {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).context("credential_source header name")?,
            HeaderValue::from_str(value).context("credential_source header value")?,
        );
    }
    let response = client
        .get(url, Auth::Configured(headers))
        .timeout(TOKEN_TIMEOUT)
        .send()
        .await?;
    let body = check_status(response)
        .await
        .context("URL subject token")?
        .text()
        .await?;
    let body: String = body.chars().take(MAX_URL_SUBJECT_TOKEN).collect();
    extract_subject_token(&body, source.format.as_ref())
}

#[derive(serde::Deserialize)]
struct ExecutableResponse {
    version: Option<u32>,
    success: Option<bool>,
    token_type: Option<String>,
    id_token: Option<String>,
    saml_response: Option<String>,
    expiration_time: Option<i64>,
    code: Option<String>,
    message: Option<String>,
}

impl ExecutableResponse {
    /// The token, or `None` when the response says it expired.
    fn token(self, now: i64) -> anyhow::Result<Option<String>> {
        anyhow::ensure!(
            self.version == Some(1),
            "executable response version {:?} is not 1",
            self.version
        );
        if self.success != Some(true) {
            anyhow::bail!(
                "executable failed: {} {}",
                self.code.unwrap_or_default(),
                self.message.unwrap_or_default()
            );
        }
        if self.expiration_time.is_some_and(|exp| exp <= now) {
            return Ok(None);
        }
        let token = match self.token_type.as_deref() {
            Some(
                "urn:ietf:params:oauth:token-type:jwt"
                | "urn:ietf:params:oauth:token-type:id_token",
            ) => self.id_token,
            Some("urn:ietf:params:oauth:token-type:saml2") => self.saml_response,
            other => anyhow::bail!("unsupported executable token_type {other:?}"),
        };
        token.context("executable response has no token").map(Some)
    }
}

/// The impersonated email sits between `serviceAccounts/` and `:generateAccessToken`.
fn impersonated_email(url: &str) -> Option<&str> {
    let rest = url.split_once("serviceAccounts/")?.1;
    Some(rest.split_once(':').map_or(rest, |(email, _)| email))
}

/// Google's executable-sourced credential: run the command and read its JSON response.
/// A cached response in `output_file` that has not expired is used first.
async fn executable_subject_token(
    executable: &Executable,
    allowed: bool,
    audience: &str,
    subject_token_type: &str,
    impersonation_url: Option<&str>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        allowed,
        "executable credential sources run only when {ALLOW_EXECUTABLES_ENV}=1"
    );
    let now = chrono::Utc::now().timestamp();
    if let Some(path) = &executable.output_file
        && let Ok(cached) = tokio::fs::read_to_string(path).await
        && let Ok(response) = serde_json::from_str::<ExecutableResponse>(&cached)
        && let Ok(Some(token)) = response.token(now)
    {
        return Ok(token);
    }

    let mut parts = executable.command.split_whitespace();
    let program = parts.next().context("executable command is empty")?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(parts)
        .env("GOOGLE_EXTERNAL_ACCOUNT_AUDIENCE", audience)
        .env("GOOGLE_EXTERNAL_ACCOUNT_TOKEN_TYPE", subject_token_type)
        .env("GOOGLE_EXTERNAL_ACCOUNT_INTERACTIVE", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(email) = impersonation_url.and_then(impersonated_email) {
        command.env("GOOGLE_EXTERNAL_ACCOUNT_IMPERSONATED_EMAIL", email);
    }
    if let Some(path) = &executable.output_file {
        command.env("GOOGLE_EXTERNAL_ACCOUNT_OUTPUT_FILE", path);
    }
    let timeout = executable
        .timeout_millis
        .map_or(DEFAULT_EXECUTABLE_TIMEOUT, Duration::from_millis);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("executable `{program}` ran past {timeout:?}"))?
        .with_context(|| format!("run executable `{program}`"))?;
    anyhow::ensure!(
        output.status.success(),
        "executable `{program}` exited with {}",
        output.status
    );
    let response: ExecutableResponse =
        serde_json::from_slice(&output.stdout).context("executable output is not JSON")?;
    response
        .token(now)?
        .context("executable returned an expired token")
}

mod aws {

    use super::*;

    const ALGORITHM: &str = "AWS4-HMAC-SHA256";
    const REQUEST_TYPE: &str = "aws4_request";
    const IMDS_TOKEN_HEADER: &str = "X-aws-ec2-metadata-token";
    const VALID_METADATA_HOSTS: [&str; 2] = ["169.254.169.254", "fd00:ec2::254"];

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    pub(super) struct SecurityCredentials {
        pub(super) access_key_id: String,
        pub(super) secret_access_key: String,
        pub(super) token: Option<String>,
    }

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn env_region() -> Option<String> {
        env("AWS_REGION").or_else(|| env("AWS_DEFAULT_REGION"))
    }

    fn env_credentials() -> Option<SecurityCredentials> {
        Some(SecurityCredentials {
            access_key_id: env("AWS_ACCESS_KEY_ID")?,
            secret_access_key: env("AWS_SECRET_ACCESS_KEY")?,
            token: env("AWS_SESSION_TOKEN"),
        })
    }

    /// Only the EC2 metadata server may serve the AWS metadata URLs.
    fn valid_metadata_url(url: Option<&str>) -> bool {
        match url {
            None | Some("") => true,
            Some(url) => Url::parse(url).is_ok_and(|u| {
                let host = u.host_str().unwrap_or("");
                let host = host.trim_start_matches('[').trim_end_matches(']');
                VALID_METADATA_HOSTS.contains(&host)
            }),
        }
    }

    pub(super) async fn subject_token(
        client: &HttpClient,
        source: &CredentialSource,
        audience: &str,
    ) -> anyhow::Result<String> {
        for (name, url) in [
            ("region_url", &source.region_url),
            ("url", &source.url),
            ("imdsv2_session_token_url", &source.imdsv2_session_token_url),
        ] {
            anyhow::ensure!(
                valid_metadata_url(url.as_deref()),
                "credential_source.{name} is not an EC2 metadata URL"
            );
        }
        let env_creds = env_credentials();
        let env_region = env_region();
        let session = match (&env_creds, &env_region, &source.imdsv2_session_token_url) {
            (Some(_), Some(_), _) | (_, _, None) => None,
            (_, _, Some(url)) => Some(imds_session_token(client, url).await?),
        };
        let credentials = match env_creds {
            Some(c) => c,
            None => metadata_credentials(client, source, session.as_deref()).await?,
        };
        let region = match env_region {
            Some(r) => r,
            None => metadata_region(client, source, session.as_deref()).await?,
        };
        let url = source
            .regional_cred_verification_url
            .as_deref()
            .context("credential_source has no regional_cred_verification_url")?
            .replace("{region}", &region);
        let url = Url::parse(&url).context("regional_cred_verification_url")?;
        let target = (!audience.is_empty()).then_some(audience);
        signed_subject_token(&url, &region, &credentials, target, chrono::Utc::now())
    }

    async fn imds_get(
        client: &HttpClient,
        url: &str,
        session: Option<&str>,
    ) -> anyhow::Result<xai_grok_net::Response> {
        let mut request = client.get(url, Auth::None).timeout(TOKEN_TIMEOUT);
        if let Some(token) = session {
            request = request.header(IMDS_TOKEN_HEADER, token);
        }
        check_status(request.send().await?).await
    }

    async fn imds_session_token(client: &HttpClient, url: &str) -> anyhow::Result<String> {
        let response = client
            .put(url, Auth::None)
            .timeout(TOKEN_TIMEOUT)
            .header("X-aws-ec2-metadata-token-ttl-seconds", "300")
            .send()
            .await?;
        Ok(check_status(response)
            .await
            .context("IMDSv2 session token")?
            .text()
            .await?)
    }

    async fn metadata_credentials(
        client: &HttpClient,
        source: &CredentialSource,
        session: Option<&str>,
    ) -> anyhow::Result<SecurityCredentials> {
        let url = source
            .url
            .as_deref()
            .context("credential_source has no security credentials url")?;
        let role = imds_get(client, url, session)
            .await
            .context("AWS role name")?
            .text()
            .await?;
        Ok(imds_get(client, &format!("{url}/{role}"), session)
            .await
            .context("AWS security credentials")?
            .json()
            .await?)
    }

    async fn metadata_region(
        client: &HttpClient,
        source: &CredentialSource,
        session: Option<&str>,
    ) -> anyhow::Result<String> {
        let url = source
            .region_url
            .as_deref()
            .context("credential_source has no region_url")?;
        let zone = imds_get(client, url, session)
            .await
            .context("AWS region")?
            .text()
            .await?;
        // The endpoint answers with a zone such as `us-east-2b`. The region drops the last letter.
        let mut region = zone;
        region.pop();
        Ok(region)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn hmac(key: &[u8], data: &str) -> anyhow::Result<Vec<u8>> {
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key)?;
        mac.update(data.as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    }

    #[derive(serde::Serialize)]
    struct RequestHeader {
        key: String,
        value: String,
    }

    #[derive(serde::Serialize)]
    struct SignedRequest {
        url: String,
        method: &'static str,
        headers: Vec<RequestHeader>,
    }

    /// The URL-encoded JSON of a signed POST to `url`, which STS verifies with AWS.
    pub(super) fn signed_subject_token(
        url: &Url,
        region: &str,
        credentials: &SecurityCredentials,
        target_resource: Option<&str>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<String> {
        let date_short = now.format("%Y%m%d").to_string();
        let date_long = now.format("%Y%m%dT%H%M%SZ").to_string();
        let host = url.host_str().unwrap_or("");
        let mut headers: Vec<(&str, &str)> = vec![("host", host), ("x-amz-date", &date_long)];
        if let Some(token) = &credentials.token {
            headers.push(("x-amz-security-token", token));
        }
        if let Some(target) = target_resource {
            headers.push(("x-goog-cloud-target-resource", target));
        }

        let method = "POST";
        let service = host.split('.').next().unwrap_or("");
        let scope = format!("{date_short}/{region}/{service}/{REQUEST_TYPE}");
        let signed_headers = headers
            .iter()
            .map(|(k, _)| *k)
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let path = match url.path() {
            "" => "/",
            p => p,
        };
        let query = url.query().unwrap_or_default();
        let empty_body_hash = hex(&Sha256::digest([]));
        let canonical = format!(
            "{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{empty_body_hash}"
        );
        let string_to_sign = format!(
            "{ALGORITHM}\n{date_long}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let mut key = format!("AWS4{}", credentials.secret_access_key).into_bytes();
        for input in [
            date_short.as_str(),
            region,
            service,
            REQUEST_TYPE,
            string_to_sign.as_str(),
        ] {
            key = hmac(&key, input)?;
        }
        let authorization = format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={}",
            credentials.access_key_id,
            hex(&key)
        );

        let mut request_headers = vec![RequestHeader {
            key: "Authorization".to_owned(),
            value: authorization,
        }];
        request_headers.extend(headers.iter().map(|(k, v)| RequestHeader {
            key: (*k).to_owned(),
            value: (*v).to_owned(),
        }));
        let request = SignedRequest {
            url: url.to_string(),
            method,
            headers: request_headers,
        };
        let json = serde_json::to_string(&request)?;
        Ok(utf8_percent_encode(&json, NON_ALPHANUMERIC).to_string())
    }
}
