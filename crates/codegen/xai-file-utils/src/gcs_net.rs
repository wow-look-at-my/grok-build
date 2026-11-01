//! Direct GCS uploads over `xai_grok_net`: the OAuth token exchange and the storage call.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use gcloud_storage::client::google_cloud_auth::credentials::CredentialsFile;
use xai_grok_net::header::{self, HeaderMap, HeaderValue};
use xai_grok_net::{Auth, Body, ClientOptions, HttpClient, Url};

pub(crate) const STORAGE_ENDPOINT: &str = "https://storage.googleapis.com";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const ADC_FILE: &str = "application_default_credentials.json";
pub(crate) const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/devstorage.full_control";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const METADATA_HOST_ENV: &str = "GCE_METADATA_HOST";
const METADATA_IP: &str = "169.254.169.254";
pub(crate) const TOKEN_TIMEOUT: Duration = Duration::from_secs(3);

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
            Some(key_json) => token_for_json(&client, key_json)
                .await
                .context("Failed to configure GCS client with service account")?,
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

pub(crate) async fn check_status(
    response: xai_grok_net::Response,
) -> anyhow::Result<xai_grok_net::Response> {
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

pub(crate) async fn read_token(response: xai_grok_net::Response) -> anyhow::Result<String> {
    let token: TokenResponse = check_status(response).await?.json().await?;
    Ok(token.access_token)
}

/// ADC order, as `gcloud-auth` does it: the credentials file, else the metadata server.
async fn adc_token(client: &HttpClient) -> anyhow::Result<String> {
    match adc_json().await {
        Ok(json) => token_for_json(client, &json).await,
        Err(file_err) => metadata_token(client).await.map_err(|metadata_err| {
            anyhow::anyhow!(
                "no GCP credentials: credentials file: {file_err:#}; metadata server: {metadata_err:#}"
            )
        }),
    }
}

/// The ADC credentials JSON, found where `gcloud-auth` looks: `GOOGLE_APPLICATION_CREDENTIALS_JSON`
/// (raw or base64), the `GOOGLE_APPLICATION_CREDENTIALS` path, then the gcloud well-known file.
async fn adc_json() -> anyhow::Result<String> {
    use base64::Engine as _;
    if let Ok(json) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS_JSON") {
        return Ok(base64::engine::general_purpose::STANDARD
            .decode(&json)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or(json));
    }
    let path = match std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS") {
        Some(path) => std::path::PathBuf::from(path),
        None if cfg!(target_os = "windows") => {
            std::path::PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is not set")?)
                .join("gcloud")
                .join(ADC_FILE)
        }
        None => dirs::home_dir()
            .context("no home directory")?
            .join(".config")
            .join("gcloud")
            .join(ADC_FILE),
    };
    tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("read {}", path.display()))
}

