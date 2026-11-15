//! Embedding provider abstraction for memory vector search.
//!
//! The sqlite-vec `chunks_vec` virtual table is the embedding cache; there is no separate cache.

use async_trait::async_trait;

/// Maximum retry attempts for transient API errors (429, 5xx).
const MAX_RETRIES: usize = 3;
/// Initial backoff delay in milliseconds (doubles on each retry: 1s, 2s, 4s).
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Generates text embeddings.
/// The `Send + Sync` bound lets implementations run inside `Send` futures such as `tokio::spawn`.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts, returning one vector per input text.
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>>;

    fn model_name(&self) -> &str;

    fn dimensions(&self) -> usize;
}

/// API-based embedding provider using an OpenAI-compatible embeddings endpoint.
/// Each request goes through a [`xai_grok_sampler::SamplingClient`], which takes a request slot
/// and bills the call to `usage_sink`.
pub struct ApiEmbeddingProvider {
    api_base: String,
    model: String,
    dimensions: usize,
    auth: xai_grok_net::Auth,
    usage_sink: xai_grok_sampler::SharedUsageSink,
    max_batch_size: usize,
}

impl ApiEmbeddingProvider {
    pub fn new(
        api_base: String,
        model: String,
        dimensions: usize,
        auth: xai_grok_net::Auth,
        usage_sink: xai_grok_sampler::SharedUsageSink,
    ) -> Self {
        Self {
            api_base,
            model,
            dimensions,
            auth,
            usage_sink,
            max_batch_size: 32,
        }
    }

    pub fn from_config(
        config: &xai_grok_config_types::MemoryEmbeddingConfig,
        api_base: String,
        auth: xai_grok_net::Auth,
        usage_sink: xai_grok_sampler::SharedUsageSink,
    ) -> Option<Self> {
        let model = config.model.clone().filter(|m| !m.is_empty())?;
        Some(Self::new(
            api_base,
            model,
            config.dimensions,
            auth,
            usage_sink,
        ))
    }

    /// `auth_key` is a key for the endpoint, sent as `Authorization: Bearer`.
    pub fn from_session(
        config: &xai_grok_config_types::MemoryEmbeddingConfig,
        proxy_base_url: String,
        auth_key: String,
        usage_sink: xai_grok_sampler::SharedUsageSink,
    ) -> Option<Self> {
        let auth = xai_grok_net::Auth::ApiKey {
            key: auth_key,
            scheme: xai_grok_net::KeyScheme::Bearer,
        };
        Self::from_config(config, proxy_base_url, auth, usage_sink)
    }

    fn sampling_client(
        &self,
    ) -> Result<xai_grok_sampler::SamplingClient, Box<dyn std::error::Error>> {
        let mut extra_headers = indexmap::IndexMap::new();
        extra_headers.insert("X-XAI-Token-Auth".to_owned(), "xai-grok-cli".to_owned());
        let config = xai_grok_sampler::SamplerConfig {
            base_url: self.api_base.clone(),
            model: self.model.clone(),
            extra_headers,
            client_version: Some(xai_grok_version::version().to_owned()),
            usage_sink: Some(self.usage_sink.clone()),
            ..Default::default()
        };
        let client = xai_grok_sampler::SamplingClient::new(config)
            .map_err(|e| format!("embedding client: {e}"))?;
        Ok(client.with_request_auth(self.auth.clone()))
    }
}

