// visibility.rs holds the visibility code.

use xai_grok_net::Auth;
use xai_grok_net::header::HeaderMap;

/// The credential for one request, and the plain headers that go with it.
#[derive(Clone, Debug)]
pub struct RequestAuth {
    /// The credential. `xai-grok-net` writes it into the request.
    pub auth: Auth,
    /// Headers that are not credentials, such as `X-XAI-Token-Auth`.
    pub headers: HeaderMap,
}

impl RequestAuth {
    /// No credential and no headers.
    pub fn none() -> Self {
        Self {
            auth: Auth::None,
            headers: HeaderMap::new(),
        }
    }

    /// Put the plain headers on `request`. The credential goes in when the request is made.
    pub fn headers_on(&self, request: xai_grok_net::Request) -> xai_grok_net::Request {
        request.headers(self.headers.clone())
    }
}

/// Lets data-collector code build a request without shell types.
pub trait HttpAuth: Send + Sync {
    /// The credential and plain headers for a request to `base_url`.
    fn request_auth(&self, base_url: &str) -> RequestAuth;
}
