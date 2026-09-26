//! Integration tests for the actor and request_task layer.
//!
//! They live in `tests/` because they need a real `tokio::runtime` and a mock axum HTTP server for the `SamplingClient` to talk to.
//! Happy-path SSE payloads come from `xai_grok_test_support::sse`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use xai_grok_sampler::{
    ApiBackend, RequestId, RetryPolicy, SamplerActor, SamplerConfig, SamplingChannel,
    SamplingErrorKind, SamplingEvent, StripReason,
};
use xai_grok_sampling_types::{
    ConversationItem, ConversationRequest, DoomLoopRecoveryPolicy, INVALID_IMAGE_ERROR_CODE,
    OutputRateFloorPolicy, SyntheticReason, UserItem,
};
use xai_grok_test_support::{SseEvent, sse};

// ---------------------------------------------------------------------------
// Mock server harness
// ---------------------------------------------------------------------------

struct MockServer {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
}

impl MockServer {
    async fn spawn(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        // Give the server a moment to start.
        tokio::time::sleep(Duration::from_millis(20)).await;
        Self { addr, shutdown_tx }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

// ---------------------------------------------------------------------------
// Config + request helpers
// ---------------------------------------------------------------------------

fn test_config(base_url: String, model: &str) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url,
        model: model.into(),
        max_completion_tokens: Some(1024),
        context_window: 128_000,
        // Keep retries minimal so tests don't take forever.
        max_retries: Some(2),
        idle_timeout_secs: Some(30),
        ..Default::default()
    }
}

fn user_request(text: &str) -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![xai_grok_sampling_types::ContentPart::Text {
                text: std::sync::Arc::<str>::from(text),
            }],
            synthetic_reason: SyntheticReason::Human,
            ..Default::default()
        })],
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// SSE generators
// ---------------------------------------------------------------------------

/// Render test-helper [`SseEvent`]s (optional `event:` name and `data:`) as axum SSE events for this file's router-based harness.
fn sse_events_to_axum(events: Vec<SseEvent>) -> Vec<Event> {
    events
        .into_iter()
        .map(|e| {
            let ev = Event::default().data(e.data);
            match e.event {
                Some(name) => ev.event(name),
                None => ev,
            }
        })
        .collect()
}

fn text_chunk_event(content: &str, finish: bool) -> Event {
    let chunk = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": content },
            "finish_reason": if finish { json!("stop") } else { json!(null) }
        }]
    });
    Event::default().data(chunk.to_string())
}

// ---------------------------------------------------------------------------
// Actor lifecycle
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_then_active_count_zero_then_cancel_unknown_is_noop() {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let cfg = test_config("http://127.0.0.1:0/v1".into(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);
    assert_eq!(handle.active_count().await, 0);
    handle.cancel(RequestId::from("nonexistent"));
    assert_eq!(handle.active_count().await, 0);
}

// ---------------------------------------------------------------------------
// Submit + event flow
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_emits_started_first_token_channel_completed() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let events = sse::chat_completion_events("hello world", "test-model");
            Sse::new(stream::iter(
                events.into_iter().map(Ok::<_, std::convert::Infallible>),
            ))
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-1");
    handle.submit(rid.clone(), user_request("hi"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(5)).await;
    server.shutdown();

    assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SamplingEvent::FirstToken { .. }))
    );

    let texts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            SamplingEvent::ChannelToken {
                channel: SamplingChannel::Text,
                text,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts.join(""), "hello world");

    match events.last().unwrap() {
        SamplingEvent::Completed {
            request_id,
            response,
            ..
        } => {
            assert_eq!(request_id, &rid);
            if let Some(a) = response.assistant() {
                assert_eq!(a.content.as_ref(), "hello world");
            } else {
                panic!("expected Assistant message");
            }
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// submit_and_collect
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_and_collect_returns_response() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let events = sse::chat_completion_events("collected response", "test-model");
            Sse::new(stream::iter(
                events.into_iter().map(Ok::<_, std::convert::Infallible>),
            ))
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-collect");
    let result = handle
        .submit_and_collect(rid, user_request("hi"))
        .await
        .expect("collected ok");
    server.shutdown();

    let (response, _metrics) = result;
    let a = response.assistant().expect("assistant item present");
    assert_eq!(a.content.as_ref(), "collected response");
}

/// A stream shaped the way Bifrost shapes one: the trailing usage chunk has no
/// choices, and the gateway writes an unset slice as `"choices": null`. Failing
/// that parse turned every turn on every model behind the gateway into
/// "Couldn't read the response", so this drives the whole SSE path, not just
/// the chunk type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn null_choices_usage_chunk_completes_the_turn() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let usage_chunk = json!({
                "id": "chatcmpl-test",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "test-model",
                "choices": null,
                "system_fingerprint": "",
                "usage": {
                    "prompt_tokens": 18,
                    "completion_tokens": 10,
                    "total_tokens": 28
                }
            });
            let events = vec![
                text_chunk_event("hello from the gateway", false),
                text_chunk_event("", true),
                Event::default().data(usage_chunk.to_string()),
            ];
            Sse::new(stream::iter(
                events.into_iter().map(Ok::<_, std::convert::Infallible>),
            ))
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-null-choices"), user_request("hi"))
        .await
        .expect("a null-choices usage chunk must not fail the turn");
    server.shutdown();

    let (response, _metrics) = result;
    let a = response.assistant().expect("assistant item present");
    assert_eq!(a.content.as_ref(), "hello from the gateway");
    // The usage on that chunk is what the turn would otherwise lose.
    assert_eq!(response.usage.map(|u| u.total_tokens), Some(28));
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_in_flight_request_terminates_task() {
    // The server yields one chunk then hangs
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            let stream = stream::iter(vec![Ok::<_, std::convert::Infallible>(text_chunk_event(
                "starting", false,
            ))])
            .chain(stream::pending());
            Sse::new(stream)
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-cancel");
    handle.submit(rid.clone(), user_request("hi"));

    // Wait for the first token to arrive so we know the request is in flight.
    let _ = await_event_matching(
        &mut event_rx,
        |e| matches!(e, SamplingEvent::FirstToken { .. }),
        Duration::from_secs(5),
    )
    .await
    .expect("first token");

    handle.cancel(rid.clone());

    // Expect a Failed event with the cancellation message.
    let failed = await_event_matching(
        &mut event_rx,
        |e| matches!(e, SamplingEvent::Failed { .. }),
        Duration::from_secs(5),
    )
    .await
    .expect("Failed event after cancel");

    if let SamplingEvent::Failed { error, .. } = failed {
        assert!(error.message.contains("cancelled"));
    }

    // Wait briefly for the task to clean up.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(handle.active_count().await, 0);
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Concurrent requests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_requests_complete_with_correct_request_ids() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let events = sse::chat_completion_events(&format!("response-{n}"), "test-model");
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid_a = RequestId::from("req-a");
    let rid_b = RequestId::from("req-b");
    handle.submit(rid_a.clone(), user_request("a"));
    handle.submit(rid_b.clone(), user_request("b"));

    // Drain until we see Completed for both.
    let mut completed_a = false;
    let mut completed_b = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(completed_a && completed_b) {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            panic!(
                "timed out waiting for both requests to complete: a={completed_a}, b={completed_b}"
            );
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, event_rx.recv()).await {
            Ok(Some(SamplingEvent::Completed { request_id, .. })) if request_id == rid_a => {
                completed_a = true;
            }
            Ok(Some(SamplingEvent::Completed { request_id, .. })) if request_id == rid_b => {
                completed_b = true;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timeout"),
        }
    }
    server.shutdown();
}

