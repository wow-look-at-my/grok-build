//! Tests of `retrying_auth` against a mock server.

use std::sync::{Arc, Mutex};

use xai_grok_net::{ClientOptions, HttpClient};

use crate::{
    AuthCredentialProvider, CredentialSnapshot, HttpAuth, RequestAuth, retrying_auth,
    sent_bearer_suffix,
};

struct MockProvider {
    token: Mutex<Option<String>>,
    refresh_result: bool,
    refresh_count: Mutex<u32>,
}

impl MockProvider {
    fn new(token: Option<&str>, refresh_result: bool) -> Self {
        Self {
            token: Mutex::new(token.map(|s| s.to_owned())),
            refresh_result,
            refresh_count: Mutex::new(0),
        }
    }

    fn refresh_count(&self) -> u32 {
        *self.refresh_count.lock().unwrap()
    }
}

impl HttpAuth for MockProvider {
    fn request_auth(&self, _: &str) -> RequestAuth {
        RequestAuth::none()
    }
}

#[async_trait::async_trait]
impl AuthCredentialProvider for MockProvider {
    fn snapshot(&self) -> CredentialSnapshot {
        CredentialSnapshot {
            token: self.token.lock().unwrap().clone(),
            ..Default::default()
        }
    }

    async fn refresh_after_unauthorized(&self) -> bool {
        *self.refresh_count.lock().unwrap() += 1;
        self.refresh_result
    }
}

/// Starts with a stale token. A refresh swaps in the fresh one.
struct SimulatedAuthManager {
    token: Mutex<Option<String>>,
    fresh_token: String,
    refresh_count: Mutex<u32>,
}

impl SimulatedAuthManager {
    fn simulated(stale: &str, fresh: &str) -> Self {
        Self {
            token: Mutex::new(Some(stale.to_owned())),
            fresh_token: fresh.to_owned(),
            refresh_count: Mutex::new(0),
        }
    }
}

impl HttpAuth for SimulatedAuthManager {
    fn request_auth(&self, _: &str) -> RequestAuth {
        RequestAuth::none()
    }
}

#[async_trait::async_trait]
impl AuthCredentialProvider for SimulatedAuthManager {
    fn snapshot(&self) -> CredentialSnapshot {
        CredentialSnapshot {
            token: self.token.lock().unwrap().clone(),
            ..Default::default()
        }
    }

    async fn refresh_after_unauthorized(&self) -> bool {
        *self.refresh_count.lock().unwrap() += 1;
        *self.token.lock().unwrap() = Some(self.fresh_token.clone());
        true
    }
}

fn client() -> HttpClient {
    HttpClient::build(ClientOptions::default()).unwrap()
}

async fn get(
    url: String,
    provider: Arc<dyn AuthCredentialProvider>,
    retries: u32,
) -> xai_grok_net::Response {
    let auth = retrying_auth(provider, &url, retries);
    client().get(url, auth.auth).send().await.unwrap()
}

#[tokio::test]
async fn test_401_no_refresh_returns_401() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/")
        .with_status(401)
        .expect(1)
        .create_async()
        .await;

    let p = Arc::new(MockProvider::new(Some("tok-0123456789"), false));
    let resp = get(server.url(), p.clone(), 1).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(p.refresh_count(), 1);
    m.assert_async().await;
}

#[tokio::test]
async fn test_e2e_stale_token_refreshed_and_retried() {
    let mut server = mockito::Server::new_async().await;
    let m401 = server
        .mock("GET", "/api")
        .match_header("authorization", "Bearer stale-token")
        .with_status(401)
        .create_async()
        .await;
    let m200 = server
        .mock("GET", "/api")
        .match_header("authorization", "Bearer fresh-token")
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;

    let p = Arc::new(SimulatedAuthManager::simulated(
        "stale-token",
        "fresh-token",
    ));
    let resp = get(format!("{}/api", server.url()), p.clone(), 1).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(*p.refresh_count.lock().unwrap(), 1);
    m401.assert_async().await;
    m200.assert_async().await;
}

