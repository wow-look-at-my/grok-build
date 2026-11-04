//! Shared upload utilities for session persistence and agent telemetry.
//!
//! Every upload goes through the cli-chat-proxy storage API.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

use crate::UploadMethod;
use xai_grok_auth::{AuthCredentialProvider, StaticAuthCredentialProvider};

use crate::storage_client::{
    Auth401AttributionCallback, ExistsResult, StaticGrokAuth, StorageClient, UploadResponse,
};

/// Threshold for switching to multipart upload (50 MB).
/// Larger files use signed-URL multipart (parts go directly to storage) instead of streaming through the proxy.
pub const MULTIPART_UPLOAD_THRESHOLD: u64 = 50 * 1024 * 1024;

/// Construct a `StorageClient` for proxy-mode uploads.
/// Uses caller-provided refresh-aware credentials, else a `StaticGrokAuth` from the inline user token.
/// Optional `http_client` lets the caller pass a shell-tuned client; `None` falls back to a default client.
fn build_proxy_client_with_fallback(
    proxy_base_url: &str,
    user_token: &str,
    credentials: Option<Arc<dyn AuthCredentialProvider>>,
    attribution: Option<Arc<dyn Auth401AttributionCallback>>,
    http_client: Option<xai_grok_net::HttpClient>,
) -> StorageClient {
    let provider = credentials.unwrap_or_else(|| {
        let creds = StaticGrokAuth::new(Some(user_token.to_owned()));
        let bearer = creds.wire_bearer();
        Arc::new(StaticAuthCredentialProvider::new(Box::new(creds), bearer))
    });
    let http_client = http_client.unwrap_or_else(crate::storage_client::default_upload_client);
    let mut client = StorageClient::with_provider(proxy_base_url, http_client, provider);
    if let Some(cb) = attribution {
        client = client.with_attribution(cb);
    }
    client
}

/// Implement `StorageConfig` for `TraceExportConfig` here so callers can use shared upload helpers.
/// Refresh-aware callers still get credential wiring via `TraceExportConfigWithAuth` (in shell).
impl StorageConfig for crate::TraceExportConfig {
    fn upload_method(&self) -> &UploadMethod {
        &self.upload_method
    }
}

/// A trait for storage configuration that provides the upload method.
/// This allows different config types (TraceExportConfig, etc.) to share upload logic.
pub trait StorageConfig {
    fn upload_method(&self) -> &UploadMethod;
    /// Optional refresh-aware credentials for proxy-mode uploads.
    /// When `Some`, upload helpers use `StorageClient::with_provider` so 401 retries can refresh.
    /// Default `None` for configs that ship a static user-token only.
    fn proxy_credentials(&self) -> Option<Arc<dyn AuthCredentialProvider>> {
        None
    }
    /// Optional 401-attribution callback. When `Some(_)`, the constructed
    /// `StorageClient` also calls `with_attribution(...)` so the embedding
    /// application records auth-attribution telemetry for proxy 401s.
    fn proxy_attribution(&self) -> Option<Arc<dyn Auth401AttributionCallback>> {
        None
    }
    /// Optional HTTP client for proxy-mode uploads. `None` falls back to a default client.
    /// Production should return the shell-tuned client (HTTP/2 keep-alive, aggressive pool eviction).
    /// Upload queues rely on that tuning to avoid stale-connection retries during backoff.
    fn proxy_http_client(&self) -> Option<xai_grok_net::HttpClient> {
        None
    }
}

/// Uploads bytes to cloud storage at the specified path.
/// Returns the full storage URL on success.
pub async fn upload_bytes<C: StorageConfig>(
    config: &C,
    object_path: &str,
    content: &[u8],
    content_type: &str,
) -> anyhow::Result<String> {
    match config.upload_method() {
        UploadMethod::Proxy {
            proxy_base_url,
            user_token,
            alpha_test_key: _,
        } => {
            // For proxy mode, bucket is determined by proxy from user ACLs
            tracing::debug!(
                proxy_base_url = %proxy_base_url,
                object_path = %object_path,
                "Uploading bytes to GCS via proxy (bucket determined by proxy from ACLs)"
            );
            upload_bytes_via_proxy(
                proxy_base_url,
                user_token,
                object_path,
                content,
                content_type,
                config.proxy_credentials(),
                config.proxy_attribution(),
                config.proxy_http_client(),
            )
            .await
        }
    }
}

