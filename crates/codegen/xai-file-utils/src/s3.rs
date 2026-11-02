//! S3 URL presigning for the ZDR video output bucket. It signs locally and sends no request.

<<<<<<< HEAD
/// Static access-key credentials for presigning S3 URLs. `Debug` is redacted,
/// because the struct holds plaintext secrets.
=======
use anyhow::Context;
use bytes::Bytes;
use futures::StreamExt;
use xai_grok_net::Method;

use crate::s3_http::{Addressing, ObjectRequest, S3Client, S3Error, xml_escape, xml_text};
use crate::s3_sigv4::Credentials;

/// Map an S3 HeadObject error to NotFound / Unauthorized / ProbeFailed.
/// An error with no status (build, dispatch, timeout) is transient.
fn classify_head_error(err: &S3Error) -> HeadOutcome {
    match err.status() {
        Some(404) => HeadOutcome::NotFound,
        Some(401) | Some(403) => HeadOutcome::Unauthorized,
        _ => HeadOutcome::ProbeFailed,
    }
}

#[derive(Debug)]
enum HeadOutcome {
    NotFound,
    Unauthorized,
    ProbeFailed,
}

/// Some S3-compatible endpoints reject single PutObject chunks above 16 MiB.
/// Use multipart upload with 8 MiB parts to stay within that limit.
const MULTIPART_THRESHOLD: usize = 8 * 1024 * 1024;
const MULTIPART_PART_SIZE: usize = 8 * 1024 * 1024;

/// Build an S3 client. Uses path-style addressing when `endpoint_url` is set.
/// Every request goes through `xai_grok_net`, which reads the proxy from the environment.
pub(crate) async fn build_s3_client(
    region: &str,
    credentials_content: Option<&str>,
    credentials_file: Option<&str>,
    endpoint_url: Option<&str>,
) -> anyhow::Result<S3Client> {
    let creds = crate::s3_credentials::resolve(credentials_content, credentials_file).await?;
    S3Client::new(creds, Addressing::new(region, endpoint_url)?)
}

/// Static access-key credentials for presigning S3 URLs.
///
/// `Debug` is intentionally redacted — the struct holds plaintext secrets.
>>>>>>> origin/master
#[derive(Clone)]
pub struct S3StaticCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for S3StaticCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3StaticCredentials")
            .field("access_key_id", &"[redacted]")
            .field("secret_access_key", &"[redacted]")
            .finish()
    }
}

<<<<<<< HEAD
/// Path-style addressing is used when `endpoint_url` is set.
fn presign_client(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
) -> aws_sdk_s3::Client {
    let credentials = aws_sdk_s3::config::Credentials::new(
        &creds.access_key_id,
        &creds.secret_access_key,
        None,
        None,
        "grok-static",
    );
    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(region.to_owned()))
        .credentials_provider(credentials)
        .force_path_style(endpoint_url.is_some());
    if let Some(url) = endpoint_url {
        builder = builder.endpoint_url(url);
=======
impl S3StaticCredentials {
    fn to_credentials(&self) -> Credentials {
        Credentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: None,
        }
>>>>>>> origin/master
    }
    aws_sdk_s3::Client::from_conf(builder.build())
}

pub async fn presign_put_url(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
    bucket: &str,
    key: &str,
    content_type: &str,
    expires_in: std::time::Duration,
) -> anyhow::Result<String> {
<<<<<<< HEAD
    let client = presign_client(region, endpoint_url, creds);
    let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)?;
    let presigned = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .presigned(presigning_config)
        .await?;
    Ok(presigned.uri().to_string())
=======
    crate::s3_http::presign_url(
        &creds.to_credentials(),
        &Addressing::new(region, endpoint_url)?,
        &Method::PUT,
        bucket,
        key,
        Some(content_type),
        expires_in,
        chrono::Utc::now(),
    )
>>>>>>> origin/master
}

pub async fn presign_get_url(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
    bucket: &str,
    key: &str,
    expires_in: std::time::Duration,
) -> anyhow::Result<String> {
<<<<<<< HEAD
    let client = presign_client(region, endpoint_url, creds);
    let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)?;
    let presigned = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(presigning_config)
        .await?;
    Ok(presigned.uri().to_string())
}