// ---------------------------------------------------------------------------
// Retry on transient transport error
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_on_500_then_succeeds() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // First attempt: server error.
                    Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({ "error": { "message": "transient" } }).to_string(),
                    ))
                } else {
                    // Subsequent attempts: success.
                    let events = sse::chat_completion_events("ok", "test-model");
                    Ok(Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    )))
                }
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    // Lots of retries available; backoff is jittered around 2s on first retry, so this test takes a bit to run
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-retry");
    handle.submit(rid.clone(), user_request("hi"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    let saw_retrying = events
        .iter()
        .any(|e| matches!(e, SamplingEvent::Retrying { .. }));
    assert!(saw_retrying, "expected at least one Retrying event");

    match events.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            if let Some(a) = response.assistant() {
                assert_eq!(a.content.as_ref(), "ok");
            }
        }
        other => panic!("expected Completed after retry, got {other:?}"),
    }

    assert!(
        counter.load(Ordering::SeqCst) >= 2,
        "server hit at least twice"
    );
}

/// A coded `invalid_image` 400 strips the image, emits ServerRejected, retries, and completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_image_code_strips_and_retries() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bodies_handler = Arc::clone(&bodies);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let bodies = Arc::clone(&bodies_handler);
            async move {
                let n = {
                    let mut b = bodies.lock().unwrap();
                    b.push(body);
                    b.len()
                };
                if n == 1 {
                    // The FLAT envelope the xAI API's non-stream rejections actually use; the message alone must not matter
                    Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::BAD_REQUEST,
                        json!({
                            "code": INVALID_IMAGE_ERROR_CODE,
                            "error": "some future wording without the legacy phrase",
                        })
                        .to_string(),
                    ))
                } else {
                    let events = sse::chat_completion_events("recovered", "test-model");
                    Ok(Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    )))
                }
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-image-code-strip"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    assert!(
        events.iter().any(|e| match e {
            SamplingEvent::ImagesStripped {
                stripped_urls,
                reason: xai_grok_sampler::StripReason::ServerRejected,
                ..
            } => stripped_urls.len() == 1 && stripped_urls[0].as_ref() == IMAGE_URI,
            _ => false,
        }),
        "expected server-rejected ImagesStripped carrying the poisoned URL, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Completed { .. })),
        "expected Completed after strip-retry"
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "one rejection, one strip-retry");
    assert!(bodies[0].contains(IMAGE_URI), "first attempt sends image");
    assert!(
        !bodies[1].contains(IMAGE_URI),
        "strip-retry must not resend the image"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_invalid_image_strips_as_server_rejected() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bodies_handler = Arc::clone(&bodies);
    let app = Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let bodies = Arc::clone(&bodies_handler);
            async move {
                let n = {
                    let mut b = bodies.lock().unwrap();
                    b.push(body);
                    b.len()
                };
                if n == 1 {
                    Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::BAD_REQUEST,
                        json!({
                            "code": INVALID_IMAGE_ERROR_CODE,
                            "error": "Invalid PNG image.",
                        })
                        .to_string(),
                    ))
                } else {
                    let events = sse_events_to_axum(sse::responses_api_reasoning_and_text_events(
                        "ok",
                        "recovered",
                        "test-model",
                    ));
                    Ok(Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    )))
                }
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        responses_config(server.base_url(), None),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-responses-invalid-image"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    assert!(
        events.iter().any(|e| match e {
            SamplingEvent::ImagesStripped {
                stripped_urls,
                reason: StripReason::ServerRejected,
                ..
            } => stripped_urls.len() == 1 && stripped_urls[0].as_ref() == IMAGE_URI,
            _ => false,
        }),
        "Responses invalid_image must strip as ServerRejected, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Completed { .. })),
        "expected Completed after strip-retry"
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "one rejection, one strip-retry");
    assert!(bodies[0].contains(IMAGE_URI), "first attempt sends image");
    assert!(
        !bodies[1].contains(IMAGE_URI),
        "strip-retry must not resend the image"
    );
}

/// A legacy-phrase 400 with no code still strips and recovers, but the reason is `PayloadHeuristic`.
/// Without the deterministic code the server blamed nothing specific, so the strip must stay request-local.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_phrase_400_strips_as_heuristic() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": {
                                "message": "Could not process image",
                                "type": "invalid_request_error",
                            }
                        })
                        .to_string(),
                    ))
                } else {
                    let events = sse::chat_completion_events("recovered", "test-model");
                    Ok(Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    )))
                }
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-legacy-phrase-strip"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    assert!(
        events.iter().any(|e| matches!(
            e,
            SamplingEvent::ImagesStripped {
                reason: StripReason::PayloadHeuristic,
                ..
            }
        )),
        "codeless legacy-phrase 400 must strip as PayloadHeuristic, got {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SamplingEvent::ImagesStripped {
                reason: StripReason::ServerRejected,
                ..
            }
        )),
        "no deterministic code, so never ServerRejected: {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Completed { .. })),
        "expected Completed after strip-retry"
    );
}

/// Guards that `user_facing_api_error_message` keeps the `.image.source` path in a codeless `invalid_request_error`.
/// That way the codeless image-strip recovery fires on many-image dimension 400s instead of hard-failing every turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_image_dimension_400_strips_as_heuristic() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::BAD_REQUEST,
                        json!({
                            "error": {
                                "message": "messages.0.content.4.image.source.base64.data: At least one of the image dimensions exceed max allowed size for many-image requests: 2000 pixels",
                                "type": "invalid_request_error",
                            }
                        })
                        .to_string(),
                    ))
                } else {
                    let events = sse::chat_completion_events("recovered", "test-model");
                    Ok(Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    )))
                }
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-many-image-dimension-strip"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    assert!(
        events.iter().any(|e| matches!(
            e,
            SamplingEvent::ImagesStripped {
                reason: StripReason::PayloadHeuristic,
                ..
            }
        )),
        "codeless many-image dimension 400 must strip as PayloadHeuristic, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Completed { .. })),
        "expected Completed after strip-retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn image_400_with_nothing_left_to_strip_is_fatal_after_one_cycle() {
    // `stripped == 0` is the only bound on the strip-retry loop.
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<Sse<futures_util::stream::Empty<Result<Event, std::convert::Infallible>>>, _>(
                    (
                        StatusCode::BAD_REQUEST,
                        json!({
                            "code": INVALID_IMAGE_ERROR_CODE,
                            "error": "Base64 string of provided image cannot be decoded.",
                        })
                        .to_string(),
                    ),
                )
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image("data:image/png;base64,cG9pc29uZWQ=");
    }
    handle.submit(RequestId::from("req-strip-exhausted"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    server.shutdown();

    let strips = events
        .iter()
        .filter(|e| matches!(e, SamplingEvent::ImagesStripped { .. }))
        .count();
    assert_eq!(strips, 1, "exactly one strip cycle");
    assert!(
        matches!(events.last(), Some(SamplingEvent::Failed { .. })),
        "second image 400 with nothing left to strip must be fatal, got {events:?}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "one rejection, one strip-retry, then stop"
    );
}

/// RST with a zero retry budget: the decision is Fatal, so the proactive heuristic strip must NOT run.
/// There is no mutation, no ImagesStripped event, and no "left out of the retry" note for a retry that never happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_decision_does_not_strip_or_emit_images_stripped() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        // RST every connection: peek then drop (see xai-grok-http).
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((sock, _)) = accepted else { break };
                    let mut buf = [0u8; 64];
                    let _ = sock.peek(&mut buf).await;
                    drop(sock);
                }
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut config = test_config(format!("http://{addr}/v1"), "test-model");
    config.max_retries = Some(0);
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(config, RetryPolicy::default(), event_tx);

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-fatal-no-strip"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    let _ = shutdown_tx.send(());

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::ImagesStripped { .. })),
        "a Fatal decision must not strip or emit ImagesStripped, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Failed { .. })),
        "expected terminal Failed, got {events:?}"
    );
}

