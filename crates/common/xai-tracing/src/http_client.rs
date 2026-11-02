// http_client.rs holds the http client code.

use http::HeaderMap;
use opentelemetry::global;
use opentelemetry_http::HeaderInjector;
use std::time::Duration;
use tracing::{Instrument, Span, field};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use xai_grok_net::{Auth, ClientOptions, HttpClient, Method, Request, Response};

pub fn attach_trace_to_http_request(headers: &mut HeaderMap) {
    global::get_text_map_propagator(|propagator| {
        let context = Span::current().context();
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
}

/// An [`HttpClient`] that opens a client span for each request and sends its `traceparent`.
#[derive(Clone, Debug)]
pub struct TracedHttpClient {
    inner: HttpClient,
}

pub fn traced_client(client: HttpClient) -> TracedHttpClient {
    TracedHttpClient { inner: client }
}

pub fn traced_client_new() -> Result<TracedHttpClient, xai_grok_net::Error> {
    traced_client_from_builder(ClientOptions::default())
}

pub fn traced_client_from_builder(
    options: ClientOptions,
) -> Result<TracedHttpClient, xai_grok_net::Error> {
    Ok(traced_client(HttpClient::build(options)?))
}

impl TracedHttpClient {
    pub fn request(&self, method: Method, url: impl AsRef<str>, auth: Auth) -> TracedRequest {
        let url = url.as_ref();
        TracedRequest {
            method: method.as_str().to_owned(),
            url: url.to_owned(),
            inner: self.inner.request(method, url, auth),
        }
    }

    pub fn get(&self, url: impl AsRef<str>, auth: Auth) -> TracedRequest {
        self.request(Method::GET, url, auth)
    }

    pub fn post(&self, url: impl AsRef<str>, auth: Auth) -> TracedRequest {
        self.request(Method::POST, url, auth)
    }

    /// The client without the tracing layer.
    pub fn inner(&self) -> &HttpClient {
        &self.inner
    }
}

/// A request of a [`TracedHttpClient`].
pub struct TracedRequest {
    inner: Request,
    method: String,
    url: String,
}

impl TracedRequest {
    /// Change the inner request, for a builder step this type does not wrap.
    pub fn map(mut self, f: impl FnOnce(Request) -> Request) -> Self {
        self.inner = f(self.inner);
        self
    }

    pub fn headers(self, headers: HeaderMap) -> Self {
        self.map(|r| r.headers(headers))
    }

    pub fn json<T: serde::Serialize + ?Sized>(self, json: &T) -> Self {
        self.map(|r| r.json(json))
    }

    pub fn body(self, body: impl Into<xai_grok_net::Body>) -> Self {
        self.map(|r| r.body(body))
    }

    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|r| r.timeout(timeout))
    }

    pub async fn send(self) -> Result<Response, xai_grok_net::Error> {
        // No active dispatcher → the span has no consumer and would only be
        // downgraded to `log` spam. See `crate::dispatcher_active`.
        let span = if crate::dispatcher_active() {
            tracing::info_span!(
                "http_request",
                otel.kind = "client",
                "http.request.method" = %self.method,
                "url.full" = %self.url,
                "http.response.status_code" = field::Empty,
            )
        } else {
            Span::none()
        };

        let inner = self.inner;
        let result = async move {
            let mut headers = HeaderMap::new();
            attach_trace_to_http_request(&mut headers);
            inner.headers(headers).send().await
        }
        .instrument(span.clone())
        .await;

        if let Ok(ref response) = result {
            span.record("http.response.status_code", response.status().as_u16());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{OtelTestEnv, otel_span_id_hex, otel_trace_id_hex, parse_traceparent};
    use tracing::Instrument;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn attach_trace_to_http_request_writes_traceparent() {
        let _env = OtelTestEnv::install();
        let span = tracing::info_span!("http_request", otel.kind = "client");
        let span_id = otel_span_id_hex(&span);
        let mut headers = HeaderMap::new();
        let _enter = span.enter();
        attach_trace_to_http_request(&mut headers);
        let tp = headers.get("traceparent").unwrap().to_str().unwrap();
        let (_ver, _tid, injected_span_id) = parse_traceparent(tp);
        assert_eq!(injected_span_id, span_id);
    }

    #[tokio::test]
    async fn traced_client_injects_client_span_not_parent_on_wire() {
        let _env = OtelTestEnv::install();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = traced_client(HttpClient::build(ClientOptions::default()).unwrap());
        let parent = tracing::info_span!("parent_handler");
        let parent_span_id = otel_span_id_hex(&parent);
        let parent_trace_id = otel_trace_id_hex(&parent);

        async {
            client
                .get(format!("{}/health", server.uri()), Auth::None)
                .send()
                .await
                .unwrap();
        }
        .instrument(parent)
        .await;

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let tp = received[0]
            .headers
            .get("traceparent")
            .expect("traceparent on wire")
            .to_str()
            .unwrap();
        let (_ver, injected_trace_id, injected_span_id) = parse_traceparent(tp);

        assert_eq!(injected_trace_id, parent_trace_id);
        assert_ne!(injected_span_id, parent_span_id);
    }

    #[tokio::test]
    async fn traced_client_returns_response_status() {
        let _env = OtelTestEnv::install();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = traced_client_new().unwrap();
        let resp = client
            .get(format!("{}/missing", server.uri()), Auth::None)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        assert!(
            server.received_requests().await.unwrap()[0]
                .headers
                .get("traceparent")
                .is_some()
        );
    }
}