=======
    crate::s3_http::presign_url(
        &creds.to_credentials(),
        &Addressing::new(region, endpoint_url)?,
        &Method::GET,
        bucket,
        key,
        None,
        expires_in,
        chrono::Utc::now(),
    )
}

fn upload_id_query(upload_id: &str) -> Vec<(String, String)> {
    vec![("uploadId".to_owned(), upload_id.to_owned())]
}

fn abort_request<'a>(bucket: &'a str, key: &'a str, upload_id: &str) -> ObjectRequest<'a> {
    ObjectRequest {
        operation: "AbortMultipartUpload",
        method: Method::DELETE,
        bucket,
        key,
        query: upload_id_query(upload_id),
        content_type: None,
        body: Bytes::new(),
    }
}

fn head_request<'a>(bucket: &'a str, key: &'a str) -> ObjectRequest<'a> {
    ObjectRequest {
        operation: "HeadObject",
        method: Method::HEAD,
        bucket,
        key,
        query: Vec::new(),
        content_type: None,
        body: Bytes::new(),
    }
}

async fn put_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    content_type: &str,
    body: Bytes,
) -> Result<(), S3Error> {
    client
        .send(&ObjectRequest {
            operation: "PutObject",
            method: Method::PUT,
            bucket,
            key,
            query: Vec::new(),
            content_type: Some(content_type),
            body,
        })
        .await
        .map(|_| ())
}

/// Aborts a created multipart upload when its future is dropped before completion.
/// Callers cancel by drop; a dropped future skips Complete and the error-path abort.
/// That leaves orphaned billable parts in the (possibly customer-managed) bucket.
struct AbortMultipartOnDrop {
    client: S3Client,
    bucket: String,
    key: String,
    upload_id: String,
    armed: bool,
}

impl AbortMultipartOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }

    /// In-scope abort for the error path: awaited, unlike the drop hook.
    /// Disarms only after the send completes, so cancelling this await still gets the drop-hook abort.
    /// AbortMultipartUpload is idempotent server-side.
    async fn abort(mut self) {
        let _ = self
            .client
            .send(&abort_request(&self.bucket, &self.key, &self.upload_id))
            .await;
        self.armed = false;
    }
}

impl Drop for AbortMultipartOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Detached: a drop site cannot await. Bounded, since the cancellation
        // often means the endpoint is already unresponsive.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let client = self.client.clone();
        let bucket = std::mem::take(&mut self.bucket);
        let key = std::mem::take(&mut self.key);
        let upload_id = std::mem::take(&mut self.upload_id);
        handle.spawn(async move {
            let request = abort_request(&bucket, &key, &upload_id);
            let abort = client.send(&request);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(30), abort).await;
        });
    }
}