/// An RST mid-upload (nginx-style 413) emits PayloadHeuristic and strips the request only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_reset_emits_payload_heuristic_and_strips_request() {
    const IMAGE_URI: &str = "data:image/png;base64,cG9pc29uZWQ=";
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bodies_handler = Arc::clone(&bodies);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        // Peek then drop so the peer sees RST (see xai-grok-http).
        if let Ok((sock, _)) = listener.accept().await {
            let mut buf = [0u8; 64];
            let _ = sock.peek(&mut buf).await;
            drop(sock);
        }
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |body: String| {
                let bodies = Arc::clone(&bodies_handler);
                async move {
                    bodies.lock().unwrap().push(body);
                    let events = sse::chat_completion_events("recovered", "test-model");
                    Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    ))
                }
            }),
        );
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(format!("http://{addr}/v1"), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image(IMAGE_URI);
    }
    handle.submit(RequestId::from("req-heuristic-strip"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    let _ = shutdown_tx.send(());

    assert!(
        events.iter().any(|e| match e {
            SamplingEvent::ImagesStripped {
                stripped_urls,
                reason: StripReason::PayloadHeuristic,
                ..
            } => stripped_urls.len() == 1 && stripped_urls[0].as_ref() == IMAGE_URI,
            _ => false,
        }),
        "connection reset must emit PayloadHeuristic ImagesStripped, got {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SamplingEvent::ImagesStripped {
                reason: StripReason::ServerRejected,
                ..
            }
        )),
        "heuristic path must not be labeled ServerRejected, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Completed { .. })),
        "strip-retry must complete, got {events:?}"
    );
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1, "only the post-strip retry hits HTTP");
    assert!(
        !bodies[0].contains(IMAGE_URI),
        "in-flight request must be stripped before the retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_failure_does_not_emit_images_stripped() {
    // Connection refused is `is_connect`, not a body-upload reset.
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config("http://127.0.0.1:1/v1".into(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let mut request = user_request("what is in this image?");
    if let Some(ConversationItem::User(u)) = request.items.first_mut() {
        u.add_image("data:image/png;base64,cG9pc29uZWQ=");
    }
    handle.submit(RequestId::from("req-connect-fail"), request);

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(15)).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::ImagesStripped { .. })),
        "connect failure must not strip images, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(SamplingEvent::Failed { .. })),
        "exhausted connect retries must be Failed, got {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Rate-limit thresholds
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limit_exhausts_at_default_threshold_and_yields_failed() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<
                    Sse<
                        futures_util::stream::Iter<
                            std::vec::IntoIter<Result<Event, std::convert::Infallible>>,
                        >,
                    >,
                    (StatusCode, String),
                >((
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({ "error": { "message": "slow down" } }).to_string(),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-429-default");
    handle.submit(rid, user_request("hi"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(60)).await;
    server.shutdown();

    match events.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, SamplingErrorKind::RateLimited);
            assert_eq!(error.status_code, Some(429));
        }
        other => panic!("expected Failed(RateLimited), got {other:?}"),
    }

    // The request task awaits and classifies each wire attempt before starting the next, so scheduling cannot add another request.
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "the default threshold permits one retry after the initial request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_rate_limit_threshold_controls_total_wire_attempts() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "0")],
                    json!({ "error": { "message": "slow down" } }).to_string(),
                )
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.max_retries = Some(6);
    cfg.rate_limit_retry_threshold = Some(4);
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-429");
    handle.submit(rid.clone(), user_request("hi"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(60)).await;
    server.shutdown();

    match events.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, SamplingErrorKind::RateLimited);
            assert_eq!(error.status_code, Some(429));
        }
        other => panic!("expected Failed(RateLimited), got {other:?}"),
    }

    let hits = counter.load(Ordering::SeqCst);
    assert_eq!(
        hits, 4,
        "the configured threshold is a total-attempt ceiling and must override the policy default of 2"
    );
}

// ---------------------------------------------------------------------------
// Auth error -> EmitToSession (immediate)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_401_emits_failed_immediately_no_retry() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<
                    Sse<
                        futures_util::stream::Iter<
                            std::vec::IntoIter<Result<Event, std::convert::Infallible>>,
                        >,
                    >,
                    (StatusCode, String),
                >((StatusCode::UNAUTHORIZED, "unauthorized".to_string()))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "test-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-auth");
    handle.submit(rid.clone(), user_request("hi"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(5)).await;
    server.shutdown();

    // The session owns auth errors: `classify_error` returns `EmitToSession`, so the actor emits Failed immediately without retrying
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Retrying { .. }))
    );
    match events.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, SamplingErrorKind::Auth);
        }
        other => panic!("expected Failed(Auth), got {other:?}"),
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1, "no retries on 401");
}

// ---------------------------------------------------------------------------
// Anthropic Messages API: refusal stop_reason + mid-stream parse failure
// ---------------------------------------------------------------------------

fn messages_config(base_url: String) -> SamplerConfig {
    let mut cfg = test_config(base_url, "messages-compatible-model");
    cfg.api_backend = ApiBackend::Messages;
    cfg
}

/// Regression for the refusal-stop_reason incident.
/// A well-formed stream terminated by `stop_reason: "refusal"` must produce a successful completion from EXACTLY ONE request, no retry storm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_refusal_stream_completes_with_single_request() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/messages",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let events = sse::messages_api_events(
                    "I can't help with that.",
                    "messages-compatible-model",
                    "refusal",
                );
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        messages_config(server.base_url()),
        RetryPolicy::default(),
        event_tx,
    );

    let result = handle
        .submit_and_collect(RequestId::from("req-refusal"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("refusal-terminated turn must complete");
    let a = response.assistant().expect("assistant item present");
    assert_eq!(a.content.as_ref(), "I can't help with that.");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "refusal must not trigger retries"
    );
}

/// Empty-bodied refusal: `message_start → message_delta(refusal) → message_stop` with zero content blocks must complete from exactly one request.
/// The content-less response must not be classified as a retryable EmptyResponse.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_empty_refusal_completes_without_retry() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/messages",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut events =
                    sse::messages_api_events("", "messages-compatible-model", "refusal");
                // Drop the content block events; keep start/delta/stop only.
                events.drain(1..4);
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        messages_config(server.base_url()),
        RetryPolicy::default(),
        event_tx,
    );

    handle.submit(RequestId::from("req-empty-refusal"), user_request("hi"));
    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(10)).await;
    server.shutdown();

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Retrying { .. })),
        "content-less refusal must not be retried"
    );
    match events.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            assert_eq!(
                response.stop_reason,
                Some(xai_grok_sampling_types::StopReason::ContentFilter)
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1, "exactly one request");
}