fn proxy_storage_client<C: StorageConfig>(config: &C) -> StorageClient {
    match config.upload_method() {
        UploadMethod::Proxy {
            proxy_base_url,
            user_token,
            alpha_test_key: _,
        } => build_proxy_client_with_fallback(
            proxy_base_url,
            user_token,
            config.proxy_credentials(),
            config.proxy_attribution(),
            config.proxy_http_client(),
        ),
    }
}

/// Existing paths among `paths` in one round trip (keep it under 100 paths: the proxy sub-batches
/// sub-batches sub-batches at a set value and reports a failed sub-batch as missing). No retries.
pub async fn batch_check_exists<C: StorageConfig, S: AsRef<str>>(
    config: &C,
    paths: &[S],
) -> ExistsResult<HashSet<String>> {
    proxy_storage_client(config).batch_check_exists(paths).await
}

/// Single-object existence probe through the proxy.
pub async fn check_exists<C: StorageConfig>(
    config: &C,
    object_path: &str,
) -> ExistsResult<UploadResponse> {
    proxy_storage_client(config).check_exists(object_path).await
}

/// Like [`upload_bytes`], but uses a pre-signed PUT so data bypasses the proxy. Avoids nginx
/// Avoids nginx `proxy-body-size` and the Cloudflare 100 MB limit for arbitrarily large payloads.
pub async fn upload_bytes_signed<C: StorageConfig>(
    config: &C,
    object_path: &str,
    content: &[u8],
    content_type: &str,
) -> anyhow::Result<String> {
    match config.upload_method() {
        UploadMethod::Proxy {
            proxy_base_url,
            user_token,
            alpha_test_key: _,
        } => {
            tracing::debug!(
                proxy_base_url = %proxy_base_url,
                object_path = %object_path,
                bytes = content.len(),
                "Uploading bytes to GCS via signed URL (bypasses proxy body limits)"
            );
            upload_bytes_via_signed_url(
                proxy_base_url,
                user_token,
                object_path,
                content,
                content_type,
                config.proxy_credentials(),
                config.proxy_attribution(),
                config.proxy_http_client(),
            )
            .await
        }
    }
}

/// Uploads a file by streaming from disk. Preferred for the background queue: never loads the full file.
/// Large files use signed-URL multipart (bypass body limits); small files stream through the proxy.
pub async fn upload_file<C: StorageConfig>(
    config: &C,
    object_path: &str,
    file_path: &Path,
    content_type: &str,
) -> anyhow::Result<String> {
    match config.upload_method() {
        UploadMethod::Proxy {
            proxy_base_url,
            user_token,
            alpha_test_key: _,
        } => {
            upload_file_via_proxy(
                proxy_base_url,
                user_token,
                object_path,
                file_path,
                content_type,
                config.proxy_credentials(),
                config.proxy_attribution(),
                config.proxy_http_client(),
            )
            .await
        }
    }
}

/// Uploads an async reader through the proxy.
/// The reader is consumed once per attempt; callers handle retries by recreating it.
pub async fn upload_stream<C: StorageConfig, R>(
    config: &C,
    object_path: &str,
    reader: R,
    content_type: &str,
) -> anyhow::Result<String>
where
    R: tokio::io::AsyncRead + Send + Sync + 'static,
{
    match config.upload_method() {
        UploadMethod::Proxy {
            proxy_base_url,
            user_token,
            alpha_test_key: _,
        } => {
            let storage_client = build_proxy_client_with_fallback(
                proxy_base_url,
                user_token,
                config.proxy_credentials(),
                config.proxy_attribution(),
                config.proxy_http_client(),
            );
            let response = storage_client
                .upload_stream(object_path, reader, content_type)
                .await
                .with_context(|| format!("Streaming upload failed for {}", object_path))?;
            Ok(format!("gs://{}/{}", response.bucket, response.path))
        }
    }
}