#[tokio::test]
async fn test_e2e_auth_header_stamped_automatically() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/api")
        .match_header("authorization", "Bearer my-token-0001")
        .with_status(200)
        .create_async()
        .await;

    let p = Arc::new(MockProvider::new(Some("my-token-0001"), false));
    let resp = get(format!("{}/api", server.url()), p.clone(), 1).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(p.refresh_count(), 0);
    mock.assert_async().await;
}

#[tokio::test]
async fn sent_bearer_suffix_reports_last_bearer() {
    let mut server = mockito::Server::new_async().await;
    let m401 = server
        .mock("GET", "/api")
        .match_header("authorization", "Bearer stale-token")
        .with_status(401)
        .create_async()
        .await;
    let m200 = server
        .mock("GET", "/api")
        .match_header("authorization", "Bearer fresh-token")
        .with_status(200)
        .create_async()
        .await;

    let p = Arc::new(SimulatedAuthManager::simulated(
        "stale-token",
        "fresh-token",
    ));
    let resp = get(format!("{}/api", server.url()), p, 1).await;
    assert_eq!(resp.status(), 200);
    // A token of a bounded number of chars is its own suffix.
    assert_eq!(
        sent_bearer_suffix(&resp).expect("bearer was sent").0,
        "fresh-token"
    );
    m401.assert_async().await;
    m200.assert_async().await;
}

/// No credential means no suffix: attribution must see that nothing was sent.
#[tokio::test]
async fn sent_bearer_suffix_is_none_when_nothing_sent() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(401)
        .create_async()
        .await;

    let p = Arc::new(MockProvider::new(None, false));
    let resp = get(server.url(), p, 0).await;
    assert_eq!(resp.status(), 401);
    assert!(
        sent_bearer_suffix(&resp).is_none(),
        "no credential must mean no suffix"
    );
    m.assert_async().await;
}

#[tokio::test]
async fn test_max_retries_bounds_attempts() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/")
        .with_status(401)
        .expect(4)
        .create_async()
        .await;

    let p = Arc::new(MockProvider::new(Some("tok-0123456789"), true));
    let resp = get(server.url(), p.clone(), 3).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(p.refresh_count(), 3);
    m.assert_async().await;
}

/// A static key is an endpoint key, not the xAI session, so it may go to any host.
#[tokio::test]
async fn static_api_key_is_sent_as_an_api_key() {
    let provider = crate::StaticAuthCredentialProvider::api_key(Some("sk-static".into()));
    assert!(matches!(
        provider.current_auth(),
        xai_grok_net::Auth::ApiKey { ref key, .. } if key == "sk-static"
    ));
}

/// A snapshot with an `api_key_id` holds an xAI API key, and any other token is the session.
#[test]
fn default_current_auth_reads_the_snapshot_kind() {
    struct Snap(CredentialSnapshot);
    impl HttpAuth for Snap {
        fn request_auth(&self, _: &str) -> RequestAuth {
            RequestAuth::none()
        }
    }
    #[async_trait::async_trait]
    impl AuthCredentialProvider for Snap {
        fn snapshot(&self) -> CredentialSnapshot {
            self.0.clone()
        }
        async fn refresh_after_unauthorized(&self) -> bool {
            false
        }
    }
    let session = Snap(CredentialSnapshot {
        token: Some("session-token".into()),
        ..Default::default()
    });
    assert!(matches!(
        session.current_auth(),
        xai_grok_net::Auth::XaiSession(_)
    ));
    let key = Snap(CredentialSnapshot {
        token: Some("xai-key".into()),
        api_key_id: Some("id".into()),
        ..Default::default()
    });
    assert!(matches!(
        key.current_auth(),
        xai_grok_net::Auth::ApiKey { .. }
    ));
    let none = Snap(CredentialSnapshot::default());
    assert!(matches!(none.current_auth(), xai_grok_net::Auth::None));
}