/// A mid-stream event that fails serde (after a valid `message_start`) is a deterministic response-parse failure.
/// It is Fatal on the first attempt and surfaces as a non-retryable Serialization error, never a retry storm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn messages_unparseable_event_is_fatal_without_retry() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app =
        Router::new().route(
            "/v1/messages",
            post(move || {
                let counter = Arc::clone(&counter_handler);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut events =
                        sse::messages_api_events("hello", "messages-compatible-model", "end_turn");
                    // Replace the tail with a `message_delta` missing the required `delta` field, which fails MessageStreamEvent serde
                    events.truncate(4);
                    events.push(Event::default().data(
                        json!({"type":"message_delta","usage":{"output_tokens":1}}).to_string(),
                    ));
                    Sse::new(stream::iter(
                        events.into_iter().map(Ok::<_, std::convert::Infallible>),
                    ))
                }
            }),
        );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        messages_config(server.base_url()),
        RetryPolicy::default(),
        event_tx,
    );

    handle.submit(RequestId::from("req-bad-event"), user_request("hi"));
    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(10)).await;
    server.shutdown();

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Retrying { .. })),
        "serde failures must not be retried"
    );
    match events.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, SamplingErrorKind::Serialization);
            assert!(!error.is_retryable, "surfaced info must be non-retryable");
        }
        other => panic!("expected Failed(Serialization), got {other:?}"),
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1, "exactly one attempt");
}

// ---------------------------------------------------------------------------
// UpdateConfig invalidates cache + applies to subsequent requests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_config_changes_subsequent_request_model() {
    use std::sync::Mutex;

    let captured_models: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_handler = Arc::clone(&captured_models);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                let model = body
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string();
                captured.lock().unwrap().push(model);
                let events = sse::chat_completion_events("ok", "test-model");
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "model-A");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let _ = handle
        .submit_and_collect(RequestId::from("req-1"), user_request("hi"))
        .await
        .expect("first req ok");

    let mut new_cfg = test_config(server.base_url(), "model-B");
    new_cfg.api_key = Some("test-key".into());
    handle.update_config(new_cfg);

    let _ = handle
        .submit_and_collect(RequestId::from("req-2"), user_request("hi"))
        .await
        .expect("second req ok");

    server.shutdown();

    let models = captured_models.lock().unwrap();
    assert_eq!(
        models.as_slice(),
        &["model-A".to_string(), "model-B".to_string()]
    );
}

// ---------------------------------------------------------------------------
// Responses doom-loop check signals
// ---------------------------------------------------------------------------

fn responses_config(base_url: String, doom_loop: Option<DoomLoopRecoveryPolicy>) -> SamplerConfig {
    let mut cfg = test_config(base_url, "test-model");
    cfg.api_backend = ApiBackend::Responses;
    cfg.doom_loop_recovery = doom_loop;
    cfg
}

/// The Responses-surface twin of `null_choices_usage_chunk_completes_the_turn`:
/// `response.created` carries an empty output list, which a Go gateway writes
/// as `"output": null`, and `tools` arrives the same way when the request sent
/// none. Both land on the very first event of every turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_null_lists_on_created_complete_the_turn() {
    let app = Router::new().route(
        "/v1/responses",
        post(|| async {
            let mut events = sse::responses_api_script_exact("an answer", "test-model");
            let created: &mut SseEvent = &mut events[0];
            let mut value: serde_json::Value = serde_json::from_str(&created.data).unwrap();
            value["response"]["output"] = serde_json::Value::Null;
            value["response"]["tools"] = serde_json::Value::Null;
            created.data = value.to_string();
            Sse::new(stream::iter(
                sse_events_to_axum(events)
                    .into_iter()
                    .map(Ok::<_, std::convert::Infallible>),
            ))
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        responses_config(server.base_url(), None),
        RetryPolicy::default(),
        event_tx,
    );

    let result = handle
        .submit_and_collect(RequestId::from("req-null-output"), user_request("hi"))
        .await
        .expect("null lists on response.created must not fail the turn");
    server.shutdown();

    let (response, _metrics) = result;
    assert_eq!(response.assistant_text(), "an answer");
}

/// Server-reported doom-loop triggers flow through the actor rung onto the completed response, without retries.
/// The trigger is non-confident (`@response` channel), so the recovery, which resamples only confident signals, leaves it alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_doom_loop_signals_reach_completed_response() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let events = sse_events_to_axum(sse::responses_api_doom_loop_terminal_only_events(
                    &["tail_repetition:4@response"],
                    "some thought",
                    "an answer",
                    "test-model",
                ));
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        responses_config(server.base_url(), Some(DoomLoopRecoveryPolicy::default())),
        RetryPolicy::default(),
        event_tx,
    );

    let result = handle
        .submit_and_collect(RequestId::from("req-doom-signal"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("a signalled turn still completes");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "warn-only: no resample");
    assert_eq!(response.doom_loop_signals.len(), 1);
    assert_eq!(
        response.doom_loop_signals[0].raw,
        "tail_repetition:4@response"
    );
    assert_eq!(response.assistant_text(), "an answer");
}

/// Acceptance spec for the recovery rung: a confident tail signal is resampled once while its detector label remains observable.
/// The clean second response is accepted on its own budget even with transport retries disabled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_confident_doom_loop_signal_resamples_once() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let bodies_handler = Arc::clone(&bodies);
    let app = Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let counter = Arc::clone(&counter_handler);
            let bodies = Arc::clone(&bodies_handler);
            async move {
                bodies
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&body).unwrap());
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let events = if attempt == 0 {
                    sse::responses_api_doom_loop_terminal_only_events(
                        &["tail_repetition:8@thinking"],
                        "loop loop loop",
                        "poisoned answer",
                        "test-model",
                    )
                } else {
                    sse::responses_api_reasoning_and_text_events(
                        "fresh thought",
                        "clean answer",
                        "test-model",
                    )
                };
                let events = sse_events_to_axum(events);
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut config = responses_config(server.base_url(), Some(DoomLoopRecoveryPolicy::default()));
    config.max_retries = Some(0);
    let handle = SamplerActor::spawn(config, RetryPolicy::default(), event_tx);

    let collected = handle
        .submit_and_collect_with_metadata(RequestId::from("req-doom-resample"), user_request("hi"))
        .await;
    server.shutdown();

    assert!(collected.terminal_event_queued);
    assert_eq!(
        collected.doom_loop_signals,
        vec!["tail_repetition:8@thinking".to_string()],
    );
    assert_eq!(1, collected.doom_loop_recovery_attempts.len());
    assert_eq!(
        collected.doom_loop_recovery_attempts[0].triggers,
        vec!["tail_repetition:8@thinking".to_string()]
    );
    let (response, _metrics) = collected
        .result
        .expect("recovery accepts the clean resample");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactly one resample");
    assert_eq!(response.assistant_text(), "clean answer");
    assert!(
        response.doom_loop_signals.is_empty(),
        "the accepted response is the clean resample"
    );

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(1)).await;
    assert!(events.iter().any(|event| matches!(
        event,
        SamplingEvent::DoomLoopSignals { triggers, .. }
            if triggers == &["tail_repetition:8@thinking".to_string()]
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SamplingEvent::Retrying {
            doom_loop_triggers: Some(triggers),
            ..
        } if triggers == &["tail_repetition:8@thinking".to_string()]
    )));

    let bodies = bodies.lock().unwrap();
    let retry_input = bodies[1]["input"].as_array().unwrap();
    assert_eq!(retry_input.len(), 4);
    assert_eq!(retry_input[1]["summary"][0]["text"], "loop loop loop");
    assert_eq!(retry_input[2]["role"], "assistant");
    assert_eq!(retry_input[2]["content"], "poisoned answer");
    assert_eq!(retry_input[3]["role"], "user");
    let reminder = retry_input[3]["content"]
        .as_str()
        .expect("the reminder is a text item");
    assert!(
        reminder.starts_with("<system_reminder>") && reminder.ends_with("</system_reminder>"),
        "the retry closes with a synthetic system-reminder envelope: {reminder}"
    );
}

