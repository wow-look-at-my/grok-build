//! WebSocket connects. The upgrade request carries its credential by the same rules as
//! an HTTP request.

use request::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub use tokio_tungstenite::tungstenite::{Message, Utf8Bytes, error, protocol};
pub use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The error a WebSocket connect or stream returns.
pub type WsError = tokio_tungstenite::tungstenite::Error;

use crate::auth::{self, Auth, is_credential_header};
use crate::error::{AuthRefusal, Error};

/// A WebSocket over TCP, with TLS when the URL is `wss`.
pub type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A WebSocket upgrade request whose credential this crate has checked.
pub struct WsRequest {
    inner: tokio_tungstenite::tungstenite::handshake::client::Request,
}

/// Why a WebSocket did not connect.
#[derive(Debug)]
pub enum WsConnectError {
    /// The request was refused or malformed before anything was sent.
    Request(Error),
    /// The handshake failed.
    Handshake(WsError),
}

impl std::fmt::Display for WsConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => e.fmt(f),
            Self::Handshake(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for WsConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(e) => Some(e),
            Self::Handshake(e) => Some(e),
        }
    }
}

impl WsRequest {
    /// An upgrade request to `url` carrying `auth` and the non-credential `headers`.
    pub fn new(url: &str, auth: Auth, headers: HeaderMap) -> Result<Self, Error> {
        let parsed = request::Url::parse(url).map_err(|_| AuthRefusal::InvalidValue)?;
        // The host rules are the HTTP ones: wss is https, ws is http.
        let mut as_http = parsed.clone();
        let scheme = match parsed.scheme() {
            "wss" => "https",
            "ws" => "http",
            other => other,
        };
        if as_http.set_scheme(scheme).is_err() {
            return Err(AuthRefusal::InvalidValue.into());
        }
        if let Some(name) = headers.keys().find(|n| is_credential_header(n)) {
            return Err(AuthRefusal::CredentialHeaderOutsideAuth(name.as_str().to_owned()).into());
        }
        let mut all = headers;
        auth::apply(&auth, &as_http, &mut all)?;
        auth::check_outgoing(&as_http, &all)?;
        let mut inner = url
            .into_client_request()
            .map_err(|_| AuthRefusal::InvalidValue)?;
        for (name, value) in &all {
            let name = HeaderName::from_bytes(name.as_str().as_bytes())
                .map_err(|_| AuthRefusal::InvalidValue)?;
            let value =
                HeaderValue::from_bytes(value.as_bytes()).map_err(|_| AuthRefusal::InvalidValue)?;
            inner.headers_mut().insert(name, value);
        }
        Ok(Self { inner })
    }

    /// The host and port the request goes to, for a caller that opens a proxy tunnel.
    pub fn host_port(&self) -> Option<(String, u16)> {
        let uri = self.inner.uri();
        let host = uri.host()?.to_owned();
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("ws") {
            80
        } else {
            443
        });
        Some((host, port))
    }
}

/// Connect over TCP with the grok TLS policy.
pub async fn connect_ws(
    request: WsRequest,
) -> Result<
    (
        WsStream,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    WsConnectError,
> {
    let connector = tokio_tungstenite::Connector::Rustls(xai_grok_extra_ca::rustls_client_config());
    tokio_tungstenite::connect_async_tls_with_config(request.inner, None, false, Some(connector))
        .await
        .map_err(WsConnectError::Handshake)
}

/// Run the handshake over a stream the caller opened, such as a proxy tunnel.
pub async fn connect_ws_over<S>(
    request: WsRequest,
    stream: S,
) -> Result<
    (
        WebSocketStream<S>,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    WsConnectError,
>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio_tungstenite::client_async(request.inner, stream)
        .await
        .map_err(WsConnectError::Handshake)
}
