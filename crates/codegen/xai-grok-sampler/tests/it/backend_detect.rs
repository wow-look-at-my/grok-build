//! A model the provider refuses on the configured API backend is sent again
//! on the others.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::stream;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use xai_grok_sampler::{
    ApiBackend, RequestId, RetryPolicy, SamplerActor, SamplerConfig, SamplingClient, SamplingEvent,
    detected_backend,
};
use xai_grok_sampling_types::{ContentPart, ConversationItem, ConversationRequest, UserItem};
use xai_grok_test_support::sse;

const RESPONSES_ONLY: &str =
    "This model is only supported in v1/responses and not in v1/chat/completions.";

struct Server {
    base_url: String,
    shutdown: oneshot::Sender<()>,
}

async fn serve(app: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, shutdown_rx) = oneshot::channel::<()>();
    xai_spawn::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Server {
        base_url: format!("http://{addr}/v1"),
        shutdown,
    }
}

fn config(base_url: &str, backend: ApiBackend) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url: base_url.to_owned(),
        model: "responses-only-model".into(),
        api_backend: backend,
        context_window: 128_000,
        max_retries: Some(3),
        idle_timeout_secs: Some(30),
        ..Default::default()
    }
}

fn hi() -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from("hi"),
            }],
            ..Default::default()
        })],
        ..Default::default()
    }
}

fn openai_error(status: StatusCode, message: &str) -> Response {
    let body = json!({
        "error": {
            "message": message,
            "type": "invalid_request_error",
            "param": "model",
            "code": null,
        }
    });
    (status, axum::Json(body)).into_response()
}

fn responses_answer(text: &str) -> Response {
    let events: Vec<Result<Event, Infallible>> =
        sse::responses_api_script_exact(text, "responses-only-model")
            .into_iter()
            .map(|e| {
                let event = Event::default().data(e.data);
                Ok(match e.event {
                    Some(name) => event.event(name),
                    None => event,
                })
            })
            .collect();
    Sse::new(stream::iter(events)).into_response()
}

#[derive(Clone, Default)]
struct Hits {
    chat: Arc<AtomicU32>,
    responses: Arc<AtomicU32>,
    messages: Arc<AtomicU32>,
}

impl Hits {
    fn counts(&self) -> (u32, u32, u32) {
        (
            self.chat.load(Ordering::SeqCst),
            self.responses.load(Ordering::SeqCst),
            self.messages.load(Ordering::SeqCst),
        )
    }
}

/// Chat Completions refuses the model, Responses serves it, and Messages has
/// no route.
fn provider(hits: &Hits, responses_ok: bool) -> Router {
    let chat = Arc::clone(&hits.chat);
    let responses = Arc::clone(&hits.responses);
    let messages = Arc::clone(&hits.messages);
    Router::new()
        .route(
            "/v1/chat/completions",
            post(move || {
                chat.fetch_add(1, Ordering::SeqCst);
                async { openai_error(StatusCode::NOT_FOUND, RESPONSES_ONLY) }
            }),
        )
        .route(
            "/v1/responses",
            post(move || {
                responses.fetch_add(1, Ordering::SeqCst);
                async move {
                    if responses_ok {
                        responses_answer("served on responses")
                    } else {
                        (StatusCode::NOT_FOUND, "Not Found").into_response()
                    }
                }
            }),
        )
        .route(
            "/v1/messages",
            post(move || {
                messages.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::METHOD_NOT_ALLOWED.into_response() }
            }),
        )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_moves_to_the_backend_the_error_names_and_keeps_it() {
    let hits = Hits::default();
    let server = serve(provider(&hits, true)).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        config(&server.base_url, ApiBackend::ChatCompletions),
        RetryPolicy::default(),
        event_tx,
    );

    let (response, _) = handle
        .submit_and_collect(RequestId::from("first"), hi())
        .await
        .expect("the request must be served on the Responses backend");
    assert_eq!(response.assistant_text(), "served on responses");
    assert_eq!(hits.counts(), (1, 1, 0), "one refusal, then Responses");
    assert_eq!(
        detected_backend(&server.base_url, "responses-only-model"),
        Some(ApiBackend::Responses)
    );

    handle
        .submit_and_collect(RequestId::from("second"), hi())
        .await
        .expect("the second request must start on Responses");
    assert_eq!(
        hits.counts(),
        (1, 2, 0),
        "the detected backend is used without another refusal"
    );
    let _ = server.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn side_call_moves_to_the_backend_the_error_names() {
    let hits = Hits::default();
    let server = serve(provider(&hits, true)).await;
    let client =
        SamplingClient::new(config(&server.base_url, ApiBackend::ChatCompletions)).unwrap();

    let response = client
        .conversation_collect(hi())
        .await
        .expect("the side call must be served on the Responses backend");
    assert_eq!(response.assistant_text(), "served on responses");
    assert_eq!(hits.counts(), (1, 1, 0));

    let fresh = SamplingClient::new(config(&server.base_url, ApiBackend::ChatCompletions)).unwrap();
    assert_eq!(fresh.api_backend(), ApiBackend::Responses);
    let _ = server.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_backend_failing_reports_the_first_refusal() {
    let hits = Hits::default();
    let server = serve(provider(&hits, false)).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        config(&server.base_url, ApiBackend::ChatCompletions),
        RetryPolicy::default(),
        event_tx,
    );

    let err = handle
        .submit_and_collect(RequestId::from("doomed"), hi())
        .await
        .expect_err("no backend serves the model");
    assert!(
        err.to_string().contains(RESPONSES_ONLY),
        "the user sees the provider's refusal, not a probe's 404: {err}"
    );
    assert_eq!(hits.counts(), (1, 1, 1), "each backend is tried once");
    assert_eq!(
        detected_backend(&server.base_url, "responses-only-model"),
        None
    );

    let mut failed = None;
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(5), event_rx.recv()).await
    {
        if let SamplingEvent::Failed { error, .. } = event {
            failed = Some(error);
            break;
        }
    }
    let failed = failed.expect("a Failed event");
    assert!(
        failed.message.contains(RESPONSES_ONLY),
        "{}",
        failed.message
    );
    let _ = server.shutdown.send(());
}
