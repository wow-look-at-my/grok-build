//! The sampling client that the Imagine image and video tools send their
//! requests through.

use xai_grok_net::header::{HeaderMap, HeaderName, HeaderValue};
use xai_tool_runtime::ToolError;

/// A sink that keeps every call, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Recorder {
    calls: parking_lot::Mutex<Vec<xai_grok_sampler::BilledCall>>,
}

#[cfg(test)]
impl Recorder {
    pub(crate) fn calls(&self) -> Vec<xai_grok_sampler::BilledCall> {
        self.calls.lock().clone()
    }
}

#[cfg(test)]
impl xai_grok_sampler::UsageSink for Recorder {
    fn record_call(&self, call: xai_grok_sampler::BilledCall) {
        self.calls.lock().push(call);
    }
}

#[cfg(test)]
pub(crate) fn recorder() -> (std::sync::Arc<Recorder>, xai_grok_sampler::SharedUsageSink) {
    let recorder = std::sync::Arc::new(Recorder::default());
    (recorder.clone(), recorder)
}

/// The endpoint, headers and sink shared by every request of one media client.
#[derive(Clone)]
pub(crate) struct MediaEndpoint {
    base_url: String,
    /// The configured headers without the credential ones.
    extra_headers: indexmap::IndexMap<String, String>,
    /// Credential headers from `extra_headers`. The bearer is merged into them per request.
    credential_headers: HeaderMap,
    /// Per-request `x-grok-session-id`. A configured value always wins.
    session_id: Option<String>,
    defaults_have_session_header: bool,
    /// Bills each call. A request with no sink is refused.
    usage_sink: Option<xai_grok_sampler::SharedUsageSink>,
}

impl MediaEndpoint {
    pub(crate) fn new(
        base_url: &str,
        extra_headers: &indexmap::IndexMap<String, String>,
    ) -> Result<Self, ToolError> {
        let mut plain = indexmap::IndexMap::new();
        let mut credential_headers = HeaderMap::new();
        for (key, value) in extra_headers {
            let name = HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                ToolError::invalid_arguments(format!("Invalid header name '{key}': {e}"))
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                ToolError::invalid_arguments(format!("Invalid header value for '{key}': {e}"))
            })?;
            if xai_grok_net::is_credential_header(&name) {
                credential_headers.insert(name, header_value);
            } else {
                plain.insert(key.clone(), value.clone());
            }
        }
        let defaults_have_session_header = plain
            .keys()
            .any(|k| k.eq_ignore_ascii_case(super::image_gen::SESSION_ID_HEADER));
        Ok(Self {
            base_url: base_url.to_owned(),
            extra_headers: plain,
            credential_headers,
            session_id: None,
            defaults_have_session_header,
            usage_sink: None,
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn set_session_id(&mut self, session_id: &str) {
        if !self.defaults_have_session_header && HeaderValue::from_str(session_id).is_ok() {
            self.session_id = Some(session_id.to_owned());
        }
    }

    #[cfg(test)]
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn set_usage_sink(&mut self, sink: Option<xai_grok_sampler::SharedUsageSink>) {
        self.usage_sink = sink;
    }

    /// The client for one request to `model` that carries `sent_bearer`.
    pub(crate) fn client(
        &self,
        tool: &str,
        model: &str,
        sent_bearer: &str,
    ) -> Result<xai_grok_sampler::SamplingClient, ToolError> {
        let Some(sink) = self.usage_sink.clone() else {
            tracing::error!(
                tool,
                "media tool has no usage sink; refusing a call that would bill nowhere"
            );
            return Err(ToolError::execution(
                xai_tool_protocol::ToolId::new(tool).expect("valid tool id"),
                format!("{tool} has no usage sink, so its model call would bill nowhere"),
            ));
        };
        let mut extra_headers = self.extra_headers.clone();
        if let Some(session_id) = &self.session_id {
            extra_headers.insert(
                super::image_gen::SESSION_ID_HEADER.to_owned(),
                session_id.clone(),
            );
        }
        let config = xai_grok_sampler::SamplerConfig {
            base_url: self.base_url.clone(),
            model: model.to_owned(),
            auth_scheme: xai_grok_sampler::AuthScheme::Bearer,
            extra_headers,
            usage_sink: Some(sink),
            ..Default::default()
        };
        let auth = crate::util::shared_http::key_auth(
            Some(sent_bearer),
            "Bearer",
            &self.credential_headers,
        );
        xai_grok_sampler::SamplingClient::new(config)
            .map(|client| client.with_request_auth(auth))
            .map_err(|e| {
                ToolError::execution(
                    xai_tool_protocol::ToolId::new(tool).expect("valid tool id"),
                    format!("{tool} could not build its request: {e}"),
                )
            })
    }
}
