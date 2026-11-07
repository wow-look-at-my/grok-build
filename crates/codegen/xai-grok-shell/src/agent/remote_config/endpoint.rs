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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, Ordering};
    const QUEUED: u8 = 0;
    const RUNNING: u8 = 1;
    const DONE: u8 = 2;

    let queued_at = std::time::Instant::now();
    let stage = Arc::new(AtomicU8::new(QUEUED));
    // A fetch that has not returned by now reports which stage holds it.
    let watchdog = {
        let stage = Arc::clone(&stage);
        xai_spawn::spawn(async move {
            tokio::time::sleep(STALL_REPORT_AFTER).await;
            let stage = match stage.load(Ordering::Acquire) {
                QUEUED => "waiting for a blocking-pool thread",
                RUNNING => "running on a blocking thread",
                _ => return,
            };
            xai_grok_telemetry::unified_log::warn(
                "model catalog: the fetch has not returned",
                None,
                Some(serde_json::json!({
                    "stage": stage,
                    "elapsed_ms": queued_at.elapsed().as_millis() as u64,
                })),
            );
        })
    };
    let result = xai_spawn::spawn_blocking(move || {
        stage.store(RUNNING, Ordering::Release);
        let result = prefetch_models_blocking(&endpoints, auth.as_ref(), fetch_auth);
        stage.store(DONE, Ordering::Release);
        result
    })
    .await
    .unwrap_or(None);
    watchdog.abort();
    result
}

/// Shorter than the caller's fetch timeout, so the report lands before the caller gives up.
const STALL_REPORT_AFTER: std::time::Duration = std::time::Duration::from_secs(4);
