// blocking.rs holds the blocking code.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{IntoUrl, Method, Url};

use crate::auth::{self, Auth, is_credential_header};
use crate::error::{AuthRefusal, Error};

/// How a blocking client is built. None of it carries a credential.
#[derive(Clone, Debug, Default)]
pub struct BlockingOptions {
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub pool_idle_timeout: Option<Duration>,
    pub tcp_keepalive: Option<Duration>,
    pub user_agent: Option<String>,
}

/// The blocking HTTP client. Same credential rules as [`crate::HttpClient`].
#[derive(Clone, Debug)]
pub struct BlockingHttpClient {
    inner: reqwest::blocking::Client,
}

impl BlockingHttpClient {
    /// Build a client with the grok TLS policy.
    pub fn build(timeout: Option<Duration>, user_agent: Option<&str>) -> Result<Self, Error> {
        Self::build_with(BlockingOptions {
            timeout,
            user_agent: user_agent.map(str::to_owned),
            ..Default::default()
        })
    }

    /// Build a client with the grok TLS policy and every option in `options`.
    pub fn build_with(options: BlockingOptions) -> Result<Self, Error> {
        let inner = xai_grok_extra_ca::build_blocking_reqwest_client(|mut b| {
            if let Some(t) = options.timeout {
                b = b.timeout(t);
            }
            if let Some(t) = options.connect_timeout {
                b = b.connect_timeout(t);
            }
            if let Some(t) = options.pool_idle_timeout {
                b = b.pool_idle_timeout(t);
            }
            if let Some(t) = options.tcp_keepalive {
                b = b.tcp_keepalive(t);
            }
            if let Some(ua) = &options.user_agent {
                b = b.user_agent(ua.as_str());
            }
            b
        })?;
        Ok(Self { inner })
    }

    pub fn request(&self, method: Method, url: impl IntoUrl, auth: Auth) -> BlockingRequest {
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
        BlockingRequest { state }
    }

    pub fn get(&self, url: impl IntoUrl, auth: Auth) -> BlockingRequest {
        self.request(Method::GET, url, auth)
    }
    pub fn post(&self, url: impl IntoUrl, auth: Auth) -> BlockingRequest {
        self.request(Method::POST, url, auth)
    }
}

struct Pending {
    builder: reqwest::blocking::RequestBuilder,
    url: Url,
    auth_headers: HeaderMap,
    extra_headers: HeaderMap,
}

/// A blocking request under construction. A failure at any step is held and returned by `send`.
pub struct BlockingRequest {
    state: Result<Pending, Error>,
}

impl BlockingRequest {
    fn map(
        mut self,
        f: impl FnOnce(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
    ) -> Self {
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

    pub fn query<T: serde::Serialize + ?Sized>(self, query: &T) -> Self {
        self.map(|b| b.query(query))
    }
    pub fn json<T: serde::Serialize + ?Sized>(self, json: &T) -> Self {
        self.map(|b| b.json(json))
    }
    pub fn form<T: serde::Serialize + ?Sized>(self, form: &T) -> Self {
        self.map(|b| b.form(form))
    }
    pub fn body(self, body: impl Into<reqwest::blocking::Body>) -> Self {
        self.map(|b| b.body(body))
    }
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|b| b.timeout(timeout))
    }

    pub fn send(self) -> Result<reqwest::blocking::Response, Error> {
        let p = self.state?;
        let (client, built) = p.builder.build_split();
        let mut built = built?;
        let headers = built.headers_mut();
        headers.extend(p.extra_headers);
        headers.extend(p.auth_headers);
        auth::check_outgoing(&p.url, headers)?;
        Ok(client.execute(built)?)
    }
}
