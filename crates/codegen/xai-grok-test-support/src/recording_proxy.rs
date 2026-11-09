//! A pass-through HTTP proxy that records how each request crossed the wire: its encoding, its delta base and its size.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
use axum::response::{IntoResponse, Response};
use xai_grok_net::{Auth, ClientOptions, HttpClient, is_credential_header};

use crate::loopback::{LoopbackServer, header_text};

/// One request as it arrived at the proxy.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    /// `Content-Encoding`, as sent.
    pub content_encoding: Option<String>,
    /// `X-Zstd-Base`: the hash of the body this is a delta against.
    pub zstd_base: Option<String>,
    /// `X-Zstd-Delta`: set when the client asks the relay to keep the body as a base.
    pub zstd_delta: Option<String>,
    /// The body length on the wire, before any decoding.
    pub wire_bytes: usize,
    pub status: u16,
}

struct ProxyState {
    upstream: String,
    client: HttpClient,
    log: Mutex<Vec<RecordedRequest>>,
}

#[must_use = "dropping stops the proxy"]
pub struct RecordingProxy {
    state: Arc<ProxyState>,
    server: LoopbackServer,
}

/// Headers that describe one hop, not the message.
fn hop_by_hop(name: &HeaderName) -> bool {
    *name == header::HOST
        || *name == header::CONNECTION
        || *name == header::CONTENT_LENGTH
        || *name == header::TRANSFER_ENCODING
}

async fn forward(State(state): State<Arc<ProxyState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, usize::MAX).await else {
        return (StatusCode::BAD_REQUEST, "unreadable request body").into_response();
    };
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_owned(), |pq| pq.as_str().to_owned());
    // xai-grok-net takes the credential headers only as an `Auth`.
    let mut credentials = HeaderMap::new();
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if hop_by_hop(name) {
            continue;
        }
        if is_credential_header(name) {
            credentials.append(name.clone(), value.clone());
        } else {
            headers.append(name.clone(), value.clone());
        }
    }
    let auth = if credentials.is_empty() {
        Auth::None
    } else {
        Auth::Configured(credentials)
    };
    let upstream = state
        .client
        .request(
            parts.method.clone(),
            format!("{}{path}", state.upstream),
            auth,
        )
        .headers(headers);
    let read = |name: &str| parts.headers.get(name).map(header_text);
    let mut record = RecordedRequest {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        content_encoding: read("content-encoding"),
        zstd_base: read("x-zstd-base"),
        zstd_delta: read("x-zstd-delta"),
        wire_bytes: body.len(),
        status: 0,
    };
    let reply = match upstream.body(body).send().await {
        Ok(reply) => reply,
        Err(error) => {
            record.status = StatusCode::BAD_GATEWAY.as_u16();
            state.log.lock().expect("proxy log").push(record);
            return (StatusCode::BAD_GATEWAY, format!("upstream: {error}")).into_response();
        }
    };
    record.status = reply.status().as_u16();
    state.log.lock().expect("proxy log").push(record);

    let mut response = Response::builder().status(reply.status().as_u16());
    for (name, value) in reply.headers() {
        if !hop_by_hop(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(reply.bytes_stream()))
        .unwrap_or_else(|error| {
            (StatusCode::BAD_GATEWAY, format!("response: {error}")).into_response()
        })
}

impl RecordingProxy {
    /// Forward every request to `upstream` (`scheme://host:port`) with its path unchanged.
    pub async fn start(upstream: &str) -> anyhow::Result<Self> {
        let client = HttpClient::build(ClientOptions::default())?;
        let state = Arc::new(ProxyState {
            upstream: upstream.trim_end_matches('/').to_owned(),
            client,
            log: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .fallback(forward)
            .with_state(Arc::clone(&state));
        let server = LoopbackServer::serve(app, "recording proxy").await?;
        Ok(Self { state, server })
    }

    /// e.g. `http://127.0.0.1:12345`
    pub fn origin(&self) -> String {
        format!("http://{}", self.server.addr())
    }

    /// Every request so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.log.lock().expect("proxy log").clone()
    }
}
