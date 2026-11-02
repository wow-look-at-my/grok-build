//! The S3 SDK's HTTP transport. Every request the SDK sends, credential lookups
//! included, goes through `xai_grok_net::HttpClient`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::body::SdkBody;
use xai_grok_net::header::{HeaderMap, HeaderName, HeaderValue};
use xai_grok_net::{Auth, ClientOptions, Method, Url};

/// The session token header of temporary AWS credentials.
const AMZ_SECURITY_TOKEN: &str = "x-amz-security-token";

type TimeoutKey = (Option<Duration>, Option<Duration>);

/// The boxed error the SDK's connector error carries.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn boxed_message(message: impl Into<String>) -> BoxError {
    let message: String = message.into();
    message.into()
}

fn boxed_error(error: impl std::error::Error + Send + Sync + 'static) -> BoxError {
    Box::new(error)
}

/// An SDK `HttpClient` that sends through `xai_grok_net`.
#[derive(Debug, Default, Clone)]
pub(crate) struct NetSmithyClient {
    connectors: Arc<Mutex<HashMap<TimeoutKey, SharedHttpConnector>>>,
}

impl NetSmithyClient {
    fn connector(&self, key: TimeoutKey) -> Result<SharedHttpConnector, xai_grok_net::Error> {
        let mut connectors = self
            .connectors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(c) = connectors.get(&key) {
            return Ok(c.clone());
        }
        let mut options = ClientOptions::default();
        if let Some(t) = key.0 {
            options = options.connect_timeout(t);
        }
        if let Some(t) = key.1 {
            options = options.read_timeout(t);
        }
        let connector = SharedHttpConnector::new(NetConnector {
            client: xai_grok_net::HttpClient::build(options)?,
        });
        connectors.insert(key, connector.clone());
        Ok(connector)
    }
}

impl HttpClient for NetSmithyClient {
    fn http_connector(
        &self,
        settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        let key = (settings.connect_timeout(), settings.read_timeout());
        match self.connector(key) {
            Ok(c) => c,
            Err(e) => SharedHttpConnector::new(BuildFailed(Arc::new(e.to_string()))),
        }
    }
}

/// Fails every request with the error from building the net client.
#[derive(Debug)]
struct BuildFailed(Arc<String>);

impl HttpConnector for BuildFailed {
    fn call(&self, _request: HttpRequest) -> HttpConnectorFuture {
        let err = boxed_message(format!("build the S3 HTTP client: {}", self.0));
        HttpConnectorFuture::ready(Err(ConnectorError::user(err)))
    }
}

#[derive(Debug)]
struct NetConnector {
    client: xai_grok_net::HttpClient,
}

impl HttpConnector for NetConnector {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let client = self.client.clone();
        HttpConnectorFuture::new(async move { send(&client, request).await })
    }
}

/// Split the SDK's headers: the credential headers go as `Auth::Configured`, the rest
/// as plain headers.
pub(crate) fn split_headers(
    headers: impl IntoIterator<Item = (HeaderName, HeaderValue)>,
) -> (HeaderMap, HeaderMap) {
    let mut credential = HeaderMap::new();
    let mut plain = HeaderMap::new();
    for (name, value) in headers {
        if xai_grok_net::is_credential_header(&name) || name.as_str() == AMZ_SECURITY_TOKEN {
            credential.append(name, value);
        } else {
            plain.append(name, value);
        }
    }
    (credential, plain)
}

async fn send(
    client: &xai_grok_net::HttpClient,
    mut request: HttpRequest,
) -> Result<HttpResponse, ConnectorError> {
    let method = Method::from_bytes(request.method().as_bytes())
        .map_err(|e| ConnectorError::user(boxed_message(e.to_string())))?;
    let url = Url::parse(&request.uri().to_string())
        .map_err(|e| ConnectorError::user(boxed_message(format!("S3 request URL: {e}"))))?;
    let mut pairs = Vec::new();
    for (name, value) in request.headers() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| ConnectorError::user(boxed_message(e.to_string())))?;
        let value = HeaderValue::from_bytes(value.as_bytes())
            .map_err(|e| ConnectorError::user(boxed_message(e.to_string())))?;
        pairs.push((name, value));
    }
    let (credential, plain) = split_headers(pairs);

    let body = request.take_body();
    let body = match body.bytes() {
        Some(bytes) => xai_grok_net::Body::from(bytes.to_vec()),
        None => xai_grok_net::Body::wrap_stream(http_body_util::BodyExt::into_data_stream(body)),
    };

    let response = client
        .request(method, url, Auth::Configured(credential))
        .headers(plain)
        .body(body)
        .send()
        .await
        .map_err(connector_error)?;

    let response = http::Response::<xai_grok_net::Body>::from(response);
    let response = response.map(SdkBody::from_body_1_x);
    HttpResponse::try_from(response).map_err(|e| ConnectorError::other(Box::new(e), None))
}

fn connector_error(e: xai_grok_net::Error) -> ConnectorError {
    if e.auth_refusal().is_some() {
        // A refusal is the caller's error: nothing was sent and a retry cannot help.
        ConnectorError::user(boxed_error(e))
    } else if e.is_timeout() {
        ConnectorError::timeout(boxed_error(e))
    } else if e.is_connect() {
        ConnectorError::io(boxed_error(e))
    } else {
        ConnectorError::other(boxed_error(e), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigv4_headers_split_into_auth_and_plain() {
        let pairs = [
            ("authorization", "AWS4-HMAC-SHA256 Credential=AKIA/x"),
            ("x-amz-security-token", "tok"),
            ("x-amz-date", "20260101T000000Z"),
            ("x-amz-content-sha256", "UNSIGNED-PAYLOAD"),
            ("content-type", "text/plain"),
        ]
        .map(|(n, v)| (HeaderName::from_static(n), HeaderValue::from_static(v)));
        let (credential, plain) = split_headers(pairs);
        assert_eq!(credential.len(), 2);
        assert!(credential.contains_key("authorization"));
        assert!(credential.contains_key("x-amz-security-token"));
        assert_eq!(plain.len(), 3);
        assert!(plain.contains_key("x-amz-date"));
    }
}
