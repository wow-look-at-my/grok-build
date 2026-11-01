//! rmcp's HTTP traits over [`xai_grok_net::HttpClient`], so every MCP request
//! takes the HTTP path of the CLI.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorData, JsonRpcMessage, ServerJsonRpcMessage,
};
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, OAuthHttpClient, OAuthHttpClientError, OAuthHttpClientFuture,
    OAuthHttpRedirectPolicy, OAuthHttpRequest,
};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_MCP_PROTOCOL_VERSION, HEADER_SESSION_ID,
    JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Error as SseError, Sse, SseStream};
use xai_grok_net::{Auth, ClientOptions, HttpClient, KeyScheme};

/// rmcp's limit on the raw size of one SSE event.
const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;
/// Bytes of an unusable JSON body kept in an error message.
const JSON_RPC_BODY_PREVIEW_LEN: usize = 256;
/// Headers rmcp sets itself. A custom header must not use one of these names.
const RESERVED_HEADERS: &[&str] = &[
    "accept",
    HEADER_SESSION_ID,
    HEADER_MCP_PROTOCOL_VERSION,
    HEADER_LAST_EVENT_ID,
];
/// rmcp's default timeout and body cap for an OAuth request.
const OAUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OAUTH_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

/// The error of a request the MCP HTTP clients sent.
#[derive(Debug)]
pub struct McpHttpError(xai_grok_net::Error);

impl McpHttpError {
    /// The error from the HTTP path.
    pub fn net(&self) -> &xai_grok_net::Error {
        &self.0
    }

    /// True when the connection to the server did not open.
    pub fn is_connect(&self) -> bool {
        self.0.is_connect()
    }
}

impl std::fmt::Display for McpHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for McpHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<xai_grok_net::Error> for McpHttpError {
    fn from(e: xai_grok_net::Error) -> Self {
        Self(e)
    }
}

impl From<xai_grok_net::TransportError> for McpHttpError {
    fn from(e: xai_grok_net::TransportError) -> Self {
        Self(e.into())
    }
}

impl From<McpHttpError> for StreamableHttpError<McpHttpError> {
    fn from(e: McpHttpError) -> Self {
        StreamableHttpError::Client(e)
    }
}

fn client_error(e: impl Into<McpHttpError>) -> StreamableHttpError<McpHttpError> {
    StreamableHttpError::Client(e.into())
}

/// Move the credential headers out of `headers` and return them.
pub(crate) fn take_credential_headers(headers: &mut HeaderMap) -> HeaderMap {
    let names: Vec<_> = headers
        .keys()
        .filter(|name| xai_grok_net::is_credential_header(name))
        .cloned()
        .collect();
    let mut credentials = HeaderMap::new();
    for name in names {
        for value in headers.get_all(&name) {
            credentials.append(name.clone(), value.clone());
        }
        headers.remove(&name);
    }
    credentials
}

/// The streamable-HTTP client rmcp's transport sends through.
#[derive(Clone, Debug)]
pub struct NetHttpClient {
    client: HttpClient,
    /// The credential headers the user configured for this server.
    configured: HeaderMap,
}

impl NetHttpClient {
    /// `configured` holds the credential headers from the server's config.
    /// Every request sends them.
    pub fn new(client: HttpClient, configured: HeaderMap) -> Self {
        Self { client, configured }
    }

    /// The credential of one request. `token` is the OAuth token rmcp holds for this server.
    fn auth(&self, token: Option<String>) -> Auth {
        match token {
            None if self.configured.is_empty() => Auth::None,
            None => Auth::Configured(self.configured.clone()),
            Some(key) if self.configured.is_empty() => Auth::ApiKey {
                key,
                scheme: KeyScheme::Bearer,
            },
            // The token replaces a configured `Authorization`, and the other configured credentials still go.
            Some(key) => match HeaderValue::from_str(&format!("Bearer {key}")) {
                Ok(bearer) => {
                    let mut headers = self.configured.clone();
                    headers.insert(AUTHORIZATION, bearer);
                    Auth::Configured(headers)
                }
                Err(_) => Auth::ApiKey {
                    key,
                    scheme: KeyScheme::Bearer,
                },
            },
        }
    }
}

fn validate_custom_header(name: &HeaderName) -> Result<(), String> {
    let reserved = RESERVED_HEADERS
        .iter()
        .any(|r| name.as_str().eq_ignore_ascii_case(r));
    // The worker sets `MCP-Protocol-Version` as a custom header after the handshake.
    if reserved
        && !name
            .as_str()
            .eq_ignore_ascii_case(HEADER_MCP_PROTOCOL_VERSION)
    {
        return Err(name.to_string());
    }
    Ok(())
}

fn apply_custom_headers(
    mut request: xai_grok_net::Request,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<xai_grok_net::Request, StreamableHttpError<McpHttpError>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name).map_err(StreamableHttpError::ReservedHeaderConflict)?;
        request = request.header(name, value);
    }
    Ok(request)
}