/// Multipart upload for payloads that exceed [`MULTIPART_THRESHOLD`].
/// Splits into [`MULTIPART_PART_SIZE`] chunks. Aborts on any part failure or cancellation
/// so incomplete multipart uploads are not leaked.
async fn multipart_upload_bytes(
    client: &S3Client,
    bucket: &str,
    object_path: &str,
    content: &[u8],
    content_type: &str,
) -> anyhow::Result<()> {
    let create = client
        .send(&ObjectRequest {
            operation: "CreateMultipartUpload",
            method: Method::POST,
            bucket,
            key: object_path,
            query: vec![("uploads".to_owned(), String::new())],
            content_type: Some(content_type),
            body: Bytes::new(),
        })
        .await
        .with_context(|| {
            format!("Failed to create multipart upload for s3://{bucket}/{object_path}")
        })?;

    let upload_id = xml_text(&String::from_utf8_lossy(&create.body), "UploadId")
        .filter(|id| !id.is_empty())
        .context("CreateMultipartUpload response missing upload_id")?;
    let abort_guard = AbortMultipartOnDrop {
        client: client.clone(),
        bucket: bucket.to_owned(),
        key: object_path.to_owned(),
        upload_id: upload_id.clone(),
        armed: true,
    };

    let mut completed_parts: Vec<(u32, String)> = Vec::new();
    let mut offset = 0usize;
    let mut part_number = 1u32;

    let result: anyhow::Result<()> = async {
        while offset < content.len() {
            let end = (offset + MULTIPART_PART_SIZE).min(content.len());
            let Some(chunk) = content.get(offset..end) else {
                break;
            };

            let mut query = upload_id_query(&upload_id);
            query.push(("partNumber".to_owned(), part_number.to_string()));
            let upload_part = client
                .send(&ObjectRequest {
                    operation: "UploadPart",
                    method: Method::PUT,
                    bucket,
                    key: object_path,
                    query,
                    content_type: None,
                    body: Bytes::copy_from_slice(chunk),
                })
                .await
                .with_context(|| {
                    format!("Failed to upload part {part_number} for s3://{bucket}/{object_path}")
                })?;

            let etag = upload_part
                .headers
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .context("UploadPart response missing ETag")?
                .to_owned();

            completed_parts.push((part_number, etag));

            offset = end;
            part_number += 1;
        }

        let mut body = String::from(
            "<CompleteMultipartUpload xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
        );
        for (number, etag) in &completed_parts {
            body.push_str(&format!(
                "<Part><PartNumber>{number}</PartNumber><ETag>{}</ETag></Part>",
                xml_escape(etag)
            ));
        }
        body.push_str("</CompleteMultipartUpload>");

        client
            .send(&ObjectRequest {
                operation: "CompleteMultipartUpload",
                method: Method::POST,
                bucket,
                key: object_path,
                query: upload_id_query(&upload_id),
                content_type: Some("application/xml"),
                body: Bytes::from(body),
            })
            .await
            .with_context(|| {
                format!("Failed to complete multipart upload for s3://{bucket}/{object_path}")
            })?;

        Ok(())
    }
    .await;

    if result.is_err() {
        abort_guard.abort().await;
    } else {
        abort_guard.disarm();
    }

    result
}

/// Upload bytes to an S3-compatible bucket.
pub async fn upload_bytes(
    bucket: &str,
    object_path: &str,
    content: &[u8],
    content_type: &str,
    region: &str,
    credentials_content: Option<&str>,
    credentials_file: Option<&str>,
    endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    let client =
        build_s3_client(region, credentials_content, credentials_file, endpoint_url).await?;
    if content.len() >= MULTIPART_THRESHOLD {
        multipart_upload_bytes(&client, bucket, object_path, content, content_type).await?;
    } else {
        put_object(
            &client,
            bucket,
            object_path,
            content_type,
            Bytes::copy_from_slice(content),
        )
        .await
        .with_context(|| format!("Failed to upload to s3://{bucket}/{object_path}"))?;
    }
    Ok(format!("s3://{bucket}/{object_path}"))
}

/// Upload a file to an S3-compatible bucket. The file is read into memory,
/// because the signature covers the SHA-256 of the body.
pub async fn upload_file(
    bucket: &str,
    object_path: &str,
    file_path: &Path,
    content_type: &str,
    region: &str,
    credentials_content: Option<&str>,
    credentials_file: Option<&str>,
    endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    let client =
        build_s3_client(region, credentials_content, credentials_file, endpoint_url).await?;
    let content = tokio::fs::read(file_path)
        .await
        .with_context(|| format!("Failed to read file for S3 upload: {}", file_path.display()))?;
    if content.len() >= MULTIPART_THRESHOLD {
        multipart_upload_bytes(&client, bucket, object_path, &content, content_type).await?;
    } else {
        put_object(
            &client,
            bucket,
            object_path,
            content_type,
            Bytes::from(content),
        )
        .await
        .with_context(|| format!("Failed to upload to s3://{bucket}/{object_path}"))?;
    }
    Ok(format!("s3://{bucket}/{object_path}"))
}

