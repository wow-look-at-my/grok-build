//! Reads the model list from an OpenAI-compatible `/v1/models`.
use crate::agent::config::EndpointsConfig;
use crate::agent::remote_config::ModelFetchAuth;
use crate::remote::client::{
    BackendError, FetchModelsResult, allow_endpoint, models_list_url_for_base,
    parse_remote_model_value,
};
use crate::remote::model_source::ModelSource;
use serde::Deserialize;
use xai_grok_login::GrokAuth;
use xai_grok_login::backend::{ActiveAuthBackend, AuthBackend};
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<serde_json::Value>,
}
/// Reads `/v1/models` from whichever host the endpoint config resolved to.
pub(crate) struct OaiModelSource {
    endpoint: ListModelsEndpoint,
    inference_base_url: String,
}
impl OaiModelSource {
    pub(crate) fn new(endpoints: &EndpointsConfig, fetch_auth: ModelFetchAuth) -> Self {
        Self {
            endpoint: ListModelsEndpoint::from_endpoints(endpoints, fetch_auth),
            inference_base_url: endpoints.resolve_inference_base_url(),
        }
    }
}
impl ModelSource for OaiModelSource {
    fn cache_origin(&self) -> String {
        self.endpoint.url.clone()
    }
    fn fetch(&self, auth: Option<&GrokAuth>) -> Result<FetchModelsResult, BackendError> {
        let client = crate::http::shared_startup_blocking_client();
        let (wire, headers) = self.request_auth(auth)?;
        let mut request = client.get(&self.endpoint.url, wire);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send()?;
        let status = response.status().as_u16();
        let etag = etag_of(response.headers());
        let body = response.text().unwrap_or_default();
        self.parse_response(status, etag, &body)
    }
}

impl OaiModelSource {
    /// The same request as [`ModelSource::fetch`], sent on the async client. A caller on a
    /// runtime uses this, so the fetch needs no blocking-pool thread.
    pub(crate) async fn fetch_async(
        &self,
        auth: Option<&GrokAuth>,
    ) -> Result<FetchModelsResult, BackendError> {
        let client = crate::http::shared_client();
        let (wire, headers) = self.request_auth(auth)?;
        let mut request = client
            .get(&self.endpoint.url, wire)
            .timeout(crate::http::STARTUP_FETCH_TIMEOUT);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let etag = etag_of(response.headers());
        let body = response.text().await.map_err(xai_grok_net::Error::from)?;
        self.parse_response(status, etag, &body)
    }