/// The `scope=` parameter of a `WWW-Authenticate` value, quoted or not.
fn extract_scope_from_header(header: &str) -> Option<String> {
    let start = header.to_ascii_lowercase().find("scope=")? + "scope=".len();
    let value = header.get(start..)?;
    if let Some(quoted) = value.strip_prefix('"') {
        return quoted
            .find('"')
            .and_then(|end| quoted.get(..end))
            .map(str::to_owned);
    }
    let end = value
        .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .unwrap_or(value.len());
    (end > 0)
        .then(|| value.get(..end).map(str::to_owned))
        .flatten()
}

fn auth_challenge(
    response: &xai_grok_net::Response,
) -> Result<(), StreamableHttpError<McpHttpError>> {
    let status = response.status();
    if status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN {
        return Ok(());
    }
    let Some(header) = response.headers().get(WWW_AUTHENTICATE) else {
        return Ok(());
    };
    let header = header.to_str().map_err(|_| {
        StreamableHttpError::UnexpectedServerResponse(Cow::from(
            "invalid www-authenticate header value",
        ))
    })?;
    if status == StatusCode::UNAUTHORIZED {
        return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
            header.to_owned(),
        )));
    }
    Err(StreamableHttpError::InsufficientScope(
        InsufficientScopeError::new(header.to_owned(), extract_scope_from_header(header)),
    ))
}

fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

fn json_rpc_body_preview(body: &[u8]) -> Cow<'static, str> {
    if body.is_empty() {
        return Cow::Borrowed("<empty>");
    }
    let lossy = String::from_utf8_lossy(body);
    if lossy.len() <= JSON_RPC_BODY_PREVIEW_LEN {
        return Cow::Owned(lossy.into_owned());
    }
    let mut end = JSON_RPC_BODY_PREVIEW_LEN;
    while end > 0 && !lossy.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(lossy.get(..end).unwrap_or_default().to_owned())
}

fn is_reply_free(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        ClientJsonRpcMessage::Notification(_)
            | ClientJsonRpcMessage::Response(_)
            | ClientJsonRpcMessage::Error(_)
    )
}

/// A legacy server can reject a sessionless `server/discover` with a plain 4xx before JSON-RPC dispatch.
/// This turns that answer into a JSON-RPC error, so the lifecycle layer reads it as a legacy server.
/// Auth failures and server errors keep their own paths.
fn legacy_discover_response(
    message: &ClientJsonRpcMessage,
    session_was_attached: bool,
    status: StatusCode,
    body: &str,
) -> Option<StreamableHttpPostResponse> {
    if session_was_attached
        || !status.is_client_error()
        || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    {
        return None;
    }
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    if !matches!(request.request, ClientRequest::DiscoverRequest(_)) {
        return None;
    }
    let error = ErrorData::invalid_request(
        format!("server/discover rejected with HTTP {status}: {body}"),
        None,
    );
    Some(StreamableHttpPostResponse::Json(
        ServerJsonRpcMessage::error(error, Some(request.id.clone())),
        None,
    ))
}

#[derive(Debug, thiserror::Error)]
enum SseBodyError {
    #[error(transparent)]
    Source(McpHttpError),
    #[error("SSE event exceeded the maximum size of {max_size} bytes")]
    EventTooLarge { max_size: usize },
}

/// Counts the bytes of the SSE event in progress, the way the SSE parser keeps them.
struct SseEventSizeLimiter {
    max_size: usize,
    retained_size: usize,
    line_size: usize,
    line_is_comment: bool,
    previous_was_cr: bool,
}

impl SseEventSizeLimiter {
    fn new(max_size: usize) -> Self {
        Self {
            max_size,
            retained_size: 0,
            line_size: 0,
            line_is_comment: false,
            previous_was_cr: false,
        }
    }

    fn observe(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_was_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    if self.line_size == 0 {
                        self.line_is_comment = byte == b':';
                    }
                    self.line_size = self.line_size.saturating_add(1);
                    self.check_limit()?;
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<(), ()> {
        if self.line_size == 0 {
            self.retained_size = 0;
        } else if !self.line_is_comment {
            // The parser adds a newline when it joins data fields.
            self.retained_size = self
                .retained_size
                .saturating_add(self.line_size)
                .saturating_add(1);
        }
        self.line_size = 0;
        self.line_is_comment = false;
        self.check_limit()
    }

    fn check_limit(&self) -> Result<(), ()> {
        if self.retained_size.saturating_add(self.line_size) > self.max_size {
            Err(())
        } else {
            Ok(())
        }
    }
}

/// The SSE events of `response`. The stream ends after its first error.
fn bounded_sse_stream(
    response: xai_grok_net::Response,
    max_event_size: usize,
) -> BoxStream<'static, Result<Sse, SseError>> {
    let limiter = SseEventSizeLimiter::new(max_event_size);
    let bytes = response
        .bytes_stream()
        .scan((limiter, false), |(limiter, failed), chunk| {
            let item = if *failed {
                None
            } else {
                Some(match chunk {
                    Ok(chunk) if limiter.observe(&chunk).is_err() => {
                        *failed = true;
                        Err(SseBodyError::EventTooLarge {
                            max_size: limiter.max_size,
                        })
                    }
                    Ok(chunk) => Ok(chunk),
                    Err(e) => {
                        *failed = true;
                        Err(SseBodyError::Source(e.into()))
                    }
                })
            };
            futures::future::ready(item)
        });
    SseStream::from_bytes_stream(bytes).boxed()
}