/// The access token for a credentials JSON. `gcloud-auth` parses it. Every request goes
/// through `xai_grok_net`.
pub(crate) async fn token_for_json(client: &HttpClient, json: &str) -> anyhow::Result<String> {
    let cred = CredentialsFile::new_from_str(json)
        .await
        .context("parse the GCP credentials")?;
    match cred.tp.as_str() {
        "service_account" => service_account_token(client, &cred).await,
        "authorized_user" => authorized_user_token(client, &cred).await,
        "external_account" => {
            crate::gcs_external::external_account_token(client, &cred, json, SCOPES).await
        }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::{Form, Json, Query};
    use axum::http::HeaderMap as AxumHeaders;
    use axum::routing::post;
    use base64::Engine as _;

    use super::*;

    #[derive(Default)]
    struct Seen {
        token_forms: Vec<HashMap<String, String>>,
        refresh_bodies: Vec<serde_json::Value>,
        uploads: Vec<(HashMap<String, String>, HashMap<String, String>, Vec<u8>)>,
    }

    fn header_strings(headers: &AxumHeaders) -> HashMap<String, String> {
        headers
            .iter()
            .map(|(n, v)| (n.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect()
    }

    /// A mock of the OAuth token endpoint and the GCS upload endpoint.
    async fn start_mock() -> (String, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let s1 = seen.clone();
        let s2 = seen.clone();
        let s3 = seen.clone();
        let router = Router::new()
            .route(
                "/token",
                post(move |Form(form): Form<HashMap<String, String>>| {
                    let seen = s1.clone();
                    async move {
                        seen.lock().unwrap().token_forms.push(form);
                        Json(serde_json::json!({
                            "access_token": "ya29.service-account",
                            "token_type": "Bearer",
                            "expires_in": 3600
                        }))
                    }
                }),
            )
            .route(
                "/refresh",
                post(move |Json(body): Json<serde_json::Value>| {
                    let seen = s2.clone();
                    async move {
                        seen.lock().unwrap().refresh_bodies.push(body);
                        Json(serde_json::json!({
                            "access_token": "ya29.user",
                            "token_type": "Bearer",
                            "expires_in": 3600
                        }))
                    }
                }),
            )
            .route(
                "/upload/storage/v1/b/{bucket}/o",
                post(
                    move |Query(query): Query<HashMap<String, String>>,
                          headers: AxumHeaders,
                          body: axum::body::Bytes| {
                        let seen = s3.clone();
                        async move {
                            seen.lock().unwrap().uploads.push((
                                query,
                                header_strings(&headers),
                                body.to_vec(),
                            ));
                            Json(serde_json::json!({}))
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), seen)
    }

    fn client() -> HttpClient {
        HttpClient::build(ClientOptions::default()).unwrap()
    }

    #[tokio::test]
    async fn service_account_key_is_exchanged_over_net_with_a_signed_jwt() {
        use aws_lc_rs::encoding::AsDer;
        use aws_lc_rs::signature::KeyPair as _;

        let (base, seen) = start_mock().await;
        let key = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let pkcs8: aws_lc_rs::encoding::Pkcs8V1Der = key.as_der().unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );
        let token_url = format!("{base}/token");
        let key_json = serde_json::json!({
            "type": "service_account",
            "client_email": "uploader@project.iam.gserviceaccount.com",
            "private_key_id": "kid-1",
            "private_key": pem,
            "token_uri": token_url,
        })
        .to_string();
        let cred = CredentialsFile::new_from_str(&key_json).await.unwrap();

        let token = token_for_credentials(&client(), &cred).await.unwrap();
        assert_eq!(token, "ya29.service-account");

        let form = seen
            .lock()
            .unwrap()
            .token_forms
            .pop()
            .expect("token request");
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some(JWT_BEARER_GRANT)
        );
        let decoding = jsonwebtoken::DecodingKey::from_rsa_der(key.public_key().as_ref());
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_audience(&[&token_url]);
        let jwt = jsonwebtoken::decode::<serde_json::Value>(
            form.get("assertion").expect("assertion"),
            &decoding,
            &validation,
        )
        .expect("the assertion is signed with the service account key");
        assert_eq!(jwt.header.kid.as_deref(), Some("kid-1"));
        assert_eq!(
            jwt.claims["iss"],
            "uploader@project.iam.gserviceaccount.com"
        );
        assert_eq!(jwt.claims["scope"], SCOPES);
    }

    #[tokio::test]
    async fn authorized_user_refresh_goes_over_net() {
        let (base, seen) = start_mock().await;
        let cred_json = serde_json::json!({
            "type": "authorized_user",
            "client_id": "cid",
            "client_secret": "csecret",
            "refresh_token": "rtok",
            "token_uri": format!("{base}/refresh"),
        })
        .to_string();
        let cred = CredentialsFile::new_from_str(&cred_json).await.unwrap();
        let token = token_for_credentials(&client(), &cred).await.unwrap();
        assert_eq!(token, "ya29.user");
        let body = seen.lock().unwrap().refresh_bodies.pop().expect("refresh");
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "rtok");
    }

    #[tokio::test]
    async fn simple_upload_reaches_the_storage_endpoint_with_the_bearer() {
        let (base, seen) = start_mock().await;
        GcsDirect::with_bearer(client(), "Bearer ya29.upload".into(), &base)
            .upload(
                "my-bucket",
                "a/b c.json",
                "application/json",
                b"{}".to_vec().into(),
            )
            .await
            .unwrap();
        let (query, headers, body) = seen.lock().unwrap().uploads.pop().expect("upload");
        assert_eq!(query.get("uploadType").map(String::as_str), Some("media"));
        assert_eq!(query.get("name").map(String::as_str), Some("a/b c.json"));
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer ya29.upload")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(body, b"{}");
    }

    #[tokio::test]
    async fn a_403_reads_as_an_auth_failure_to_the_queue() {
        let router = Router::new().route(
            "/upload/storage/v1/b/{bucket}/o",
            post(|| async { axum::http::StatusCode::FORBIDDEN }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let err = GcsDirect::with_bearer(client(), "Bearer x".into(), &format!("http://{addr}"))
            .upload("b", "o", "text/plain", Vec::new().into())
            .await
            .expect_err("the mock answers 403");
        assert!(err.to_string().contains("HTTP 403"), "{err}");
    }

    #[tokio::test]
    async fn a_session_token_as_the_gcs_bearer_is_refused_by_net() {
        let token = "xai-session-token-for-gcs-routing-test";
        xai_grok_net::register_session_token(token);
        let err = GcsDirect::with_bearer(
            client(),
            format!("Bearer {token}"),
            "https://storage.routing-test.invalid",
        )
        .upload("b", "o", "text/plain", b"x".to_vec().into())
        .await
        .expect_err("net must refuse the session token");
        assert!(
            err.chain()
                .any(|e| e.downcast_ref::<xai_grok_net::AuthRefusal>().is_some()),
            "{err:?}"
        );
    }
}