/// A caller that opted into `retry_only_before_output` cannot retract text it already received.
/// So a doomed turn that streamed output fails instead of resampling over the delivered prefix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_doom_loop_does_not_resample_after_output_when_retry_only_before_output() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let events = sse_events_to_axum(sse::responses_api_doom_loop_terminal_only_events(
                    &["tail_repetition:8@thinking"],
                    "loop loop loop",
                    "poisoned answer",
                    "test-model",
                ));
                Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let retry_policy = RetryPolicy {
        retry_only_before_output: true,
        ..RetryPolicy::default()
    };
    let handle = SamplerActor::spawn(
        responses_config(server.base_url(), Some(DoomLoopRecoveryPolicy::default())),
        retry_policy,
        event_tx,
    );

    let result = handle
        .submit_and_collect(RequestId::from("req-doom-no-retract"), user_request("hi"))
        .await;
    server.shutdown();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "no resample after output"
    );
    assert!(
        matches!(
            result,
            Err(xai_grok_sampling_types::SamplingError::DoomLoopDetected { .. })
        ),
        "the doomed turn is surfaced rather than resampled: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Output-rate floor
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// A collapsed stream gets a backup generation beside it. The original never
/// finishes and the backup answers at the same time, so the backup's answer wins.
async fn a_collapsed_stream_loses_to_a_backup_that_finishes_first() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    // 60 chunks at 200 ms outlasts the breach by far; the
                    // client drops the stream partway through.
                    let events: Vec<Event> =
                        (0..60).map(|_| text_chunk_event("x", false)).collect();
                    let slow = stream::iter(events).then(|event| async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        Ok::<_, std::convert::Infallible>(event)
                    });
                    return Sse::new(slow.boxed());
                }
                let events = vec![text_chunk_event("clean answer", true)];
                Sse::new(
                    stream::iter(events.into_iter().map(Ok::<_, std::convert::Infallible>)).boxed(),
                )
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(OutputRateFloorPolicy {
        min_tokens_per_sec: 100.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: 0,
    });
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-rate-floor"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the backup answers");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactly one backup");
    assert_eq!(response.assistant_text(), "clean answer");

    let mut saw_rate_retry = false;
    let mut saw_rate_event = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            SamplingEvent::Retrying { kind, .. } => {
                if kind == SamplingErrorKind::OutputRateCollapsed {
                    saw_rate_retry = true;
                }
            }
            SamplingEvent::OutputRate {
                tokens_per_sec,
                floor_tokens_per_sec,
                ..
            } => {
                saw_rate_event = true;
                assert!(
                    tokens_per_sec < 100.0,
                    "the collapsed stream's rate was {tokens_per_sec}"
                );
                assert_eq!(floor_tokens_per_sec, Some(100.0));
            }
            _ => {}
        }
    }
    assert!(
        saw_rate_retry,
        "the reissue must be attributed to the rate floor, not to a transport retry"
    );
    assert!(
        saw_rate_event,
        "the rate the gate measured must also reach the client"
    );
}

/// When the server dropped a response body, which a cancelled request causes.
struct DropStamp(Arc<std::sync::Mutex<Option<std::time::Instant>>>);

impl Drop for DropStamp {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = Some(std::time::Instant::now());
    }
}

fn floor_policy_100() -> OutputRateFloorPolicy {
    OutputRateFloorPolicy {
        min_tokens_per_sec: 100.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: 0,
    }
}

/// Every event the actor sends, with the instant it arrived.
fn collect_timed(
    mut event_rx: mpsc::UnboundedReceiver<SamplingEvent>,
) -> Arc<std::sync::Mutex<Vec<(std::time::Instant, SamplingEvent)>>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            sink.lock()
                .unwrap()
                .push((std::time::Instant::now(), event));
        }
    });
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// A slow original that speeds back up keeps its answer and cancels its backup.
async fn a_recovered_stream_cancels_its_backup_and_keeps_its_answer() {
    let counter = Arc::new(AtomicU32::new(0));
    let backup_dropped = Arc::new(std::sync::Mutex::new(None));
    let counter_handler = Arc::clone(&counter);
    let dropped_handler = Arc::clone(&backup_dropped);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            let dropped = Arc::clone(&dropped_handler);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    let fast = "f".repeat(40);
                    let mut events: Vec<(u64, Event)> = (0..20)
                        .map(|_| (200, text_chunk_event("x", false)))
                        .collect();
                    events.extend((0..120).map(|_| (25, text_chunk_event(&fast, false))));
                    events.push((25, text_chunk_event("", true)));
                    let paced = stream::iter(events).then(|(delay, event)| async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        Ok::<_, std::convert::Infallible>(event)
                    });
                    return Sse::new(paced.boxed());
                }
                let stamp = DropStamp(dropped);
                let mut events: Vec<Event> =
                    (0..200).map(|_| text_chunk_event("", false)).collect();
                events.push(text_chunk_event("late", true));
                let idle = stream::iter(events).then(move |event| {
                    let _ = &stamp;
                    async move {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok::<_, std::convert::Infallible>(event)
                    }
                });
                Sse::new(idle.boxed())
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(floor_policy_100());
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);
    let events = collect_timed(event_rx);

    let result = handle
        .submit_and_collect(RequestId::from("req-rate-recover"), user_request("hi"))
        .await;
    let finished_at = std::time::Instant::now();
    server.shutdown();

    let (response, _metrics) = result.expect("the recovered original answers");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "one backup was started");
    assert_eq!(
        response.assistant_text(),
        format!("{}{}", "x".repeat(20), "f".repeat(40 * 120)),
        "the original's answer, never the backup's"
    );
    let dropped_at = backup_dropped
        .lock()
        .unwrap()
        .expect("the backup's request was cancelled");
    assert!(
        dropped_at + Duration::from_secs(1) < finished_at,
        "the recovery cancels the backup, not the original's end: dropped {:?} before the end",
        finished_at.saturating_duration_since(dropped_at)
    );
    let retried = events.lock().unwrap().iter().any(|(_, e)| {
		matches!(e, SamplingEvent::Retrying { kind, .. } if *kind == SamplingErrorKind::OutputRateCollapsed)
	});
    assert!(!retried, "the caller never switched streams");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
/// A backup that gets ahead replaces the slow original while it still streams.
async fn a_backup_that_overtakes_replaces_the_slow_stream_mid_response() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let (text, count, delay, finish) = if attempt == 0 {
                    ("x", 60, 200, false)
                } else {
                    ("yy", 50, 50, true)
                };
                let mut events: Vec<Event> =
                    (0..count).map(|_| text_chunk_event(text, false)).collect();
                if finish {
                    events.push(text_chunk_event("", true));
                }
                let paced = stream::iter(events).then(move |event| async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    Ok::<_, std::convert::Infallible>(event)
                });
                Sse::new(paced.boxed())
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(floor_policy_100());
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);
    let events = collect_timed(event_rx);

    let result = handle
        .submit_and_collect(RequestId::from("req-rate-overtake"), user_request("hi"))
        .await;
    let finished_at = std::time::Instant::now();
    server.shutdown();

    let (response, _metrics) = result.expect("the backup answers");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "one backup, no reissue");
    assert_eq!(response.assistant_text(), "y".repeat(100));

    let events = events.lock().unwrap();
    let switch = events
		.iter()
		.position(|(_, e)| {
			matches!(e, SamplingEvent::Retrying { kind, .. } if *kind == SamplingErrorKind::OutputRateCollapsed)
		})
		.expect("the switch is reported as a rate-floor retry");
    let switched_at = events[switch].0;
    assert!(
        switched_at + Duration::from_secs(1) < finished_at,
        "the backup took over while it was still streaming, {:?} before its end",
        finished_at.saturating_duration_since(switched_at)
    );
    let after: Vec<&SamplingEvent> = events[switch + 1..].iter().map(|(_, e)| e).collect();
    assert!(
        matches!(after.first(), Some(SamplingEvent::StreamStarted { .. })),
        "the backup's stream is replayed from its start: {:?}",
        after.first()
    );
    let slow_text_after_switch = after
        .iter()
        .any(|e| matches!(e, SamplingEvent::ChannelToken { text, .. } if text.contains('x')));
    assert!(
        !slow_text_after_switch,
        "nothing from the slow stream reaches the caller after the switch"
    );
}