/// Upload a file through the cli-chat-proxy, choosing multipart vs streaming based on size.
/// Files above `MULTIPART_UPLOAD_THRESHOLD` use signed-URL multipart so parts skip the proxy body.
/// Avoids the proxy body-size limit and the timeouts that dominate large-blob failures.
async fn upload_file_via_proxy(
    proxy_base_url: &str,
    user_token: &str,
    object_path: &str,
    file_path: &Path,
    content_type: &str,
    credentials: Option<Arc<dyn AuthCredentialProvider>>,
    attribution: Option<Arc<dyn Auth401AttributionCallback>>,
    http_client: Option<xai_grok_net::HttpClient>,
) -> anyhow::Result<String> {
    use crate::storage_client::{MultipartUploadOptions, RetryConfig};

    let storage_client = build_proxy_client_with_fallback(
        proxy_base_url,
        user_token,
        credentials,
        attribution,
        http_client,
    )
    .with_retry_config(RetryConfig::conservative());

    let file_size = tokio::fs::metadata(file_path)
        .await
        .with_context(|| format!("Failed to get file metadata: {}", file_path.display()))?
        .len();

    if file_size > MULTIPART_UPLOAD_THRESHOLD {
        // Large file: upload directly to cloud storage via signed URLs (bypasses proxy body)
        tracing::info!(
            file_size,
            threshold = MULTIPART_UPLOAD_THRESHOLD,
            upload_method = "multipart",
            path = %file_path.display(),
            "Upload queue: using multipart for large file"
        );
        let options = MultipartUploadOptions::new().with_max_concurrent(4);
        let response = storage_client
            .upload_multipart(object_path, file_path, content_type, Some(options))
            .await
            .with_context(|| format!("Multipart upload failed for {}", object_path))?;
        Ok(response.gcs_url)
    } else {
        // Small file: stream through proxy (no memory copy)
        tracing::debug!(
            file_size,
            upload_method = "streaming",
            path = %file_path.display(),
            "Upload queue: using streaming for small file"
        );
        let response = storage_client
            .upload_file(object_path, file_path, content_type)
            .await
            .with_context(|| format!("Streaming upload failed for {}", object_path))?;
        Ok(format!("gs://{}/{}", response.bucket, response.path))
    }
}

/// Uploads bytes via the cli-chat-proxy storage proxy API.
/// The bucket is determined by the proxy based on the user's ACLs.
async fn upload_bytes_via_proxy(
    proxy_base_url: &str,
    user_token: &str,
    object_path: &str,
    content: &[u8],
    content_type: &str,
    credentials: Option<Arc<dyn AuthCredentialProvider>>,
    attribution: Option<Arc<dyn Auth401AttributionCallback>>,
    http_client: Option<xai_grok_net::HttpClient>,
) -> anyhow::Result<String> {
    use crate::storage_client::RetryConfig;

    // Conservative retry config handles storage-backend 429 errors during autoscaling.
    let storage_client = build_proxy_client_with_fallback(
        proxy_base_url,
        user_token,
        credentials,
        attribution,
        http_client,
    )
    .with_retry_config(RetryConfig::conservative());

    let response = storage_client
        .upload(object_path, content, content_type)
        .await
        .with_context(|| {
            format!(
                "Failed to upload to storage proxy: {} (path: {})",
                proxy_base_url, object_path
            )
        })?;

    // Return the full GCS URL
    Ok(format!("gs://{}/{}", response.bucket, response.path))
}