fn accept_value() -> String {
    [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", ")
}

impl StreamableHttpClient for NetHttpClient {
    type Error = McpHttpError;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_token,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request = self
            .client
            .get(uri.as_ref(), self.auth(auth_token))
            .header(ACCEPT, accept_value());
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        let request = apply_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        auth_challenge(&response)?;
        let response = response.error_for_status().map_err(client_error)?;
        match response.headers().get(CONTENT_TYPE) {
            Some(ct)
                if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    || ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {}
            Some(ct) => {
                return Err(StreamableHttpError::UnexpectedContentType(Some(
                    String::from_utf8_lossy(ct.as_bytes()).to_string(),
                )));
            }
            None => return Err(StreamableHttpError::UnexpectedContentType(None)),
        }
        Ok(bounded_sse_stream(response, max_sse_event_size))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let request = self
            .client
            .delete(uri.as_ref(), self.auth(auth_token))
            .header(HEADER_SESSION_ID, session.as_ref());
        let request = apply_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            tracing::debug!("this server doesn't support deleting session");
            return Ok(());
        }
        response.error_for_status().map_err(client_error)?;
        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_token,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let request = self
            .client
            .post(uri.as_ref(), self.auth(auth_token))
            .header(ACCEPT, accept_value());
        let mut request = apply_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(&message).send().await.map_err(client_error)?;
        auth_challenge(&response)?;
        let status = response.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if status.is_success() && response.content_length() == Some(0) && is_reply_free(&message) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // A failed status can carry a JSON-RPC error, which must reach the caller as one.
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
            {
                match parse_json_rpc_error(&body) {
                    Some(message) => {
                        return Ok(StreamableHttpPostResponse::Json(message, session_id));
                    }
                    None => tracing::warn!(
                        "HTTP {status}: could not parse JSON body as a JSON-RPC error"
                    ),
                }
            }
            if let Some(response) =
                legacy_discover_response(&message, session_was_attached, status, &body)
            {
                return Ok(response);
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                Ok(StreamableHttpPostResponse::Sse(
                    bounded_sse_stream(response, max_sse_event_size),
                    session_id,
                ))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                let body = response.bytes().await.map_err(client_error)?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, session_id)),
                    // A message that awaits no reply reads an unusable body as accepted.
                    // A request still needs a reply, so the worker must not wait on SSE forever.
                    Err(e) if is_reply_free(&message) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                    Err(e) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                        format!(
                            "could not parse JSON response as ServerJsonRpcMessage: {e}: {}",
                            json_rpc_body_preview(&body)
                        ),
                    ))),
                }
            }
            _ => {
                tracing::error!("unexpected content type: {:?}", content_type);
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }
}

/// The client every OAuth request of an [`AuthorizationManager`] goes through.
pub struct NetOAuthHttpClient {
    follow_redirects: HttpClient,
    stop_redirects: HttpClient,
}

impl NetOAuthHttpClient {
    pub fn new() -> Result<Self, xai_grok_net::Error> {
        let options = ClientOptions::default().timeout(OAUTH_HTTP_TIMEOUT);
        Ok(Self {
            follow_redirects: HttpClient::build(options.clone())?,
            stop_redirects: HttpClient::build(options.no_redirects())?,
        })
    }
}