/// Upload an async reader to S3.
/// Buffers first because S3 PutObject requires a known Content-Length.
/// Acceptable for the primary caller: zstd-compressed dedup blobs are typically small.
pub async fn upload_stream<R: tokio::io::AsyncRead + Send + Sync + 'static>(
    bucket: &str,
    object_path: &str,
    reader: R,
    content_type: &str,
    region: &str,
    credentials_content: Option<&str>,
    credentials_file: Option<&str>,
    endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    use tokio::io::AsyncReadExt;

    let mut buf = Vec::new();
    tokio::pin!(reader);
    reader.read_to_end(&mut buf).await?;

    upload_bytes(
        bucket,
        object_path,
        &buf,
        content_type,
        region,
        credentials_content,
        credentials_file,
        endpoint_url,
    )
    .await
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // Used once the S3 storage backend is wired up.
pub struct S3ExistsResponse {
    pub bucket: String,
    pub path: String,
    pub size: i64,
}

/// S3-native storage client providing batch operations via concurrent requests.
/// Holds one [`S3Client`] for the lifetime of the struct.
#[allow(dead_code)] // Used once the S3 storage backend is wired up.
pub struct S3StorageClient {
    client: S3Client,
    bucket: String,
}

#[allow(dead_code)] // Used once the S3 storage backend is wired up.
impl S3StorageClient {
    pub fn bucket_name(&self) -> &str {
        &self.bucket
    }

    pub async fn new(
        bucket: String,
        region: &str,
        credentials_content: Option<&str>,
        credentials_file: Option<&str>,
        endpoint_url: Option<&str>,
    ) -> anyhow::Result<Self> {
        let client =
            build_s3_client(region, credentials_content, credentials_file, endpoint_url).await?;
        tracing::debug!(bucket = %bucket, region, endpoint = ?endpoint_url, "S3StorageClient created");
        Ok(Self { client, bucket })
    }

    /// Check existence of multiple S3 objects via concurrent HeadObject calls.
    /// Aggregates worst-first: 401/403 → `Unauthorized`, non-404 transient → `ProbeFailed`, else found/not.
    pub async fn batch_check_exists<S: AsRef<str>>(
        &self,
        paths: &[S],
    ) -> crate::storage_client::ExistsResult<HashSet<String>> {
        use crate::storage_client::ExistsResult;
        let client = &self.client;
        let bucket = &*self.bucket;

        // Owned strings keep the closures HRTB-clean for callers passing both
        // `&[String]` and `&[&str]` from the same async fn.
        let owned_paths: Vec<String> = paths
            .iter()
            .map(|p| <S as AsRef<str>>::as_ref(p).to_string())
            .collect();
        let total = owned_paths.len();

        #[derive(Debug)]
        enum PathOutcome {
            Exists(String),
            NotFound,
            Unauthorized,
            ProbeFailed,
        }

        let outcomes: Vec<PathOutcome> = futures::stream::iter(owned_paths)
            .map(|path| async move {
                match client.send(&head_request(bucket, &path)).await {
                    Ok(_) => PathOutcome::Exists(path),
                    Err(e) => match classify_head_error(&e) {
                        HeadOutcome::NotFound => PathOutcome::NotFound,
                        HeadOutcome::Unauthorized => PathOutcome::Unauthorized,
                        HeadOutcome::ProbeFailed => PathOutcome::ProbeFailed,
                    },
                }
            })
            .buffer_unordered(32)
            .collect()
            .await;

        let mut found = HashSet::new();
        let mut not_found_count: usize = 0;
        let mut any_unauthorized = false;
        let mut any_transient = false;
        for o in outcomes {
            match o {
                PathOutcome::Exists(p) => {
                    found.insert(p);
                }
                PathOutcome::NotFound => not_found_count += 1,
                PathOutcome::Unauthorized => any_unauthorized = true,
                PathOutcome::ProbeFailed => any_transient = true,
            }
        }
        if any_unauthorized {
            ExistsResult::Unauthorized
        } else if any_transient {
            ExistsResult::ProbeFailed
        } else if total > 0 && not_found_count == total {
            // Symmetric with the proxy: all-404 batch surfaces as NotFound,
            // not Found(empty_set).
            ExistsResult::NotFound
        } else {
            ExistsResult::Found(found)
        }
    }