/// Uploads bytes via a pre-signed PUT URL from the proxy. Data bypasses the proxy entirely.
/// The proxy is contacted once to mint the URL. Use when the payload may exceed the 4 MB ingress limit.
pub async fn upload_bytes_via_signed_url(
    proxy_base_url: &str,
    user_token: &str,
    object_path: &str,
    content: &[u8],
    content_type: &str,
    credentials: Option<Arc<dyn AuthCredentialProvider>>,
    attribution: Option<Arc<dyn Auth401AttributionCallback>>,
    http_client: Option<xai_grok_net::HttpClient>,
) -> anyhow::Result<String> {
    let storage_client = build_proxy_client_with_fallback(
        proxy_base_url,
        user_token,
        credentials,
        attribution,
        http_client,
    );

    let signed = storage_client
        .upload_bytes_signed(object_path, content, content_type)
        .await
        .with_context(|| {
            format!(
                "Failed to upload via signed URL: {} (path: {})",
                proxy_base_url, object_path
            )
        })?;

    Ok(format!("gs://{}/{}", signed.bucket, signed.path))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TraceExportConfig, UploadMethod};

    fn proxy_config() -> TraceExportConfig {
        proxy_config_with_url("https://proxy.example.com/v1".to_string())
    }

    fn proxy_config_with_url(base_url: String) -> TraceExportConfig {
        TraceExportConfig {
            bucket_url: None,
            upload_method: UploadMethod::Proxy {
                proxy_base_url: base_url,
                user_token: "tok".to_string(),
                alpha_test_key: None,
            },
            prefix_dir: None,
            gcs_prefix: None,
            absolute_paths: false,
            archive_name_override: None,
        }
    }

    #[test]
    fn multipart_threshold_is_50mb() {
        assert_eq!(
            MULTIPART_UPLOAD_THRESHOLD,
            50 * 1024 * 1024,
            "Multipart threshold must be 50 MB to match the plan and repo_changes.rs"
        );
    }

    #[tokio::test]
    async fn upload_file_proxy_missing_file_returns_error() {
        // upload_file_via_proxy checks metadata before connecting — should fail
        // fast with a descriptive error if the temp file was deleted mid-flight.
        let config = proxy_config();
        let result = upload_file(
            &config,
            "session/turn_0/test.bin",
            std::path::Path::new("/tmp/nonexistent_upload_queue_test_file"),
            "application/octet-stream",
        )
        .await;
        assert!(result.is_err(), "Should error for missing file");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("metadata") || err.contains("No such file"),
            "Error should mention file metadata: {}",
            err
        );
    }

    /// Shared state for the dispatch test server, tracking which endpoints were hit.
    #[derive(Clone, Default)]
    struct DispatchState {
        multipart_called: std::sync::Arc<std::sync::atomic::AtomicBool>,
        storage_called: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    /// Start a minimal axum server that records which upload routes were hit.
    /// Same State extractor pattern as storage_client_tests so flag updates are reliable in Bazel CI.
    async fn start_dispatch_test_server() -> (std::net::SocketAddr, DispatchState) {
        use axum::{
            Router, body::Body, extract::State, http::StatusCode, response::IntoResponse,
            routing::post,
        };
        use std::sync::atomic::Ordering;
        use tokio::net::TcpListener;

        let state = DispatchState::default();

        async fn multipart_handler(
            State(s): State<DispatchState>,
            _body: Body,
        ) -> impl IntoResponse {
            s.multipart_called.store(true, Ordering::SeqCst);
            // 400 = non-retryable: client fails fast without backoff delays
            (StatusCode::BAD_REQUEST, r#"{"error":"test"}"#)
        }

        async fn storage_handler(State(s): State<DispatchState>, _body: Body) -> impl IntoResponse {
            s.storage_called.store(true, Ordering::SeqCst);
            (StatusCode::BAD_REQUEST, r#"{"error":"test"}"#)
        }

        let app = Router::new()
            .route("/v1/storage/multipart/init", post(multipart_handler))
            .route("/v1/storage", post(storage_handler))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        xai_spawn::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Give the server 50ms to bind and accept — more headroom for Bazel CI sandboxing.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        (addr, state)
    }

    #[tokio::test]
    async fn upload_file_via_proxy_uses_multipart_for_large_files() {
        // Large file (just over 50 MB threshold) should hit the multipart init endpoint.
        // Uses set_len() to create a sparse file — no actual disk write.
        let (addr, state) = start_dispatch_test_server().await;
        let config = proxy_config_with_url(format!("http://{}/v1", addr));

        let temp = tempfile::TempDir::new().unwrap();
        let large_file = temp.path().join("large.bin");
        let f = std::fs::File::create(&large_file).unwrap();
        f.set_len(MULTIPART_UPLOAD_THRESHOLD + 1).unwrap(); // sparse file, no actual disk write

        let _ = upload_file(
            &config,
            "session/turn_0/large.bin",
            &large_file,
            "application/octet-stream",
        )
        .await;

        assert!(
            state
                .multipart_called
                .load(std::sync::atomic::Ordering::SeqCst),
            "File > 50MB should use multipart upload"
        );
        assert!(
            !state
                .storage_called
                .load(std::sync::atomic::Ordering::SeqCst),
            "File > 50MB should NOT use the simple storage endpoint"
        );
    }

    #[tokio::test]
    async fn upload_file_via_proxy_uses_streaming_for_small_files() {
        // Small file (1 KB) should hit the simple storage endpoint, not multipart.
        let (addr, state) = start_dispatch_test_server().await;
        let config = proxy_config_with_url(format!("http://{}/v1", addr));

        let temp = tempfile::TempDir::new().unwrap();
        let small_file = temp.path().join("small.bin");
        std::fs::write(&small_file, vec![0u8; 1024]).unwrap();

        let _ = upload_file(
            &config,
            "session/turn_0/small.bin",
            &small_file,
            "application/octet-stream",
        )
        .await;

        assert!(
            !state
                .multipart_called
                .load(std::sync::atomic::Ordering::SeqCst),
            "File < 50MB should NOT use multipart upload"
        );
        assert!(
            state
                .storage_called
                .load(std::sync::atomic::Ordering::SeqCst),
            "File < 50MB should use the simple storage endpoint"
        );
    }
}
