//! Direct GCS uploads over `xai_grok_net`: the OAuth token exchange and the storage call.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use gcloud_storage::client::google_cloud_auth::credentials::CredentialsFile;
use xai_grok_net::header::{self, HeaderMap, HeaderValue};
use xai_grok_net::{Auth, Body, ClientOptions, HttpClient, Url};

pub(crate) const STORAGE_ENDPOINT: &str = "https://storage.googleapis.com";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/devstorage.full_control";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const METADATA_HOST_ENV: &str = "GCE_METADATA_HOST";
const METADATA_IP: &str = "169.254.169.254";
const TOKEN_TIMEOUT: Duration = Duration::from_secs(3);

/// A GCS client with the bearer for one upload.
pub(crate) struct GcsDirect {
    client: HttpClient,
    bearer: String,
    endpoint: String,
}

impl GcsDirect {
    /// Get a token from the service account key, or from Application Default Credentials.
    pub(crate) async fn new(service_account_key: Option<&str>) -> anyhow::Result<Self> {
        let client = HttpClient::build(ClientOptions::default()).context("build the GCS client")?;
        let token = match service_account_key {
            Some(key_json) => {
                let cred = CredentialsFile::new_from_str(key_json)
                    .await
                    .context("Failed to parse service account key")?;
                token_for_credentials(&client, &cred)
                    .await
                    .context("Failed to configure GCS client with service account")?
            }
            None => adc_token(&client)
                .await
                .context("Failed to authenticate GCS client")?,
        };
        Ok(Self::with_bearer(
            client,
            format!("Bearer {token}"),
            STORAGE_ENDPOINT,
        ))
    }

    pub(crate) fn with_bearer(client: HttpClient, bearer: String, endpoint: &str) -> Self {
        Self {
            client,
            bearer,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
        }
    }

    /// A simple media upload (`uploadType=media`).
    pub(crate) async fn upload(
        &self,
        bucket: &str,
        object_path: &str,
        content_type: &str,
        body: Body,
    ) -> anyhow::Result<()> {
        let mut url = Url::parse(&self.endpoint).context("GCS endpoint URL")?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("GCS endpoint URL cannot take a path"))?
            .extend(["upload", "storage", "v1", "b", bucket, "o"]);
        url.query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", object_path);
        let mut auth = HeaderMap::new();
        auth.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&self.bearer).context("GCS bearer is not a header value")?,
        );
        let response = self
            .client
            .post(url, Auth::Configured(auth))
            .header(header::CONTENT_TYPE, content_type)
            .header("X-Goog-Api-Client", "rust")
            .header(header::USER_AGENT, "google-cloud-storage")
            .body(body)
            .send()
            .await?;
        check_status(response).await.map(drop)
    }
}

async fn check_status(response: xai_grok_net::Response) -> anyhow::Result<xai_grok_net::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!("HTTP {status}: {body}")
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
}

async fn read_token(response: xai_grok_net::Response) -> anyhow::Result<String> {
    let token: TokenResponse = check_status(response).await?.json().await?;
    Ok(token.access_token)
}

/// ADC order, as `gcloud-auth` does it: the credentials file, else the metadata server.
async fn adc_token(client: &HttpClient) -> anyhow::Result<String> {
    match CredentialsFile::new().await {
        Ok(cred) => token_for_credentials(client, &cred).await,
        Err(file_err) => metadata_token(client).await.map_err(|metadata_err| {
            anyhow::anyhow!(
                "no GCP credentials: credentials file: {file_err}; metadata server: {metadata_err:#}"
            )
        }),
    }
}

/// The access token for a parsed credentials file.
pub(crate) async fn token_for_credentials(
    client: &HttpClient,
    cred: &CredentialsFile,
) -> anyhow::Result<String> {
    match cred.tp.as_str() {
        "service_account" => service_account_token(client, cred).await,
        "authorized_user" => authorized_user_token(client, cred).await,
        "external_account" => external_account_token(cred).await,
        other => anyhow::bail!("unsupported GCP credential type `{other}`"),
    }
}

#[derive(serde::Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    exp: u64,
    iat: u64,
}

async fn service_account_token(
    client: &HttpClient,
    cred: &CredentialsFile,
) -> anyhow::Result<String> {
    let token_url = cred.token_uri.as_deref().unwrap_or(TOKEN_URL);
    let email = cred.client_email.as_deref().unwrap_or_default();
    let pem = cred
        .private_key
        .as_deref()
        .context("service account key has no private_key")?;
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes())
        .context("service account private_key is not an RSA PEM key")?;
    let iat = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = Claims {
        iss: email,
        scope: SCOPES,
        aud: token_url,
        exp: iat + 3600,
        iat,
    };
    let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    jwt_header.kid = cred.private_key_id.clone();
    let assertion = jsonwebtoken::encode(&jwt_header, &claims, &key)?;
    let response = client
        .post(token_url, Auth::None)
        .timeout(TOKEN_TIMEOUT)
        .form(&[("grant_type", JWT_BEARER_GRANT), ("assertion", &assertion)])
        .send()
        .await?;
    read_token(response).await
}

/// The refresh-token flow for a `gcloud auth application-default login` credential.
async fn authorized_user_token(
    client: &HttpClient,
    cred: &CredentialsFile,
) -> anyhow::Result<String> {
    let token_url = cred.token_uri.as_deref().unwrap_or(TOKEN_URL);
    let refresh_token = cred
        .refresh_token
        .as_deref()
        .context("authorized_user credential has no refresh_token")?;
    let body = serde_json::json!({
        "client_id": cred.client_id.as_deref().unwrap_or_default(),
        "client_secret": cred.client_secret.as_deref().unwrap_or_default(),
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    let response = client
        .post(token_url, Auth::None)
        .timeout(TOKEN_TIMEOUT)
        .json(&body)
        .send()
        .await?;
    read_token(response).await
}

/// The default service account's token from the GCE metadata server.
async fn metadata_token(client: &HttpClient) -> anyhow::Result<String> {
    let host = std::env::var(METADATA_HOST_ENV).unwrap_or_else(|_| METADATA_IP.to_owned());
    let mut url = Url::parse(&format!(
        "http://{host}/computeMetadata/v1/instance/service-accounts/default/token"
    ))
    .context("metadata server URL")?;
    url.query_pairs_mut()
        .append_pair("scopes", &SCOPES.replace(' ', ","));
    let response = client
        .get(url, Auth::None)
        .timeout(TOKEN_TIMEOUT)
        .header("Metadata-Flavor", "Google")
        .send()
        .await?;
    read_token(response).await
}

/// Workload identity federation reads a subject token from a file, a URL, or AWS, trades it
/// at STS, and can impersonate a service account. `gcloud-auth` runs that exchange with its
/// own HTTP client, because it takes no injected one.
async fn external_account_token(cred: &CredentialsFile) -> anyhow::Result<String> {
    use gcloud_storage::client::google_cloud_auth::project::{
        Config, create_token_source_from_credentials,
    };
    let scopes: Vec<&str> = SCOPES.split(' ').collect();
    let source =
        create_token_source_from_credentials(cred, &Config::default().with_scopes(&scopes))
            .await
            .context("external_account token source")?;
    let token = source.token().await.context("external_account token")?;
    Ok(token.access_token)
}