impl OAuthHttpClient for NetOAuthHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let OAuthHttpRequest {
                request,
                redirect_policy,
                timeout,
                ..
            } = request;
            let client = match redirect_policy {
                OAuthHttpRedirectPolicy::Follow => &self.follow_redirects,
                _ => &self.stop_redirects,
            };
            let (parts, body) = request.into_parts();
            let mut headers = parts.headers;
            // rmcp puts a client secret in `Authorization: Basic` on some token requests.
            let credentials = take_credential_headers(&mut headers);
            let auth = if credentials.is_empty() {
                Auth::None
            } else {
                Auth::Configured(credentials)
            };
            let mut outgoing = client
                .request(parts.method, parts.uri.to_string().as_str(), auth)
                .headers(headers)
                .body(body);
            if let Some(timeout) = timeout {
                outgoing = outgoing.timeout(timeout);
            }
            let response = outgoing
                .send()
                .await
                .map_err(|e| Box::new(McpHttpError(e)) as OAuthHttpClientError)?;

            let mut builder = http::Response::builder()
                .status(response.status())
                .version(response.version());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let mut body = Vec::new();
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                let chunk =
                    chunk.map_err(|e| Box::new(McpHttpError::from(e)) as OAuthHttpClientError)?;
                if chunk.len() > MAX_OAUTH_RESPONSE_BODY_BYTES - body.len() {
                    return Err(format!(
                        "OAuth HTTP response body exceeds {MAX_OAUTH_RESPONSE_BODY_BYTES} bytes"
                    )
                    .into());
                }
                body.extend_from_slice(&chunk);
            }
            builder
                .body(body)
                .map_err(|e| Box::new(e) as OAuthHttpClientError)
        })
    }
}

/// An [`AuthorizationManager`] whose OAuth requests take the CLI's HTTP path.
pub async fn new_authorization_manager(url: &str) -> Result<AuthorizationManager, AuthError> {
    let client = NetOAuthHttpClient::new().map_err(|e| {
        AuthError::InternalError(format!("failed to build the OAuth HTTP client: {e}"))
    })?;
    AuthorizationManager::new_with_oauth_http_client(url, Arc::new(client)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_is_read_quoted_and_unquoted() {
        assert_eq!(
            extract_scope_from_header(r#"Bearer error="insufficient_scope", scope="a b""#),
            Some("a b".to_owned())
        );
        assert_eq!(
            extract_scope_from_header(r#"Bearer scope=read:data, error="x""#),
            Some("read:data".to_owned())
        );
        assert_eq!(extract_scope_from_header("Bearer"), None);
    }

    #[test]
    fn reserved_custom_headers_are_refused_except_the_protocol_version() {
        assert!(validate_custom_header(&HeaderName::from_static("accept")).is_err());
        assert!(validate_custom_header(&HeaderName::from_static("mcp-session-id")).is_err());
        assert!(validate_custom_header(&HeaderName::from_static("mcp-protocol-version")).is_ok());
        assert!(validate_custom_header(&HeaderName::from_static("x-custom")).is_ok());
    }

    #[test]
    fn the_oauth_token_is_a_bearer_key_and_keeps_other_configured_credentials() {
        let client = HttpClient::build(ClientOptions::default()).unwrap();
        let bare = NetHttpClient::new(client.clone(), HeaderMap::new());
        assert!(matches!(bare.auth(None), Auth::None));
        assert!(matches!(
            bare.auth(Some("t".into())),
            Auth::ApiKey {
                scheme: KeyScheme::Bearer,
                ..
            }
        ));

        let mut configured = HeaderMap::new();
        configured.insert("x-api-key", HeaderValue::from_static("k"));
        configured.insert(AUTHORIZATION, HeaderValue::from_static("Bearer old"));
        let with_key = NetHttpClient::new(client, configured);
        let Auth::Configured(sent) = with_key.auth(Some("new".into())) else {
            panic!("configured credentials must still be sent");
        };
        assert_eq!(sent["authorization"], "Bearer new");
        assert_eq!(sent["x-api-key"], "k");
    }

    #[test]
    fn credential_headers_move_out_of_the_header_map() {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_static("k"));
        headers.insert("x-other", HeaderValue::from_static("v"));
        let credentials = take_credential_headers(&mut headers);
        assert_eq!(credentials.len(), 1);
        assert_eq!(headers.len(), 1);
        assert!(headers.contains_key("x-other"));
    }

    /// The limit applies before the SSE parser, so an oversized event is an error.
    #[tokio::test]
    async fn post_sse_response_honors_the_event_limit() {
        use axum::{Router, routing::post};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/mcp",
                post(|| async {
                    (
                        [(CONTENT_TYPE, "text/event-stream")],
                        "data: this event is too large\n",
                    )
                }),
            );
            axum::serve(listener, app).await
        });
        let message = ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(Default::default()),
            rmcp::model::RequestId::Number(1),
        );
        let client = NetHttpClient::new(
            HttpClient::build(ClientOptions::default()).unwrap(),
            HeaderMap::new(),
        );
        let response = client
            .post_message_with_max_sse_event_size(
                Arc::from(format!("http://{addr}/mcp")),
                message,
                None,
                None,
                HashMap::new(),
                16,
            )
            .await
            .unwrap();
        let StreamableHttpPostResponse::Sse(mut stream, _) = response else {
            panic!("expected an SSE response");
        };
        let error = stream.next().await.unwrap().unwrap_err();
        server.abort();
        assert!(
            error.to_string().contains("maximum size of 16 bytes"),
            "{error}"
        );
    }
}