/// nothing was asked for, so nothing is reissued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ungated_session_never_reissues_a_slow_stream() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut events: Vec<Event> =
                    (0..12).map(|_| text_chunk_event("x", false)).collect();
                events.push(text_chunk_event("", true));
                let slow = stream::iter(events).then(|event| async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Ok::<_, std::convert::Infallible>(event)
                });
                Sse::new(slow.boxed())
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url(), "test-model"),
        RetryPolicy::default(),
        event_tx,
    );

    let result = handle
        .submit_and_collect(RequestId::from("req-no-floor"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("a slow stream still answers");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "no reissue");
    assert_eq!(response.assistant_text(), "xxxxxxxxxxxx");
}

/// A server-side web search delivers nothing while it runs. The model is not
/// generating during it, so the gap is the server's time and not a collapsed
/// stream: a response that searches for longer than the sustained duration is
/// answered, not reissued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hosted_search_gap_is_not_a_collapsed_stream() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                // One 39-byte word plus its space is 40 bytes, and 40 bytes
                // every 100 ms is 100 tok/s: five times the floor below, for
                // two seconds before the search.
                let word = "0".repeat(39);
                let fast = vec![word; 20].join(" ");
                let mut script = sse::responses_api_script_exact(&fast, "test-model");
                // `response.created` first, then the healthy burst, then the
                // search, then `response.completed` last.
                let created = script.remove(0);
                let completed = script.pop().expect("the terminal event");
                let mut events = vec![Delayed::now(created)];
                for event in script {
                    events.push(Delayed::after(100, event));
                }
                events.push(Delayed::now(SseEvent::data(
                    json!({
                        "type": "response.web_search_call.in_progress",
                        "sequence_number": 900,
                        "output_index": 0,
                        "item_id": "ws_1"
                    })
                    .to_string(),
                )));
                // The search runs for three seconds: longer than the window
                // and the sustained duration together.
                events.push(Delayed::after(
                    3000,
                    SseEvent::data(
                        json!({
                            "type": "response.output_item.done",
                            "sequence_number": 901,
                            "output_index": 0,
                            "item": {
                                "type": "web_search_call",
                                "id": "ws_1",
                                "status": "completed",
                                "action": { "type": "search", "query": "q", "sources": [] }
                            }
                        })
                        .to_string(),
                    ),
                ));
                events.push(Delayed::after(100, completed));
                Sse::new(delayed_stream(events))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let mut cfg = responses_config(server.base_url(), None);
    cfg.output_rate_floor = Some(OutputRateFloorPolicy {
        min_tokens_per_sec: 20.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: 0,
    });
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-hosted-search"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the searching response answers");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the search must not be read as a collapse and reissued"
    );
    assert!(!response.assistant_text().is_empty());
}

/// `stream_tool_calls` is off by default, so the upstream writes a whole tool
/// call before it says anything about it. The client sees the call open and
/// then nothing at all while the model generates its arguments. That span is
/// generation nobody streamed, not a stalled engine, so the response is
/// answered rather than reissued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unstreamed_tool_call_is_not_a_collapsed_stream() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let word = "0".repeat(39);
                let fast = vec![word; 20].join(" ");
                let mut script = sse::responses_api_script_exact(&fast, "test-model");
                let created = script.remove(0);
                script.pop();
                let mut events = vec![Delayed::now(created)];
                for event in script {
                    events.push(Delayed::after(100, event));
                }
                let call = json!({
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"a\"}",
                    "status": "completed"
                });
                events.push(Delayed::now(SseEvent::data(
                    json!({
                        "type": "response.output_item.added",
                        "sequence_number": 900,
                        "output_index": 1,
                        "item": {
                            "type": "function_call",
                            "id": "fc_1",
                            "call_id": "call_1",
                            "name": "read_file",
                            "arguments": "",
                            "status": "in_progress"
                        }
                    })
                    .to_string(),
                )));
                // Three seconds of writing the call upstream, which outlasts
                // the window and the sustained duration together.
                events.push(Delayed::after(
                    3000,
                    SseEvent::data(
                        json!({
                            "type": "response.completed",
                            "sequence_number": 901,
                            "response": {
                                "id": "resp_test",
                                "object": "response",
                                "created_at": 1234567890,
                                "model": "test-model",
                                "status": "completed",
                                "output": [call],
                                "usage": {
                                    "input_tokens": 10,
                                    "output_tokens": 5,
                                    "total_tokens": 15,
                                    "input_tokens_details": { "cached_tokens": 0 },
                                    "output_tokens_details": { "reasoning_tokens": 0 }
                                }
                            }
                        })
                        .to_string(),
                    ),
                ));
                Sse::new(delayed_stream(events))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let mut cfg = responses_config(server.base_url(), None);
    cfg.output_rate_floor = Some(OutputRateFloorPolicy {
        min_tokens_per_sec: 20.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: 0,
    });
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-unstreamed-call"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the tool-calling response answers");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "writing a tool call must not be read as a collapse and reissued"
    );
    assert_eq!(response.tool_calls().len(), 1);
}

// ---------------------------------------------------------------------------
// Time-to-first-token limit
// ---------------------------------------------------------------------------

/// A policy with only the time-to-first-token limit armed.
fn ttft_only_policy(limit_secs: u64) -> OutputRateFloorPolicy {
    OutputRateFloorPolicy {
        min_tokens_per_sec: 0.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: limit_secs,
    }
}

/// Whether any `Retrying` event on the channel names a first-token timeout.
fn saw_ttft_retry(event_rx: &mut mpsc::UnboundedReceiver<SamplingEvent>) -> bool {
    let mut seen = false;
    while let Ok(event) = event_rx.try_recv() {
        if let SamplingEvent::Retrying { kind, .. } = event
            && kind == SamplingErrorKind::FirstTokenTimeout
        {
            seen = true;
        }
    }
    seen
}

/// The first attempt sends its headers and then nothing for three seconds,
/// against a one-second limit. It is abandoned and the reissue answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_silent_stream_is_reissued_after_the_ttft_limit() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let text = if attempt == 0 { "late" } else { "clean answer" };
                let delay = if attempt == 0 { 3000 } else { 0 };
                let events = vec![text_chunk_event(text, true)];
                let delayed = stream::iter(events).then(move |event| async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    Ok::<_, std::convert::Infallible>(event)
                });
                Sse::new(delayed.boxed())
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(ttft_only_policy(1));
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-ttft-body"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the reissued request answers");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactly one reissue");
    assert_eq!(response.assistant_text(), "clean answer");
    assert!(
        saw_ttft_retry(&mut event_rx),
        "the reissue must be attributed to the first-token limit"
    );
}

