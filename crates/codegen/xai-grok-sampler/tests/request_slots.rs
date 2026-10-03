//! The process-wide request cap, driven through the real actor and client against a mock server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, oneshot};

use xai_grok_sampler::{
    RequestId, RetryPolicy, SamplerActor, SamplerConfig, SamplingClient, SamplingErrorKind,
    SamplingEvent, set_max_parallel_requests, timeout_excluding_queue,
};
use xai_grok_sampling_types::{
    ConversationItem, ConversationRequest, OutputRateFloorPolicy, SyntheticReason, UserItem,
};

/// Every test sets the global cap, so the tests take turns.
static CAP: Mutex<()> = Mutex::const_new(());

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
        Self { addr, shutdown_tx }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

fn test_config(base_url: String) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url,
        model: "test-model".into(),
        max_completion_tokens: Some(1024),
        context_window: 128_000,
        max_retries: Some(2),
        idle_timeout_secs: Some(30),
        ..Default::default()
    }
}

fn user_request(text: &str) -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![xai_grok_sampling_types::ContentPart::Text {
                text: Arc::<str>::from(text),
            }],
            synthetic_reason: SyntheticReason::Human,
            ..Default::default()
        })],
        ..Default::default()
    }
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

/// A server whose every response takes `hold` before its only chunk. It
/// counts the requests it served and the most it held open at once.
struct Counting {
    served: Arc<AtomicUsize>,
    live: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Counting {
    fn router(&self, hold: Duration) -> Router {
        let served = Arc::clone(&self.served);
        let live = Arc::clone(&self.live);
        let peak = Arc::clone(&self.peak);
        Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let served = Arc::clone(&served);
                let live = Arc::clone(&live);
                let peak = Arc::clone(&peak);
                async move {
                    served.fetch_add(1, Ordering::SeqCst);
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let body = stream::once(async move {
                        tokio::time::sleep(hold).await;
                        live.fetch_sub(1, Ordering::SeqCst);
                        Ok::<_, std::convert::Infallible>(text_chunk_event("answer", true))
                    });
                    Sse::new(body.boxed())
                }
            }),
        )
    }

    fn new() -> Self {
        Self {
            served: Arc::new(AtomicUsize::new(0)),
            live: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_never_sees_more_requests_than_the_cap() {
    let _cap = CAP.lock().await;
    set_max_parallel_requests(3);
    let counting = Counting::new();
    let server = MockServer::spawn(counting.router(Duration::from_millis(300))).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url()),
        RetryPolicy::default(),
        event_tx,
    );

    let results = futures_util::future::join_all((0..10).map(|n| {
        let handle = handle.clone();
        async move {
            handle
                .submit_and_collect(RequestId::from(format!("req-cap-{n}")), user_request("hi"))
                .await
        }
    }))
    .await;
    server.shutdown();

    for result in results {
        result.expect("every queued request still answers");
    }
    assert_eq!(counting.served.load(Ordering::SeqCst), 10);
    assert_eq!(counting.peak.load(Ordering::SeqCst), 3);
}

/// A request queued behind a slow one must not be reissued by its first-token limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queue_time_does_not_count_toward_the_first_token_limit() {
    let _cap = CAP.lock().await;
    set_max_parallel_requests(1);
    let counting = Counting::new();
    let slow = MockServer::spawn(counting.router(Duration::from_secs(3))).await;
    let fast_served = Counting::new();
    let fast = MockServer::spawn(fast_served.router(Duration::ZERO)).await;

    let (slow_tx, _slow_rx) = mpsc::unbounded_channel();
    let slow_handle = SamplerActor::spawn(
        test_config(slow.base_url()),
        RetryPolicy::default(),
        slow_tx,
    );
    let (fast_tx, mut fast_rx) = mpsc::unbounded_channel();
    let mut fast_cfg = test_config(fast.base_url());
    fast_cfg.output_rate_floor = Some(OutputRateFloorPolicy {
        min_tokens_per_sec: 0.0,
        window_secs: 2,
        sustained_secs: 1,
        max_retries: 2,
        ttft_timeout_secs: 1,
    });
    let fast_handle = SamplerActor::spawn(fast_cfg, RetryPolicy::default(), fast_tx);

    let slow_call = tokio::spawn(async move {
        slow_handle
            .submit_and_collect(RequestId::from("req-slow"), user_request("hi"))
            .await
    });
    while counting.served.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let queued = fast_handle
        .submit_and_collect(RequestId::from("req-queued"), user_request("hi"))
        .await;
    slow_call.await.unwrap().expect("the slow request answers");
    slow.shutdown();
    fast.shutdown();

    queued.expect("the queued request answers");
    assert_eq!(
        fast_served.served.load(Ordering::SeqCst),
        1,
        "a queued request must not be reissued for its time in the queue"
    );
    let mut queue_events = Vec::new();
    while let Ok(event) = fast_rx.try_recv() {
        match event {
            SamplingEvent::Retrying { kind, .. } => {
                assert_ne!(kind, SamplingErrorKind::FirstTokenTimeout);
            }
            SamplingEvent::Queued { ahead, limit, .. } => {
                queue_events.push(format!("queued ahead={ahead} limit={limit}"));
            }
            SamplingEvent::Dequeued { waited_ms, .. } => {
                assert!(waited_ms > 0, "the request waited behind the slow one");
                queue_events.push("dequeued".to_string());
            }
            _ => {}
        }
    }
    assert_eq!(queue_events, ["queued ahead=0 limit=1", "dequeued"]);
}

/// A deadline around `submit_and_collect` excludes the queue, though the request runs on the actor's task.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outer_deadline_excludes_the_queue_across_the_actor() {
    let _cap = CAP.lock().await;
    set_max_parallel_requests(1);
    let counting = Counting::new();
    let server = MockServer::spawn(counting.router(Duration::from_secs(2))).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        test_config(server.base_url()),
        RetryPolicy::default(),
        event_tx,
    );

    let first = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .submit_and_collect(RequestId::from("req-first"), user_request("hi"))
                .await
        })
    };
    while counting.served.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = timeout_excluding_queue(
        Duration::from_secs(3),
        handle.submit_and_collect(RequestId::from("req-second"), user_request("hi")),
    )
    .await;
    first.await.unwrap().expect("the first request answers");
    server.shutdown();

    second
        .expect("the queue must not count toward the deadline")
        .expect("the second request answers");
}

/// The same deadline over a direct client call, which takes its slot inside the client rather than in the actor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_direct_client_call_queues_and_excludes_the_queue() {
    let _cap = CAP.lock().await;
    set_max_parallel_requests(1);
    let counting = Counting::new();
    let server = MockServer::spawn(counting.router(Duration::from_secs(2))).await;
    let client = SamplingClient::new(test_config(server.base_url())).unwrap();

    let first = {
        let client = client.clone();
        tokio::spawn(async move { client.conversation_collect(user_request("hi")).await })
    };
    while counting.served.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = timeout_excluding_queue(
        Duration::from_secs(3),
        client.conversation_collect(user_request("hi")),
    )
    .await;
    first.await.unwrap().expect("the first call answers");
    server.shutdown();

    second
        .expect("the queue must not count toward the deadline")
        .expect("the second call answers");
    assert_eq!(counting.peak.load(Ordering::SeqCst), 1);
}
