// client.rs holds the client code.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{IntoUrl, Method, Url};

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
    user_agent: Option<String>,
    default_headers: HeaderMap,
    identity: Option<reqwest::Identity>,
    root_certificates: Vec<reqwest::Certificate>,
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
        identity: reqwest::Identity,
    );
    flag_setters!(
        http2_keep_alive_while_idle,
        tcp_nodelay,
        http1_only,
        https_only,
        no_proxy,
        no_redirects,
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

    pub fn add_root_certificate(mut self, cert: reqwest::Certificate) -> Self {
        self.root_certificates.push(cert);
        self
    }

    fn apply(&self, mut b: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
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
            b = b.redirect(reqwest::redirect::Policy::none());
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
    inner: reqwest::Client,
}

impl HttpClient {
    /// Build a client with the grok TLS policy.
    pub fn build(options: ClientOptions) -> Result<Self, Error> {
        options.check()?;
        let inner = xai_grok_extra_ca::build_reqwest_client(|b| options.apply(b))?;
        Ok(Self { inner })
    }

    pub fn request(&self, method: Method, url: impl IntoUrl, auth: Auth) -> Request {
        let state = url.into_url().map_err(Error::from).and_then(|url| {
            let mut headers = HeaderMap::new();
            auth::apply(&auth, &url, &mut headers)?;
            Ok(Pending {
                builder: self.inner.request(method, url.clone()),
                url,
                auth_headers: headers,
                extra_headers: HeaderMap::new(),
            })
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
    pub fn library_client(&self) -> reqwest::Client {
        self.inner.clone()
    }
}

struct Pending {
    builder: reqwest::RequestBuilder,
    url: Url,
    auth_headers: HeaderMap,
    extra_headers: HeaderMap,
}

/// A request under construction. A failure at any step is held and returned by `send`.
pub struct Request {
    state: Result<Pending, Error>,
}

impl Request {
    fn map(mut self, f: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder) -> Self {
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
    pub fn body(self, body: impl Into<reqwest::Body>) -> Self {
        self.map(|b| b.body(body))
    }
    pub fn multipart(self, form: reqwest::multipart::Form) -> Self {
        self.map(|b| b.multipart(form))
    }
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|b| b.timeout(timeout))
    }
    pub fn version(self, version: reqwest::Version) -> Self {
        self.map(|b| b.version(version))
    }

    /// A copy of this request, or `None` when its body is a stream.
    pub fn try_clone(&self) -> Option<Self> {
        match &self.state {
            Ok(p) => p.builder.try_clone().map(|builder| Self {
                state: Ok(Pending {
                    builder,
                    url: p.url.clone(),
                    auth_headers: p.auth_headers.clone(),
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

    pub async fn send(self) -> Result<reqwest::Response, Error> {
        let request = self.finish()?;
        Ok(request.0.execute(request.1).await?)
    }

    fn finish(self) -> Result<(reqwest::Client, reqwest::Request), Error> {
        let p = self.state?;
        let (client, built) = p.builder.build_split();
        let mut built = built?;
        let headers = built.headers_mut();
        headers.extend(p.extra_headers);
        headers.extend(p.auth_headers);
        auth::check_outgoing(&p.url, headers)?;
        Ok((client, built))
    }
}

use serde as serde_like;