/// The limit also covers the wait for response headers: a server that holds
/// the whole response back is abandoned the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn withheld_headers_are_reissued_after_the_ttft_limit() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
                let events = vec![text_chunk_event("clean answer", true)];
                Sse::new(
                    stream::iter(events.into_iter().map(Ok::<_, std::convert::Infallible>)).boxed(),
                )
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(ttft_only_policy(1));
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-ttft-headers"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the reissued request answers");
    assert_eq!(counter.load(Ordering::SeqCst), 2, "exactly one reissue");
    assert_eq!(response.assistant_text(), "clean answer");
    assert!(saw_ttft_retry(&mut event_rx));
}

/// Output inside the limit ends the check: a stream whose first chunk lands
/// early and whose whole body runs well past the limit is never reissued.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_output_is_never_reissued_by_the_ttft_limit() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_handler = Arc::clone(&counter);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter_handler);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut events: Vec<Event> = (0..6).map(|_| text_chunk_event("x", false)).collect();
                events.push(text_chunk_event("", true));
                let paced = stream::iter(events).then(|event| async move {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    Ok::<_, std::convert::Infallible>(event)
                });
                Sse::new(paced.boxed())
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.base_url(), "test-model");
    cfg.output_rate_floor = Some(ttft_only_policy(1));
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let result = handle
        .submit_and_collect(RequestId::from("req-ttft-early"), user_request("hi"))
        .await;
    server.shutdown();

    let (response, _metrics) = result.expect("the stream answers");
    assert_eq!(counter.load(Ordering::SeqCst), 1, "no reissue");
    assert_eq!(response.assistant_text(), "xxxxxx");
    assert!(!saw_ttft_retry(&mut event_rx));
}

/// One scripted SSE event and how long the mock server waits before it.
struct Delayed {
    delay_ms: u64,
    event: SseEvent,
}

impl Delayed {
    fn now(event: SseEvent) -> Self {
        Self { delay_ms: 0, event }
    }

    fn after(delay_ms: u64, event: SseEvent) -> Self {
        Self { delay_ms, event }
    }
}

/// Serve `events`, waiting each one's delay before it goes out.
fn delayed_stream(
    events: Vec<Delayed>,
) -> futures_util::stream::BoxStream<'static, Result<Event, std::convert::Infallible>> {
    stream::iter(events)
        .then(|event| async move {
            tokio::time::sleep(Duration::from_millis(event.delay_ms)).await;
            Ok(sse_events_to_axum(vec![event.event]).remove(0))
        })
        .boxed()
}

// ---------------------------------------------------------------------------
// Helpers for draining the event channel
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Strict-schema message-property recovery (Cerebras `wrong_api_format`)
// ---------------------------------------------------------------------------
//
// The provider's schema rejects any message property it does not define, and
// the offending properties (`model_id`, `reasoning_content`) live in stored
// conversation history. Without the strip-and-retry arm this 400 is Fatal and
// the conversation is bricked from turn 2 onward. These tests drive the real
// `SamplerActor` retry loop and assert on the bodies the server actually
// received.

/// History whose assistant item carries a recorded `model_id` plus a replayed
/// reasoning sibling — the shape that produced the live Cerebras 400.
fn poisoned_request(text: &str) -> ConversationRequest {
    ConversationRequest {
        items: vec![
            ConversationItem::User(UserItem {
                content: vec![xai_grok_sampling_types::ContentPart::Text {
                    text: std::sync::Arc::<str>::from(text),
                }],
                synthetic_reason: SyntheticReason::Human,
                ..Default::default()
            }),
            ConversationItem::Reasoning(xai_grok_sampling_types::synthesized_reasoning_item(
                "thinking about q1",
            )),
            ConversationItem::Assistant(xai_grok_sampling_types::conversation::AssistantItem {
                content: "a1".into(),
                tool_calls: vec![],
                model_id: Some("qwen-3.8-27b".into()),
                model_fingerprint: None,
                reasoning_effort: None,
            }),
            ConversationItem::User(UserItem {
                content: vec![xai_grok_sampling_types::ContentPart::Text {
                    text: std::sync::Arc::<str>::from("q2"),
                }],
                synthetic_reason: SyntheticReason::Human,
                ..Default::default()
            }),
        ],
        ..Default::default()
    }
}

/// The documented Cerebras rejection of replayed history.
fn cerebras_400_body() -> serde_json::Value {
    json!({
        "message": "wrong_api_format: messages.2.assistant.model_id: property \
            'messages.2.assistant.model_id' is unsupported\n\
            messages.2.assistant.reasoning_content: property \
            'messages.2.assistant.reasoning_content' is unsupported",
        "type": "invalid_request_error",
        "param": "validation_error",
        "code": "wrong_api_format",
    })
}

/// History predating the fix must recover, not dead-end: the first attempt is
/// answered with the Cerebras 400, and the retried body must omit exactly the
/// properties the provider named.
///
/// Asserts on the recorded request bodies — what the provider actually
/// received — using the shared mock server's `request_bodies()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_message_property_400_strips_and_recovers() {
    let server = xai_grok_test_support::MockInferenceServer::start()
        .await
        .unwrap();
    server.enqueue_response(
        "/v1/chat/completions",
        xai_grok_test_support::ScriptedResponse::json(400, cerebras_400_body()),
    );
    server.set_keep_requests(true);
    server.set_response("recovered");

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.url(), "qwen-3.8-27b");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let rid = RequestId::from("req-strict-schema");
    handle.submit(rid.clone(), poisoned_request("q1"));

    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(30)).await;

    // The turn completed rather than dead-ending.
    let response = match events.last().unwrap() {
        SamplingEvent::Completed { response, .. } => response.clone(),
        other => panic!("expected Completed after recovery, got {other:?}"),
    };
    assert!(
        response
            .assistant()
            .is_some_and(|a| a.content.contains("recovered")),
        "the recovered turn must carry the provider's reply: {response:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Retrying { .. })),
        "the recovery is a retry and must be observable as one"
    );

    // Exactly two requests: the rejected attempt, then the recovered retry.
    let bodies = server.request_bodies();
    assert_eq!(
        bodies.len(),
        2,
        "expected the rejected attempt plus one recovered retry, got {bodies:#?}"
    );

    // The rejected attempt carried the properties (that is what caused the 400).
    let first = bodies[0]["messages"].as_array().expect("messages array");
    let first_assistant = first
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant in the first attempt");
    assert!(
        first_assistant.get("model_id").is_some(),
        "the first attempt should carry model_id — the rejected shape: {first:#?}"
    );

    // The recovered retry carries neither property on any message.
    let retried = bodies[1]["messages"].as_array().expect("messages array");
    for m in retried {
        assert!(
            m.get("model_id").is_none(),
            "recovered body must omit model_id: {m:#}"
        );
        assert!(
            m.get("reasoning_content").is_none(),
            "recovered body must omit reasoning_content: {m:#}"
        );
    }

    // Recovery must not cost content: the assistant reply and the follow-up
    // user turn survive.
    let retried_assistant = retried
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant in the recovered attempt");
    assert_eq!(retried_assistant["content"], json!("a1"));
    assert!(
        retried.iter().any(|m| m["content"] == json!("q2")),
        "the conversation past the poisoned turn must survive: {retried:#?}"
    );
}