pub(super) fn retrying_auth(
    credentials: std::sync::Arc<dyn xai_grok_auth::AuthCredentialProvider>,
    base_url: &str,
) -> xai_grok_net::Auth {
    xai_grok_http::auth_retry(credentials, base_url).auth
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    #[tracing::instrument(name = "memory.embed_batch", skip_all, fields(batch_size = texts.len()))]
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        let endpoint = format!("{}/embeddings", self.api_base);
        xai_grok_http::endpoint_allowlist::check(&endpoint)
            .map_err(|refusal| refusal.to_string())?;
        let client = self.sampling_client()?;
        // Process in batches to respect API payload limits
        for batch in texts.chunks(self.max_batch_size) {
            let input: Vec<&str> = batch.to_vec();
            let body_json = serde_json::json!({
                "model": self.model,
                "input": input,
                "dimensions": self.dimensions,
            });

            // Transient errors (429, 5xx) are retried with exponential backoff
            let mut last_err = String::new();
            let mut success = false;
            for attempt in 0..MAX_RETRIES {
                if attempt > 0 {
                    let delay = INITIAL_BACKOFF_MS * 2u64.pow(attempt as u32 - 1);
                    tracing::warn!(
                        attempt,
                        delay_ms = delay,
                        "retrying embedding API call after transient error"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }

                let response = match client.create_embeddings(&body_json).await {
                    Ok(r) => r,
                    // A refusal sent nothing, and a retry is refused the same way.
                    Err(xai_grok_sampling_types::SamplingError::Http(e))
                        if e.auth_refusal().is_some() =>
                    {
                        return Err(format!("embedding request refused: {e}").into());
                    }
                    Err(e) => {
                        last_err = format!("request failed: {e}");
                        continue;
                    }
                };

                let status = response.status;
                if status.is_success() {
                    let body: serde_json::Value = serde_json::from_slice(&response.body)?;
                    let data = body
                        .get("data")
                        .and_then(|d| d.as_array())
                        .ok_or("embedding response missing 'data' array")?;

                    for item in data {
                        let embedding: Vec<f32> = item
                            .get("embedding")
                            .and_then(|e| e.as_array())
                            .ok_or("embedding item missing 'embedding' array")?
                            .iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect();
                        all_embeddings.push(embedding);
                    }
                    success = true;
                    break;
                }

                // Retry on 429 (rate limit) or 5xx (server error)
                if status == xai_grok_net::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                {
                    last_err = format!("HTTP {status}: {}", response.text());
                    continue;
                }

                // Any other status is not retryable, so fail immediately
                let body = response.text();
                return Err(format!("embedding API error {status}: {body}").into());
            }

            if !success {
                return Err(format!(
                    "embedding API failed after {MAX_RETRIES} attempts: {last_err}"
                )
                .into());
            }
        }

        Ok(all_embeddings)
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

/// A mock provider for tests: each vector is derived from the blake3 hash of the text, so results are deterministic.
#[cfg(any(test, feature = "test-support"))]
pub struct MockEmbeddingProvider {
    pub dimensions: usize,
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        Ok(texts
            .iter()
            .map(|text| {
                let hash = blake3::hash(text.as_bytes());
                let bytes = hash.as_bytes();
                bytes
                    .iter()
                    .cycle()
                    .take(self.dimensions)
                    .map(|&b| b as f32 / 255.0)
                    .collect()
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "mock-embedding"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Recorder {
        calls: parking_lot::Mutex<Vec<xai_grok_sampler::BilledCall>>,
    }

    impl xai_grok_sampler::UsageSink for Recorder {
        fn record_call(&self, call: xai_grok_sampler::BilledCall) {
            self.calls.lock().push(call);
        }
    }

    /// Each batch goes through the sampling client, which bills the call's token usage.
    #[tokio::test]
    async fn embed_batch_bills_each_call_to_the_sink() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/embeddings"))
            .and(wiremock::matchers::header("authorization", "Bearer key"))
            .and(wiremock::matchers::header(
                "x-xai-token-auth",
                "xai-grok-cli",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "model": "served-embed",
                    "data": [{ "embedding": [0.5, 0.25] }],
                    "usage": { "prompt_tokens": 5, "total_tokens": 5 }
                })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let recorder = std::sync::Arc::new(Recorder::default());
        let provider = ApiEmbeddingProvider::new(
            format!("{}/v1", server.uri()),
            "embed-model".into(),
            2,
            xai_grok_net::Auth::ApiKey {
                key: "key".into(),
                scheme: xai_grok_net::KeyScheme::Bearer,
            },
            recorder.clone(),
        );

        let vectors = provider.embed_batch(&["hello"]).await.expect("embeddings");

        assert_eq!(vectors, vec![vec![0.5, 0.25]]);
        let calls = recorder.calls.lock().clone();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let call = calls.first().expect("one call");
        assert_eq!(call.model, "served-embed");
        assert_eq!(call.usage.as_ref().map(|u| u.prompt_tokens), Some(5));
    }

    #[tokio::test]
    async fn test_mock_embedding_deterministic() {
        let provider = MockEmbeddingProvider { dimensions: 4 };
        let r1 = provider.embed_batch(&["hello"]).await.unwrap();
        let r2 = provider.embed_batch(&["hello"]).await.unwrap();
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn test_mock_embedding_different_texts() {
        let provider = MockEmbeddingProvider { dimensions: 4 };
        let results = provider.embed_batch(&["hello", "world"]).await.unwrap();
        let [a, b] = results.as_slice() else {
            panic!("expected two embeddings: {results:?}");
        };
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn test_mock_embedding_empty_input() {
        let provider = MockEmbeddingProvider { dimensions: 4 };
        let results = provider.embed_batch(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_mock_embedding_correct_dimensions() {
        let provider = MockEmbeddingProvider { dimensions: 128 };
        let results = provider.embed_batch(&["test"]).await.unwrap();
        assert_eq!(results.first().map(Vec::len), Some(128));
    }
}
