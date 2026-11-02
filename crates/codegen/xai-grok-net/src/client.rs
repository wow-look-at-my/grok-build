// client.rs holds the client code.

use std::time::Duration;

use request::header::{HeaderMap, HeaderName, HeaderValue};
use request::{IntoUrl, Method, Url};

use crate::auth::{self, Auth, is_credential_header};
use crate::error::{AuthRefusal, Error};

/// How a client is built. Nothing here can carry a credential: a default header with a
/// credential name fails the build.
#[derive(Clone, Default)]
pub struct ClientOptions {
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: Option<usize>,
    tcp_keepalive: Option<Duration>,
    http2_keep_alive_interval: Option<Duration>,
    http2_keep_alive_timeout: Option<Duration>,
    http2_keep_alive_while_idle: bool,
    tcp_nodelay: bool,
    http1_only: bool,
    https_only: bool,
    no_proxy: bool,
    no_redirects: bool,
    decompress: bool,
    proxy: Option<request::Proxy>,
    user_agent: Option<String>,
    default_headers: HeaderMap,
    identity: Option<request::Identity>,
    root_certificates: Vec<request::Certificate>,
}

macro_rules! option_setters {
    ($($name:ident: $ty:ty),* $(,)?) => {
        $(pub fn $name(mut self, value: $ty) -> Self {
            self.$name = Some(value);
            self
        })*
    };
}

macro_rules! flag_setters {
    ($($name:ident),* $(,)?) => {
        $(pub fn $name(mut self) -> Self {
            self.$name = true;
            self
        })*
    };
}

impl ClientOptions {
    option_setters!(
        timeout: Duration,
        connect_timeout: Duration,
        read_timeout: Duration,
        pool_idle_timeout: Duration,
        pool_max_idle_per_host: usize,
        tcp_keepalive: Duration,
        http2_keep_alive_interval: Duration,
        http2_keep_alive_timeout: Duration,
        identity: request::Identity,
        proxy: request::Proxy,
    );
    flag_setters!(
        http2_keep_alive_while_idle,
        tcp_nodelay,
        http1_only,
        https_only,
        no_proxy,
        no_redirects,
        decompress,
    );

    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = Some(value.into());
        self
    }

    /// Headers sent on every request. A credential header here fails the build.
    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        self.default_headers.extend(headers);
        self
    }

    pub fn add_root_certificate(mut self, cert: request::Certificate) -> Self {
        self.root_certificates.push(cert);
        self
    }

    fn apply(&self, mut b: request::ClientBuilder) -> request::ClientBuilder {
        if let Some(v) = self.timeout {
            b = b.timeout(v);
        }
        if let Some(v) = self.connect_timeout {
            b = b.connect_timeout(v);
        }
        if let Some(v) = self.read_timeout {
            b = b.read_timeout(v);
        }
        if let Some(v) = self.pool_idle_timeout {
            b = b.pool_idle_timeout(v);
        }
        if let Some(v) = self.pool_max_idle_per_host {
            b = b.pool_max_idle_per_host(v);
        }
        if let Some(v) = self.tcp_keepalive {
            b = b.tcp_keepalive(v);
        }
        if let Some(v) = self.http2_keep_alive_interval {
            b = b.http2_keep_alive_interval(v);
        }
        if let Some(v) = self.http2_keep_alive_timeout {
            b = b.http2_keep_alive_timeout(v);
        }
        if self.http2_keep_alive_while_idle {
            b = b.http2_keep_alive_while_idle(true);
        }
        if self.tcp_nodelay {
            b = b.tcp_nodelay(true);
        }
        if self.http1_only {
            b = b.http1_only();
        }
        if self.https_only {
            b = b.https_only(true);
        }
        if self.no_proxy {
            b = b.no_proxy();
        }
        if self.no_redirects {
            b = b.redirect(request::redirect::Policy::none());
        }
        if self.decompress {
            b = b.gzip(true).brotli(true).deflate(true);
        }
        if let Some(proxy) = &self.proxy {
            b = b.proxy(proxy.clone());
        }
        if let Some(ua) = &self.user_agent {
            b = b.user_agent(ua.as_str());
        }
        if !self.default_headers.is_empty() {
            b = b.default_headers(self.default_headers.clone());
        }
        if let Some(id) = &self.identity {
            b = b.identity(id.clone());
        }
        for cert in &self.root_certificates {
            b = b.add_root_certificate(cert.clone());
        }
        b
    }

    fn check(&self) -> Result<(), AuthRefusal> {
        match self
            .default_headers
            .keys()
            .find(|name| is_credential_header(name))
        {
            Some(name) => Err(AuthRefusal::CredentialHeaderOutsideAuth(
                name.as_str().to_owned(),
            )),
            None => Ok(()),
        }
    }
}

/// The async HTTP client. Every request names its credential; see [`Auth`].
#[derive(Clone, Debug)]
pub struct HttpClient {
    inner: request::Client,
}

impl HttpClient {
    /// Build a client with the grok TLS policy.
    pub fn build(options: ClientOptions) -> Result<Self, Error> {
        options.check()?;
        let inner = xai_grok_extra_ca::build_raw_http_client(|b| options.apply(b))?;
        Ok(Self { inner })
    }

    pub fn request(&self, method: Method, url: impl IntoUrl, auth: Auth) -> Request {
        let state = url.into_url().map_err(Error::from).map(|url| Pending {
            builder: self.inner.request(method, url.clone()),
            url,
            auth,
            extra_headers: HeaderMap::new(),
        });
        Request { state }
    }

