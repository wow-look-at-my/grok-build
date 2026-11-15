//! The provider endpoints other than the chat APIs: images, video, embeddings.

use std::time::Duration;

use xai_grok_net::Method;
use xai_grok_net::header::{CONTENT_TYPE, HeaderMap};
use xai_grok_net::ws::{WsRequest, WsStream};
use xai_grok_sampling_types::{Result, SamplingError};

use super::{SamplingClient, SentRequest};
use crate::request_slots::RequestSlot;

/// The provider's answer to one endpoint request, with any status.
#[derive(Debug, Clone)]
pub struct EndpointResponse {
    pub status: xai_grok_net::StatusCode,
    pub body: bytes::Bytes,
}

impl EndpointResponse {
    /// The body as text. Invalid UTF-8 is replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// An open speech-to-text socket and the request slot it holds.
pub struct WebSocketSession {
    pub socket: WsStream,
    /// Keep this for as long as the socket is open.
    pub slot: Option<RequestSlot>,
}

impl SamplingClient {
    /// `POST images/generations`.
    pub async fn generate_images(
        &self,
        request: &serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::POST, "images/generations", Some(request), timeout)
            .await
    }

    /// `POST images/edits`.
    pub async fn edit_images(
        &self,
        request: &serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::POST, "images/edits", Some(request), timeout)
            .await
    }

    /// `POST videos/generations`. The answer names the request id that [`Self::video_status`] reads.
    pub async fn generate_video(
        &self,
        request: &serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::POST, "videos/generations", Some(request), timeout)
            .await
    }

    /// `GET videos/{request_id}`.
    pub async fn video_status(
        &self,
        request_id: &str,
        timeout: Option<Duration>,
    ) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::GET, &format!("videos/{request_id}"), None, timeout)
            .await
    }

    /// `POST embeddings`.
    pub async fn create_embeddings(&self, request: &serde_json::Value) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::POST, "embeddings", Some(request), None)
            .await
    }

    /// `POST tokenize-text`.
    pub async fn tokenize(&self, request: &serde_json::Value) -> Result<EndpointResponse> {
        self.endpoint_exchange(Method::POST, "tokenize-text", Some(request), None)
            .await
    }

    /// Open a WebSocket to `path` (with its query) on the base URL, `https` read as `wss`.
    /// `headers` go out over the client's own headers.
    pub async fn connect_websocket(
        &self,
        path: &str,
        headers: HeaderMap,
    ) -> Result<WebSocketSession> {
        let url = self.endpoint(path);
        let url = match url.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}"),
            Some(("http", rest)) => format!("ws://{rest}"),
            _ => url,
        };
        let slot = crate::request_slots::acquire_unless_held().await;
        self.prepare_bearer().await;
        let SentRequest { builder, .. } = self.post(url.clone());
        let mut all = builder.headers;
        all.remove(CONTENT_TYPE);
        for (name, value) in &headers {
            all.insert(name.clone(), value.clone());
        }
        let request = WsRequest::new(&url, builder.auth, all).map_err(SamplingError::Http)?;
        let (socket, _) = xai_grok_net::ws::connect_ws(request)
            .await
            .map_err(|e| SamplingError::EventStreamError(format!("websocket connect: {e}")))?;
        Ok(WebSocketSession { socket, slot })
    }

    /// Send one request and read its whole body under a slot. A success reports the usage and
    /// cost the body carries. A body with neither reports nothing.
    async fn endpoint_exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        timeout: Option<Duration>,
    ) -> Result<EndpointResponse> {
        let model = body
            .and_then(|b| b.get("model"))
            .and_then(serde_json::Value::as_str);
        let _slot = crate::request_slots::acquire_unless_held().await;
        let mut meter = self.meter(model);
        self.prepare_bearer().await;
        let SentRequest { builder, .. } = self.post(self.endpoint(path));
        let builder = match body {
            Some(body) => builder.json(body)?,
            None => builder,
        };
        let mut request = self.net_request_as(method, builder)?;
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(SamplingError::from)?;
        let status = response.status();
        let body = super::read_body(response, status).await?;
        if status.is_success() {
            meter.observe_endpoint_body(&body);
        }
        meter.finish();
        Ok(EndpointResponse { status, body })
    }
}
