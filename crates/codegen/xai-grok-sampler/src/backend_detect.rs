//! API-backend autodetection.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use xai_grok_sampling_types::{ApiBackend, SamplingError};

use crate::config::SamplerConfig;
use crate::retry::clone_error;

/// The backends the fallback tries, in this order.
const CANDIDATES: [ApiBackend; 3] = [
    ApiBackend::ChatCompletions,
    ApiBackend::Responses,
    ApiBackend::Messages,
];

type Key = (String, String);

static DETECTED: LazyLock<Mutex<HashMap<Key, ApiBackend>>> = LazyLock::new(Default::default);

#[allow(clippy::disallowed_methods)] // a poisoned map is still a valid map
fn detected() -> MutexGuard<'static, HashMap<Key, ApiBackend>> {
    DETECTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn key(base_url: &str, model: &str) -> Key {
    (base_url.trim_end_matches('/').to_owned(), model.to_owned())
}

/// The backend a request to `model` at `base_url` answered on after a
/// fallback, if one did in this process.
pub fn detected_backend(base_url: &str, model: &str) -> Option<ApiBackend> {
    detected().get(&key(base_url, model)).cloned()
}

pub(crate) fn remember(base_url: &str, model: &str, backend: ApiBackend) {
    tracing::info!(
        base_url,
        model,
        backend = ?backend,
        "API backend detected; later requests to this model start on it"
    );
    detected().insert(key(base_url, model), backend);
}

/// The backend a client built from `config` speaks: the detected one when a
/// fallback found one, else the configured one.
pub fn effective_api_backend(config: &SamplerConfig) -> ApiBackend {
    resolve_api_backend(&config.base_url, &config.model, &config.api_backend)
}

/// [`effective_api_backend`] for callers that hold the parts of a config.
pub fn resolve_api_backend(base_url: &str, model: &str, configured: &ApiBackend) -> ApiBackend {
    if !CANDIDATES.contains(configured) {
        return configured.clone();
    }
    detected_backend(base_url, model).unwrap_or_else(|| configured.clone())
}

/// Whether `err` says the request went to the wrong API for the model.
pub fn is_backend_mismatch(err: &SamplingError) -> bool {
    err.is_unsupported_backend_error() || err.is_endpoint_missing_error()
}

/// One request's walk through the backends.
#[derive(Default)]
pub(crate) struct BackendProbe {
    tried: Vec<ApiBackend>,
    /// The error that started the walk.
    first_error: Option<SamplingError>,
}

impl BackendProbe {
    /// Whether the request left the backend it started on.
    pub(crate) fn switched(&self) -> bool {
        self.first_error.is_some()
    }

    /// Whether `err` sends the request to the next backend. This covers once
    /// the request left its first backend. Any client error other than auth or
    /// a rate limit means that this backend does not serve the model either.
    pub(crate) fn should_switch(&self, err: &SamplingError) -> bool {
        if is_backend_mismatch(err) {
            return true;
        }
        let SamplingError::Api { status, .. } = err else {
            return false;
        };
        self.switched() && status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 429)
    }

    /// Mark `current` as failed with `err` and give the next backend to try.
    /// A backend the error names as the supported one goes first.
    pub(crate) fn next(&mut self, current: &ApiBackend, err: &SamplingError) -> Option<ApiBackend> {
        if !CANDIDATES.contains(current) {
            return None;
        }
        if !self.tried.contains(current) {
            self.tried.push(current.clone());
        }
        if self.first_error.is_none() {
            self.first_error = Some(clone_error(err));
        }
        err.backend_named_as_supported()
            .filter(|named| CANDIDATES.contains(named) && !self.tried.contains(named))
            .or_else(|| {
                CANDIDATES
                    .iter()
                    .find(|backend| !self.tried.contains(backend))
                    .cloned()
            })
    }

    /// The error to report when no backend answered.
    pub(crate) fn exhausted_error(&self, last: &SamplingError) -> SamplingError {
        clone_error(self.first_error.as_ref().unwrap_or(last))
    }
}

#[cfg(test)]
mod tests {
    use xai_grok_net::StatusCode;

    use super::*;

    fn api(status: u16, message: &str) -> SamplingError {
        SamplingError::Api {
            status: StatusCode::from_u16(status).unwrap(),
            message: message.to_owned(),
            model_metadata: None,
            retry_after_secs: None,
            should_retry: None,
            error_code: None,
        }
    }

    #[test]
    fn a_named_backend_goes_first() {
        let mut probe = BackendProbe::default();
        let err = api(400, "Please use /v1/messages for this model.");
        assert_eq!(
            probe.next(&ApiBackend::ChatCompletions, &err),
            Some(ApiBackend::Messages)
        );
    }

    #[test]
    fn every_backend_is_tried_once_then_the_first_error_is_reported() {
        let mut probe = BackendProbe::default();
        let first = api(
            404,
            "This model is not supported in the v1/responses endpoint.",
        );
        assert!(!probe.switched());
        assert_eq!(
            probe.next(&ApiBackend::Responses, &first),
            Some(ApiBackend::ChatCompletions)
        );
        assert!(probe.switched());
        let missing = api(405, "Method Not Allowed");
        assert_eq!(
            probe.next(&ApiBackend::ChatCompletions, &missing),
            Some(ApiBackend::Messages)
        );
        assert_eq!(probe.next(&ApiBackend::Messages, &missing), None);
        assert_eq!(
            probe.exhausted_error(&missing).to_string(),
            first.to_string()
        );
    }

    #[test]
    fn ollama_never_probes() {
        let mut probe = BackendProbe::default();
        let err = api(405, "Method Not Allowed");
        assert_eq!(probe.next(&ApiBackend::Ollama, &err), None);
        assert!(!probe.switched());
    }

    #[test]
    fn a_detected_backend_overrides_the_config_for_that_endpoint_and_model() {
        let config = SamplerConfig {
            base_url: "https://detect.test/v1/".into(),
            model: "detect-model".into(),
            api_backend: ApiBackend::ChatCompletions,
            ..Default::default()
        };
        assert_eq!(effective_api_backend(&config), ApiBackend::ChatCompletions);
        remember(
            "https://detect.test/v1",
            "detect-model",
            ApiBackend::Responses,
        );
        assert_eq!(effective_api_backend(&config), ApiBackend::Responses);

        let other_model = SamplerConfig {
            model: "other".into(),
            ..config.clone()
        };
        assert_eq!(
            effective_api_backend(&other_model),
            ApiBackend::ChatCompletions
        );
    }
}