    /// Upload multiple small files via concurrent PutObject calls.
    /// Returns the proxy-compatible `BatchUploadResult` so downstream handling stays unchanged.
    pub async fn batch_upload(
        &self,
        files: Vec<(String, Vec<u8>, String)>,
    ) -> Option<Vec<prod_mc_cli_chat_proxy_types::BatchUploadResult>> {
        let client = &self.client;
        let bucket = &*self.bucket;
        let results: Vec<prod_mc_cli_chat_proxy_types::BatchUploadResult> = futures::stream::iter(
            files,
        )
        .map(|(path, content, content_type)| async move {
            let size = content.len() as i64;
            let bucket_owned = bucket.to_string();
            let upload_result = if content.len() >= MULTIPART_THRESHOLD {
                multipart_upload_bytes(client, bucket, &path, &content, &content_type).await
            } else {
                put_object(client, bucket, &path, &content_type, Bytes::from(content))
                    .await
                    .map_err(|e| anyhow::anyhow!(e))
            };
            match upload_result {
                Ok(_) => prod_mc_cli_chat_proxy_types::BatchUploadResult {
                    path,
                    bucket: Some(bucket_owned),
                    status: prod_mc_cli_chat_proxy_types::BatchUploadStatus::Ok,
                    size: Some(size),
                    generation: None,
                    error: None,
                },
                Err(e) => {
                    let error_msg = format!("{:#}", e);
                    tracing::warn!(path = %path, error = %error_msg, "S3 batch upload item failed");
                    prod_mc_cli_chat_proxy_types::BatchUploadResult {
                        path,
                        bucket: Some(bucket_owned),
                        status: prod_mc_cli_chat_proxy_types::BatchUploadStatus::Error,
                        size: None,
                        generation: None,
                        error: Some(error_msg),
                    }
                }
            }
        })
        .buffer_unordered(16)
        .collect()
        .await;

        Some(results)
    }

    /// Check if a single object exists via HeadObject.
    pub async fn check_exists(&self, path: &str) -> Option<S3ExistsResponse> {
        match self.client.send(&head_request(&self.bucket, path)).await {
            Ok(response) => Some(S3ExistsResponse {
                bucket: self.bucket.clone(),
                path: path.to_string(),
                size: response
                    .headers
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            }),
            Err(_) => None,
        }
    }
}