    pub fn get(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::GET, url, auth)
    }
    pub fn post(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::POST, url, auth)
    }
    pub fn put(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::PUT, url, auth)
    }
    pub fn patch(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::PATCH, url, auth)
    }
    pub fn delete(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::DELETE, url, auth)
    }
    pub fn head(&self, url: impl IntoUrl, auth: Auth) -> Request {
        self.request(Method::HEAD, url, auth)
    }

    /// The underlying client, for a library that takes one and sends its own
    /// requests (an MCP transport, an OTLP exporter).
    pub fn library_client(&self) -> request::Client {
        self.inner.clone()
    }
}

struct Pending {
    builder: request::RequestBuilder,
    url: Url,
    auth: Auth,
    extra_headers: HeaderMap,
}

/// A request under construction. A failure at any step is held and returned by `send`.
pub struct Request {
    state: Result<Pending, Error>,
}

impl Request {
    fn map(mut self, f: impl FnOnce(request::RequestBuilder) -> request::RequestBuilder) -> Self {
        self.state = self.state.map(|mut p| {
            p.builder = f(p.builder);
            p
        });
        self
    }

    /// Set a header. A credential header is refused: name it as an [`Auth`].
    pub fn header<K, V>(mut self, name: K, value: V) -> Self
    where
        K: TryInto<HeaderName>,
        V: TryInto<HeaderValue>,
    {
        self.state = self.state.and_then(|mut p| {
            let name = name.try_into().map_err(|_| AuthRefusal::InvalidValue)?;
            if is_credential_header(&name) {
                return Err(
                    AuthRefusal::CredentialHeaderOutsideAuth(name.as_str().to_owned()).into(),
                );
            }
            let value = value.try_into().map_err(|_| AuthRefusal::InvalidValue)?;
            p.extra_headers.append(name, value);
            Ok(p)
        });
        self
    }

    /// Set several headers. A credential header is refused: name it as an [`Auth`].
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.state = self.state.and_then(|mut p| {
            if let Some(name) = headers.keys().find(|n| is_credential_header(n)) {
                return Err(
                    AuthRefusal::CredentialHeaderOutsideAuth(name.as_str().to_owned()).into(),
                );
            }
            p.extra_headers.extend(headers);
            Ok(p)
        });
        self
    }

    pub fn query<T: serde_like::Serialize + ?Sized>(self, query: &T) -> Self {
        self.map(|b| b.query(query))
    }
    pub fn json<T: serde_like::Serialize + ?Sized>(self, json: &T) -> Self {
        self.map(|b| b.json(json))
    }
    pub fn form<T: serde_like::Serialize + ?Sized>(self, form: &T) -> Self {
        self.map(|b| b.form(form))
    }
    pub fn body(self, body: impl Into<request::Body>) -> Self {
        self.map(|b| b.body(body))
    }
    pub fn multipart(self, form: request::multipart::Form) -> Self {
        self.map(|b| b.multipart(form))
    }
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|b| b.timeout(timeout))
    }
    pub fn version(self, version: request::Version) -> Self {
        self.map(|b| b.version(version))
    }

    /// A copy of this request, or `None` when its body is a stream.
    pub fn try_clone(&self) -> Option<Self> {
        match &self.state {
            Ok(p) => p.builder.try_clone().map(|builder| Self {
                state: Ok(Pending {
                    builder,
                    url: p.url.clone(),
                    auth: p.auth.clone(),
                    extra_headers: p.extra_headers.clone(),
                }),
            }),
            Err(_) => None,
        }
    }

    /// The URL this request goes to, when it parsed.
    pub fn url(&self) -> Option<&Url> {
        self.state.as_ref().ok().map(|p| &p.url)
    }

    /// Send the request. The response's extensions hold the
    /// [`crate::SentBearerSuffix`] of the credential that produced it.
    pub async fn send(self) -> Result<request::Response, Error> {
        let p = self.state?;
        let (source, retries) = match &p.auth {
            Auth::Source {
                source,
                retries_on_401,
            } => (Some(source.clone()), *retries_on_401),
            _ => (None, 0),
        };
        let backup = if retries > 0 {
            p.builder.try_clone()
        } else {
            None
        };
        let mut response = send_once(p.builder, &p.url, &p.auth, &p.extra_headers).await?;
        let (Some(source), Some(backup)) = (source, backup) else {
            return Ok(response);
        };
        for _ in 0..retries {
            if response.status() != request::StatusCode::UNAUTHORIZED
                || !source.refresh_after_unauthorized().await
            {
                break;
            }
            let Some(retry) = backup.try_clone() else {
                break;
            };
            tracing::info!(host = %auth::host_of(&p.url), "retrying after a 401 with a refreshed credential");
            response = send_once(retry, &p.url, &p.auth, &p.extra_headers).await?;
        }
        Ok(response)
    }
}

async fn send_once(
    builder: request::RequestBuilder,
    url: &Url,
    auth: &Auth,
    extra_headers: &HeaderMap,
) -> Result<request::Response, Error> {
    let fixed = auth.resolve();
    let (client, built) = builder.build_split();
    let mut built = built?;
    let headers = built.headers_mut();
    headers.extend(extra_headers.clone());
    auth::apply(&fixed, url, headers)?;
    auth::check_outgoing(url, headers)?;
    let mut response = client.execute(built).await?;
    if let Some(suffix) = auth::bearer_suffix(&fixed) {
        response.extensions_mut().insert(suffix);
    }
    Ok(response)
}

use serde as serde_like;