    /// The credential and the extra headers for the listing request.
    fn request_auth(
        &self,
        auth: Option<&GrokAuth>,
    ) -> Result<(xai_grok_net::Auth, Vec<(&'static str, String)>), BackendError> {
        allow_endpoint(&self.endpoint.url)?;
        tracing::info!("Fetching models from {}", self.endpoint.url);
        match self.endpoint.auth {
            EndpointAuth::ApiKey => {
                let wire = match crate::agent::auth_method::read_xai_api_key_env() {
                    Ok(key) => xai_grok_net::Auth::ApiKey {
                        key,
                        scheme: xai_grok_net::KeyScheme::Bearer,
                    },
                    Err(_) => auth.map(GrokAuth::wire_auth).ok_or_else(|| {
                        BackendError::Auth(
                            "No API key for custom models endpoint. Set XAI_API_KEY.".into(),
                        )
                    })?,
                };
                Ok((wire, Vec::new()))
            }
            EndpointAuth::Session => {
                let auth = auth
                    .filter(|_| ActiveAuthBackend::default().is_xai_authority())
                    .ok_or_else(|| {
                        BackendError::Auth("No auth credentials for cli-chat-proxy".into())
                    })?;
                let mut headers = vec![
                    ("X-XAI-Token-Auth", "xai-grok-cli".to_owned()),
                    ("x-userid", auth.user_id.clone()),
                    (
                        "x-grok-client-version",
                        xai_grok_version::version().to_owned(),
                    ),
                    (
                        crate::http::CLIENT_MODE_HEADER,
                        crate::http::process_client_mode().to_owned(),
                    ),
                ];
                if let Some(email) = &auth.email {
                    headers.push(("x-email", email.clone()));
                }
                Ok((auth.wire_auth(), headers))
            }
        }
    }

    fn parse_response(
        &self,
        status: u16,
        etag: Option<String>,
        body: &str,
    ) -> Result<FetchModelsResult, BackendError> {
        if !(200..300).contains(&status) {
            tracing::warn!("Failed to fetch models: {} - {}", status, body);
            return Err(BackendError::RequestFailed {
                status,
                body: body.to_owned(),
            });
        }
        let models_response: ModelsResponse = serde_json::from_str(body)?;
        tracing::info!(
            "Fetched {} models from {}",
            models_response.data.len(),
            self.endpoint.url
        );
        let mut models = Vec::with_capacity(models_response.data.len());
        for (idx, value) in models_response.data.into_iter().enumerate() {
            // Synthetic-shaped entries (non-standard `context_length`,
            // `max_output_length`, `reasoning_parameters.efforts` keys) are parsed
            // through the Synthetic-scoped home so the extension keys stay out of
            // the generic cross-provider fallback chains.
            let parsed = if crate::agent::remote_config::is_synthetic_listing(&value) {
                crate::agent::remote_config::parse_synthetic_model_entry(
                    &value,
                    &self.inference_base_url,
                )
            } else {
                parse_remote_model_value(&value, &self.inference_base_url)
            };
            match parsed {
                Some(model) => models.push(model),
                None => {
                    tracing::warn!(
                        "Skipping model at index {}: missing required field ('model' or 'context_window') or invalid types",
                        idx
                    )
                }
            }
        }
        Ok(FetchModelsResult { models, etag })
    }
}

fn etag_of(headers: &xai_grok_net::header::HeaderMap) -> Option<String> {
    headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointAuth {
    ApiKey,
    Session,
}
struct ListModelsEndpoint {
    url: String,
    auth: EndpointAuth,
}
impl ListModelsEndpoint {
    fn from_endpoints(endpoints: &EndpointsConfig, fetch_auth: ModelFetchAuth) -> Self {
        if endpoints.has_custom_endpoint() {
            Self {
                url: endpoints.resolve_models_list_url(),
                auth: EndpointAuth::ApiKey,
            }
        } else if fetch_auth == ModelFetchAuth::ApiKey {
            let base = endpoints.xai_api_base_url.trim();
            Self {
                url: if base.is_empty() {
                    String::new()
                } else {
                    models_list_url_for_base(base)
                },
                auth: EndpointAuth::ApiKey,
            }
        } else {
            Self {
                url: endpoints.resolve_models_list_url(),
                auth: EndpointAuth::Session,
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// The async fetch sends the request the blocking fetch sends and reads the same models.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn async_fetch_matches_blocking_fetch() {
        use xai_grok_test_support::EnvGuard;
        use xai_grok_test_support::mock_server::{MockInferenceServer, MockModelEntry};
        let server = MockInferenceServer::start_with_models(vec![
            MockModelEntry::new("model-a"),
            MockModelEntry::new("model-b"),
        ])
        .await
        .expect("start mock server");
        let _key = EnvGuard::set("XAI_API_KEY", "test-api-key");
        let _base = EnvGuard::unset("GROK_XAI_API_BASE_URL");
        let _list = EnvGuard::unset("GROK_MODELS_LIST_URL");
        let cfg = EndpointsConfig::from_config_value(
            &toml::from_str(&format!(
                "[endpoints]\nxai_api_base_url = \"{}\"",
                server.url()
            ))
            .expect("endpoints toml"),
        );
        let source = OaiModelSource::new(&cfg, ModelFetchAuth::ApiKey);
        let ids = |result: FetchModelsResult| {
            result
                .models
                .into_iter()
                .map(|m| m.model)
                .collect::<Vec<_>>()
        };

        let from_async = ids(source.fetch_async(None).await.expect("async fetch"));
        let from_blocking = ids(tokio::task::spawn_blocking(move || source.fetch(None))
            .await
            .expect("join")
            .expect("blocking fetch"));

        assert_eq!(from_async, ["model-a", "model-b"]);
        assert_eq!(from_async, from_blocking);
    }

    #[test]
    #[serial_test::serial]
    fn models_fetch_endpoint_matches_auth_mode() {
        use crate::agent::config::EndpointsConfig;
        use crate::agent::remote_config::ModelFetchAuth;
        for k in [
            "GROK_CLI_CHAT_PROXY_BASE_URL",
            "GROK_XAI_API_BASE_URL",
            "GROK_MODELS_LIST_URL",
        ] {
            unsafe { std::env::remove_var(k) };
        }
        let cfg = EndpointsConfig::from_config_value(
            &toml::from_str(
                r#"[endpoints]
                    xai_api_base_url = "https://inference.acme-corp.example/xai/v1""#,
            )
            .unwrap(),
        );
        let session = ListModelsEndpoint::from_endpoints(&cfg, ModelFetchAuth::Session);
        assert_eq!(session.url, "", "no proxy configured, so no listing URL");
        assert_eq!(session.auth, EndpointAuth::Session);
        let api = ListModelsEndpoint::from_endpoints(&cfg, ModelFetchAuth::ApiKey);
        assert_eq!(api.url, "https://inference.acme-corp.example/xai/v1/models");
        assert_eq!(api.auth, EndpointAuth::ApiKey);
        let default = EndpointsConfig::from_config_value(&toml::Value::Table(Default::default()));
        assert_eq!(
            ListModelsEndpoint::from_endpoints(&default, ModelFetchAuth::ApiKey).url,
            "",
            "no xAI API URL configured, so no listing URL"
        );
        let custom = EndpointsConfig::from_config_value(
            &toml::from_str(
                r#"[endpoints]
                    models_base_url = "https://models.acme.com/v1""#,
            )
            .unwrap(),
        );
        let ep = ListModelsEndpoint::from_endpoints(&custom, ModelFetchAuth::Session);
        assert_eq!(ep.url, "https://models.acme.com/v1/models");
        assert_eq!(ep.auth, EndpointAuth::ApiKey);
    }
}