/// A model configured `strict_message_schema` sends a body the provider
/// accepts on the first attempt — no 400 and no retry. This is the primary
/// fix for new sessions; the recovery above covers pre-existing history.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strict_model_config_sends_no_unsupported_property_at_all() {
    let server = xai_grok_test_support::MockInferenceServer::start()
        .await
        .unwrap();
    server.set_keep_requests(true);
    server.set_response("ok");

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut cfg = test_config(server.url(), "qwen-3.8-27b");
    // Exactly what `sampling_config_for_model` produces for a model entry with
    // `strict_message_schema = true`.
    cfg.chat_message_profile = xai_grok_sampling_types::ChatMessageProfile::STRICT;
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    handle.submit(RequestId::from("req-strict-config"), poisoned_request("q1"));
    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(30)).await;

    assert!(
        matches!(events.last().unwrap(), SamplingEvent::Completed { .. }),
        "a strict-configured model must complete: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SamplingEvent::Retrying { .. })),
        "no retry should be needed: {events:?}"
    );

    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 1, "exactly one request: {bodies:#?}");
    for m in bodies[0]["messages"].as_array().unwrap() {
        assert!(
            m.get("model_id").is_none() && m.get("reasoning_content").is_none(),
            "a strict model must never send these properties: {m:#}"
        );
    }
}

/// The regression guard at the wire level: a permissive model (the default)
/// still sends both properties and needs no retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn permissive_model_still_sends_replayed_properties() {
    let server = xai_grok_test_support::MockInferenceServer::start()
        .await
        .unwrap();
    server.set_keep_requests(true);
    server.set_response("ok");

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.url(), "qwen-3.8-27b");
    assert_eq!(
        cfg.chat_message_profile,
        xai_grok_sampling_types::ChatMessageProfile::PERMISSIVE,
        "the default config must stay permissive"
    );
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    handle.submit(RequestId::from("req-permissive"), poisoned_request("q1"));
    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(30)).await;
    assert!(matches!(
        events.last().unwrap(),
        SamplingEvent::Completed { .. }
    ));

    let bodies = server.request_bodies();
    assert_eq!(
        bodies.len(),
        1,
        "no retry for a tolerant target: {bodies:#?}"
    );
    let messages = bodies[0]["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant present");
    assert_eq!(
        assistant["model_id"],
        json!("qwen-3.8-27b"),
        "tolerant target keeps model_id: {assistant:#}"
    );
    assert_eq!(
        assistant["reasoning_content"],
        json!("thinking about q1"),
        "tolerant target keeps replayed reasoning: {assistant:#}"
    );
}

/// A 400 that is not the strict-schema class must stay fatal — the recovery
/// must not silently rewrite bodies for unrelated request bugs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_400_stays_fatal_without_a_strip_retry() {
    let server = xai_grok_test_support::MockInferenceServer::start()
        .await
        .unwrap();
    server.set_keep_requests(true);
    server.enqueue_response(
        "/v1/chat/completions",
        xai_grok_test_support::ScriptedResponse::json(
            400,
            json!({
                "message": "malformed tool call in history",
                "type": "invalid_request_error",
                "code": "invalid_request",
            }),
        ),
    );

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.url(), "qwen-3.8-27b");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    handle.submit(RequestId::from("req-unrelated-400"), poisoned_request("q1"));
    let events = drain_until_terminal(&mut event_rx, Duration::from_secs(30)).await;

    assert!(
        matches!(events.last().unwrap(), SamplingEvent::Failed { .. }),
        "an unrelated 400 must end the turn as failed: {events:?}"
    );
    assert_eq!(
        server.request_bodies().len(),
        1,
        "no strip-retry for an unrelated 400: {:#?}",
        server.request_bodies()
    );
}

/// Drain the event channel until a terminal event (`Completed` or `Failed`) is received, or until `deadline` elapses.
async fn drain_until_terminal(
    rx: &mut mpsc::UnboundedReceiver<SamplingEvent>,
    timeout: Duration,
) -> Vec<SamplingEvent> {
    let mut out = Vec::new();
    let start = tokio::time::Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            panic!(
                "drain_until_terminal timed out after {:?}; got {} events",
                timeout,
                out.len()
            );
        }
        let remaining = timeout - elapsed;
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(ev)) => {
                let terminal = matches!(
                    ev,
                    SamplingEvent::Completed { .. } | SamplingEvent::Failed { .. }
                );
                out.push(ev);
                if terminal {
                    return out;
                }
            }
            Ok(None) => panic!("event channel closed before terminal event"),
            Err(_) => panic!(
                "drain_until_terminal timed out after {:?}; got {} events",
                timeout,
                out.len()
            ),
        }
    }
}

/// Wait for the next event matching `pred`, or return `None` on timeout.
async fn await_event_matching(
    rx: &mut mpsc::UnboundedReceiver<SamplingEvent>,
    mut pred: impl FnMut(&SamplingEvent) -> bool,
    timeout: Duration,
) -> Option<SamplingEvent> {
    let start = tokio::time::Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return None;
        }
        let remaining = timeout - elapsed;
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(ev)) => {
                if pred(&ev) {
                    return Some(ev);
                }
            }
            Ok(None) => return None,
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Model that takes no image input
// ---------------------------------------------------------------------------

fn user_request_with_image(text: &str) -> ConversationRequest {
    use xai_grok_sampling_types::ContentPart;
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![
                ContentPart::Text {
                    text: std::sync::Arc::<str>::from(text),
                },
                ContentPart::Image {
                    url: std::sync::Arc::<str>::from("data:image/png;base64,AAAA"),
                },
            ],
            synthetic_reason: SyntheticReason::Human,
            ..Default::default()
        })],
        ..Default::default()
    }
}

/// The reported trap, end to end: a vision-less model answers an image with a
/// 404, which is otherwise fatal. The images live in conversation history, so
/// a fatal there bricks every following turn — including `/goal resume` — with
/// no way out but a new session.
///
/// Proves both halves of the recovery: this request completes after a strip,
/// and the *next* request never ships the image at all, so a session that
/// pasted a screenshot does not pay a rejected upload on every turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn image_input_rejection_strips_and_then_stops_resending() {
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let bodies_handler = Arc::clone(&bodies);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let bodies = Arc::clone(&bodies_handler);
            async move {
                let saw_image = body.contains("image_url");
                bodies.lock().unwrap().push(body);
                if saw_image {
                    return Err::<Sse<_>, (StatusCode, String)>((
                        StatusCode::NOT_FOUND,
                        json!({
                            "error": { "message": "No endpoints found that support image input" }
                        })
                        .to_string(),
                    ));
                }
                let events = sse::chat_completion_events("ok", "test-model");
                Ok(Sse::new(stream::iter(
                    events.into_iter().map(Ok::<_, std::convert::Infallible>),
                )))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let cfg = test_config(server.base_url(), "no-vision-model");
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);

    let (first, _) = handle
        .submit_and_collect(
            RequestId::from("req-image-1"),
            user_request_with_image("what is in this screenshot"),
        )
        .await
        .expect("the rejection must recover by stripping, not fail the turn");
    assert_eq!(
        first.assistant().map(|a| a.content.to_string()).as_deref(),
        Some("ok")
    );

    let (second, _) = handle
        .submit_and_collect(
            RequestId::from("req-image-2"),
            user_request_with_image("and now"),
        )
        .await
        .expect("second turn must not fail either");
    assert_eq!(
        second.assistant().map(|a| a.content.to_string()).as_deref(),
        Some("ok")
    );

    server.shutdown();

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(
        bodies.iter().filter(|b| b.contains("image_url")).count(),
        1,
        "the image is sent once; after the model rejects it, later turns strip it up front"
    );
    assert_eq!(
        bodies.len(),
        3,
        "one rejected attempt, its strip retry, then one clean turn"
    );
    assert!(
        bodies[1].contains("cannot read images"),
        "the placeholder must tell the model why the image is gone: {}",
        bodies[1]
    );
}