>>>>>>> origin/master
#[cfg(test)]
mod tests {
    use super::*;

<<<<<<< HEAD
    fn creds() -> S3StaticCredentials {
        S3StaticCredentials {
            access_key_id: "AKIDPRESIGN".into(),
            secret_access_key: "secret".into(),
=======
    /// In-progress multipart uploads: upload_id -> (key, parts: part_number -> bytes).
    type MultipartUploads = HashMap<String, (String, HashMap<i32, Vec<u8>>)>;

    /// Shared state for the mock S3 server.
    struct MockS3State {
        objects: RwLock<HashMap<String, Vec<u8>>>,
        multipart_uploads: RwLock<MultipartUploads>,
    }

    fn xml_response(status: u16, body: String) -> axum::response::Response {
        axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/xml")
            .body(axum::body::Body::from(body))
            .unwrap()
            .into_response()
    }

    fn s3_error(status: u16, code: &str, message: &str) -> axum::response::Response {
        xml_response(
            status,
            format!("<Error><Code>{code}</Code><Message>{message}</Message></Error>"),
        )
    }

    /// Recompute the SigV4 signature of the request as received, with secret `test`.
    async fn verify_sigv4(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let (parts, body) = req.into_parts();
        let body = axum::body::to_bytes(body, 4 * MULTIPART_PART_SIZE)
            .await
            .unwrap();
        let header = |n: &str| {
            parts
                .headers
                .get(n)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        };
        let auth = header("authorization");
        let Some(rest) = auth.strip_prefix("AWS4-HMAC-SHA256 Credential=test/") else {
            return s3_error(403, "AccessDenied", "unsigned request");
        };
        let mut fields = rest.split(", ");
        let region = fields
            .next()
            .and_then(|scope| scope.split('/').nth(1))
            .unwrap_or("")
            .to_owned();
        let signed = fields
            .next()
            .and_then(|f| f.strip_prefix("SignedHeaders="))
            .unwrap_or("");
        let payload_hash = header("x-amz-content-sha256");
        if payload_hash != crate::s3_sigv4::sha256_hex(&body) {
            return s3_error(400, "XAmzContentSHA256Mismatch", "payload hash");
        }
        let time = chrono::NaiveDateTime::parse_from_str(&header("x-amz-date"), "%Y%m%dT%H%M%SZ")
            .unwrap()
            .and_utc();
        let headers: Vec<(String, String)> = signed
            .split(';')
            .map(|n| (n.to_owned(), header(n)))
            .collect();
        let query: Vec<(String, String)> =
            url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let creds = Credentials {
            access_key_id: "test".into(),
            secret_access_key: "test".into(),
            session_token: None,
        };
        let expected = crate::s3_sigv4::Signer::new(&creds, &region, time).authorization(
            parts.method.as_str(),
            parts.uri.path(),
            &query,
            &headers,
            &payload_hash,
        );
        if expected != auth {
            return s3_error(403, "SignatureDoesNotMatch", "signature mismatch");
        }
        next.run(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::from(body),
        ))
        .await
    }

    /// Mock S3 server with PUT/GET/HEAD/POST/DELETE and multipart upload support.
    fn mock_s3_router() -> (Router, Arc<MockS3State>) {
        let state = Arc::new(MockS3State {
            objects: RwLock::new(HashMap::new()),
            multipart_uploads: RwLock::new(HashMap::new()),
        });

        let s = state.clone();
        let head_handler = move |AxumPath((_, key)): AxumPath<(String, String)>| {
            let state = s.clone();
            async move {
                match state.objects.read().await.get(&key) {
                    Some(data) => (StatusCode::OK, [("content-length", data.len().to_string())])
                        .into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        };

        let s = state.clone();
        let put_handler = move |AxumPath((_, key)): AxumPath<(String, String)>,
                                query: axum::extract::Query<HashMap<String, String>>,
                                body: axum::body::Bytes| {
            let state = s.clone();
            async move {
                if let (Some(pn), Some(uid)) = (query.get("partNumber"), query.get("uploadId")) {
                    let part_num: i32 = pn.parse().unwrap_or(0);
                    let mut uploads = state.multipart_uploads.write().await;
                    if let Some((_, parts)) = uploads.get_mut(uid) {
                        parts.insert(part_num, body.to_vec());
                        return (StatusCode::OK, [("etag", format!("\"etag-{part_num}\""))])
                            .into_response();
                    }
                    return StatusCode::NOT_FOUND.into_response();
                }
                if body.len() > 16 * 1024 * 1024 {
                    return (StatusCode::BAD_REQUEST, "chunk too big").into_response();
                }
                state.objects.write().await.insert(key, body.to_vec());
                StatusCode::OK.into_response()
            }
        };

        let s = state.clone();
        let get_handler = move |AxumPath((_, key)): AxumPath<(String, String)>| {
            let state = s.clone();
            async move {
                match state.objects.read().await.get(&key) {
                    Some(data) => (StatusCode::OK, data.clone()).into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        };

        let s = state.clone();
        let post_handler = move |AxumPath((_, key)): AxumPath<(String, String)>,
                                 query: axum::extract::Query<HashMap<String, String>>,
                                 body: axum::body::Bytes| {
            let state = s.clone();
            async move {
                if query.contains_key("uploads") {
                    use std::sync::atomic::{AtomicU64, Ordering};
                    static CTR: AtomicU64 = AtomicU64::new(0);
                    let uid = format!("upload-{}", CTR.fetch_add(1, Ordering::Relaxed));
                    state
                        .multipart_uploads
                        .write()
                        .await
                        .insert(uid.clone(), (key.clone(), HashMap::new()));
                    return xml_response(
                        200,
                        format!(
                            "<InitiateMultipartUploadResult><UploadId>{uid}</UploadId></InitiateMultipartUploadResult>"
                        ),
                    );
                }
                if let Some(uid) = query.get("uploadId") {
                    let mut uploads = state.multipart_uploads.write().await;
                    if let Some((stored_key, parts)) = uploads.remove(uid) {
                        let listed = String::from_utf8_lossy(&body);
                        let all_listed = parts.keys().all(|n| {
                            listed.contains(&format!(
                                "<Part><PartNumber>{n}</PartNumber><ETag>&quot;etag-{n}&quot;</ETag></Part>"
                            ))
                        });
                        if !all_listed {
                            return s3_error(400, "InvalidPart", "part list mismatch");
                        }
                        let mut sorted: Vec<_> = parts.into_iter().collect();
                        sorted.sort_by_key(|(n, _)| *n);
                        let combined: Vec<u8> = sorted.into_iter().flat_map(|(_, d)| d).collect();
                        state.objects.write().await.insert(stored_key, combined);
                        return xml_response(200, "<CompleteMultipartUploadResult/>".into());
                    }
                    return StatusCode::NOT_FOUND.into_response();
                }
                let _ = body;
                StatusCode::BAD_REQUEST.into_response()
            }
        };

        let s = state.clone();
        let delete_handler =
            move |AxumPath((_, key)): AxumPath<(String, String)>,
                  query: axum::extract::Query<HashMap<String, String>>| {
                let state = s.clone();
                async move {
                    if let Some(uid) = query.get("uploadId") {
                        state.multipart_uploads.write().await.remove(uid);
                    } else {
                        state.objects.write().await.remove(&key);
                    }
                    StatusCode::NO_CONTENT
                }
            };

        let router = Router::new()
            .route(
                "/{bucket}/{*key}",
                head(head_handler)
                    .put(put_handler)
                    .get(get_handler)
                    .post(post_handler)
                    .delete(delete_handler),
            )
            .layer(axum::extract::DefaultBodyLimit::max(
                2 * MULTIPART_PART_SIZE,
            ))
            .layer(axum::middleware::from_fn(verify_sigv4));
        (router, state)
    }

    /// Start the mock server and return (endpoint_url, state).
    async fn start_mock_server() -> (String, Arc<MockS3State>) {
        let (router, state) = mock_s3_router();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), state)
    }

    async fn make_test_client(endpoint_url: &str) -> S3StorageClient {
        S3StorageClient::new(
            "test-bucket".to_string(),
            "us-east-1",
            Some(r#"{"aws_access_key_id":"test","aws_secret_access_key":"test"}"#),
            None,
            Some(endpoint_url),
        )
        .await
        .unwrap()
    }

    /// Start a mock server that rejects PUT for keys containing "fail".
    async fn start_mock_server_rejecting_fail_keys() -> String {
        let put_handler = move |AxumPath((_, key)): AxumPath<(String, String)>,
                                _body: axum::body::Bytes| async move {
            if key.contains("fail") {
                StatusCode::FORBIDDEN.into_response()
            } else {
                StatusCode::OK.into_response()
            }
        };

        let router = Router::new().route("/{bucket}/{*key}", put(put_handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn unwrap_found(r: crate::storage_client::ExistsResult<HashSet<String>>) -> HashSet<String> {
        match r {
            crate::storage_client::ExistsResult::Found(s) => s,
            other => panic!("expected Found, got {:?}", other),
>>>>>>> origin/master
        }
    }

    #[tokio::test]
    async fn presign_put_url_signs_a_path_style_url_for_a_custom_endpoint() {
        let url = presign_put_url(
            "us-east-1",
            Some("https://s3.example.com"),
            &creds(),
            "bucket",
            "dir/video.mp4",
            "video/mp4",
            std::time::Duration::from_secs(600),
        )
        .await
        .expect("presign must not need a network");
        assert!(
            url.starts_with("https://s3.example.com/bucket/dir/video.mp4?"),
            "path-style URL expected: {url}"
        );
        assert!(url.contains("X-Amz-Signature="), "unsigned URL: {url}");
        assert!(url.contains("AKIDPRESIGN"), "credential missing: {url}");
        assert!(url.contains("X-Amz-Expires=600"), "expiry missing: {url}");
    }

    #[tokio::test]
    async fn presign_get_url_signs_a_url() {
        let url = presign_get_url(
            "us-east-1",
            Some("https://s3.example.com"),
            &creds(),
            "bucket",
            "video.mp4",
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("presign must not need a network");
        assert!(
            url.starts_with("https://s3.example.com/bucket/video.mp4?"),
            "{url}"
        );
        assert!(url.contains("X-Amz-Signature="), "unsigned URL: {url}");
    }

    #[test]
    fn debug_redacts_the_credentials() {
        let shown = format!("{:?}", creds());
        assert!(!shown.contains("\"secret\""), "{shown}");
        assert!(!shown.contains("AKIDPRESIGN"), "{shown}");
    }

    const TEST_CREDS: &str = r#"{"aws_access_key_id":"test","aws_secret_access_key":"test"}"#;

    #[tokio::test]
    async fn upload_bytes_multipart_round_trip() {
        let (endpoint, state) = start_mock_server().await;
        let content: Vec<u8> = (0..MULTIPART_THRESHOLD + 1234)
            .map(|i| (i % 251) as u8)
            .collect();
        let url = upload_bytes(
            "test-bucket",
            "dir/multi part.bin",
            &content,
            "application/octet-stream",
            "us-east-1",
            Some(TEST_CREDS),
            None,
            Some(&endpoint),
        )
        .await
        .unwrap();
        assert_eq!(url, "s3://test-bucket/dir/multi part.bin");
        let objects = state.objects.read().await;
        assert_eq!(objects.get("dir/multi part.bin").unwrap(), &content);
        assert!(state.multipart_uploads.read().await.is_empty());
    }

    #[tokio::test]
    async fn upload_file_sends_the_file() {
        let (endpoint, state) = start_mock_server().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, b"file body").unwrap();
        upload_file(
            "test-bucket",
            "f.txt",
            &path,
            "text/plain",
            "us-east-1",
            Some(TEST_CREDS),
            None,
            Some(&endpoint),
        )
        .await
        .unwrap();
        assert_eq!(
            state.objects.read().await.get("f.txt").unwrap(),
            b"file body"
        );
    }

    #[tokio::test]
    async fn error_response_carries_the_s3_code_and_message() {
        let put_handler = |_: AxumPath<(String, String)>| async {
            s3_error(403, "AccessDenied", "Access Denied")
        };
        let router = Router::new().route("/{bucket}/{*key}", put(put_handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let err = upload_bytes(
            "test-bucket",
            "k",
            b"x",
            "text/plain",
            "us-east-1",
            Some(TEST_CREDS),
            None,
            Some(&format!("http://{addr}")),
        )
        .await
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("HTTP 403"), "{text}");
        assert!(text.contains("AccessDenied"), "{text}");
        assert!(text.contains("Access Denied"), "{text}");
    }

    /// The mock checks signatures, so a wrong secret must be refused.
    #[tokio::test]
    async fn wrong_secret_fails_the_signature_check() {
        let (endpoint, state) = start_mock_server().await;
        let err = upload_bytes(
            "test-bucket",
            "k",
            b"x",
            "text/plain",
            "us-east-1",
            Some(r#"{"aws_access_key_id":"test","aws_secret_access_key":"wrong"}"#),
            None,
            Some(&endpoint),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("SignatureDoesNotMatch"),
            "{err:#}"
        );
        assert!(state.objects.read().await.is_empty());
    }

    #[tokio::test]
    async fn head_reports_the_content_length_through_the_signed_mock() {
        let (endpoint, state) = start_mock_server().await;
        state
            .objects
            .write()
            .await
            .insert("a/b c.txt".into(), b"12345".to_vec());
        let client = make_test_client(&endpoint).await;
        let resp = client.check_exists("a/b c.txt").await.unwrap();
        assert_eq!(resp.size, 5);
    }

    #[tokio::test]
    async fn presign_get_url_points_at_the_endpoint() {
        let creds = S3StaticCredentials {
            access_key_id: "AK".into(),
            secret_access_key: "SK".into(),
        };
        let url = presign_get_url(
            "us-east-1",
            Some("https://s3.example.com"),
            &creds,
            "bucket",
            "videos/out.mp4",
            std::time::Duration::from_secs(300),
        )
        .await
        .unwrap();
        assert!(
            url.starts_with("https://s3.example.com/bucket/videos/out.mp4?"),
            "{url}"
        );
        assert!(url.contains("X-Amz-SignedHeaders=host"), "{url}");
        assert!(url.contains("X-Amz-Signature="), "{url}");
    }
}
