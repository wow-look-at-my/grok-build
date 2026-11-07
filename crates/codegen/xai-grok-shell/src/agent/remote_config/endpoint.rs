//! HTTP binding for the `/v1/models` catalog endpoint.

use std::future::Future;
use std::pin::Pin;

use indexmap::IndexMap;

use super::{ModelFetchAuth, prefetch_models_blocking};
use crate::agent::config::{self, ModelEntry};
use xai_grok_login::GrokAuth;

/// Boxed future returned by [`ModelsEndpoint::fetch_models`].
pub(crate) type ModelsFetchFuture =
    Pin<Box<dyn Future<Output = Option<IndexMap<String, ModelEntry>>> + Send>>;

/// The `/v1/models` fetch behind a trait so tests can inject a fake.
pub(crate) trait ModelsEndpoint: Send + Sync {
    fn fetch_models(
        &self,
        endpoints: config::EndpointsConfig,
        auth: Option<GrokAuth>,
        fetch_auth: ModelFetchAuth,
    ) -> ModelsFetchFuture;
}

/// The default implementation: the real `/v1/models` fetch.
pub(crate) struct HttpModelsEndpoint;

impl ModelsEndpoint for HttpModelsEndpoint {
    fn fetch_models(
        &self,
        endpoints: config::EndpointsConfig,
        auth: Option<GrokAuth>,
        fetch_auth: ModelFetchAuth,
    ) -> ModelsFetchFuture {
        Box::pin(fetch_models_async(endpoints, auth, fetch_auth))
    }
}

pub(crate) async fn fetch_models_async(
    endpoints: config::EndpointsConfig,
    auth: Option<GrokAuth>,
    fetch_auth: ModelFetchAuth,
) -> Option<IndexMap<String, ModelEntry>> {
    let queued_at = std::time::Instant::now();
    xai_spawn::spawn_blocking(move || {
        // The caller's timeout also counts the wait for a blocking-pool thread.
        let queued = queued_at.elapsed();
        if queued >= QUEUE_WAIT_WORTH_LOGGING {
            xai_grok_telemetry::unified_log::warn(
                "model catalog: the fetch waited for a blocking thread",
                None,
                Some(serde_json::json!({ "queued_ms": queued.as_millis() as u64 })),
            );
        }
        prefetch_models_blocking(&endpoints, auth.as_ref(), fetch_auth)
    })
    .await
    .unwrap_or(None)
}

/// A wait for a blocking-pool thread this long uses a large part of the fetch timeout.
const QUEUE_WAIT_WORTH_LOGGING: std::time::Duration = std::time::Duration::from_millis(500);
