//! Every request a client sends reports its usage to the config's sink once, through the real client and actor.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use futures_util::stream;
use parking_lot::Mutex;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use xai_grok_sampler::{
    ApiBackend, BilledCall, RequestId, RetryPolicy, SamplerActor, SamplerConfig, SamplingClient,
    SharedUsageSink, UsageSink,
};
use xai_grok_sampling_types::{ConversationItem, ConversationRequest};

#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<BilledCall>>,
}

impl UsageSink for Recorder {
    fn record_call(&self, call: BilledCall) {
        self.calls.lock().push(call);
    }
}

impl Recorder {
    fn calls(&self) -> Vec<BilledCall> {
        self.calls.lock().clone()
    }
}

struct Server {
    base_url: String,
    shutdown: oneshot::Sender<()>,
}

async fn serve(app: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, rx) = oneshot::channel::<()>();
    xai_spawn::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    Server {
        base_url: format!("http://{addr}/v1"),
        shutdown,
    }
}

fn config(base_url: &str, backend: ApiBackend, sink: SharedUsageSink) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url: base_url.to_owned(),
        model: "asked-model".into(),
        api_backend: backend,
        context_window: 128_000,
        max_retries: Some(2),
        idle_timeout_secs: Some(30),
        usage_sink: Some(sink),
        ..Default::default()
    }
}

fn recorder() -> (Arc<Recorder>, SharedUsageSink) {
    let recorder = Arc::new(Recorder::default());
    let sink: SharedUsageSink = recorder.clone();
    (recorder, sink)
}

fn hi() -> ConversationRequest {
    ConversationRequest::from_items(vec![ConversationItem::user("hi")])
}

fn chat_chunk(content: Option<&str>, usage: Option<serde_json::Value>) -> Event {
    let mut chunk = json!({
        "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "served-model",
        "choices": content.map_or_else(Vec::new, |text| vec![json!({
            "index": 0,
            "delta": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        })]),
    });
    if let Some(usage) = usage {
        chunk["usage"] = usage;
    }
    Event::default().data(chunk.to_string())
}

fn usage(prompt: u32, completion: u32, ticks: i64) -> serde_json::Value {
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "cost_in_usd_ticks": ticks,
    })
}

/// A Chat Completions server. A streaming request gets a text chunk and a usage chunk; a plain one gets JSON.
/// The first `fail_first` streaming requests send their usage and then a retryable stream error.
fn chat_server(served: Arc<AtomicUsize>, fail_first: usize) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        post(move |body: Bytes| {
            let served = Arc::clone(&served);
            async move {
                let n = served.fetch_add(1, Ordering::SeqCst);
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if request["stream"] != json!(true) {
                    return axum::Json(json!({
                        "id": "c", "object": "chat.completion", "created": 0, "model": "served-model",
                        "choices": [{
                            "index": 0,
                            "message": { "role": "assistant", "content": "plain" },
                            "finish_reason": "stop"
                        }],
                        "usage": usage(30, 3, 11),
                    }))
                    .into_response();
                }
                let events: Vec<Event> = if n < fail_first {
                    vec![
                        chat_chunk(None, Some(usage(20, 0, 5))),
                        Event::default().data(
                            json!({"error": {"type": "overloaded_error", "message": "Overloaded"}})
                                .to_string(),
                        ),
                    ]
                } else {
                    vec![
                        chat_chunk(Some("answer"), None),
                        chat_chunk(None, Some(usage(10, 2, 7))),
                    ]
                };
                Sse::new(stream::iter(
                    events
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
                .into_response()
            }
        }),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_side_call_collect_reports_one_call() {
    let (recorder, sink) = recorder();
    let served = Arc::new(AtomicUsize::new(0));
    let server = serve(chat_server(Arc::clone(&served), 0)).await;
    let client = SamplingClient::new(config(&server.base_url, ApiBackend::ChatCompletions, sink))
        .expect("client");

    let response = client.conversation_collect(hi()).await.expect("collect");
    assert_eq!(response.assistant_text(), "answer");

    let calls = recorder.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = calls.first().expect("one call");
    assert_eq!(call.model, "served-model");
    assert_eq!(call.cost_usd_ticks, Some(7));
    assert_eq!(call.usage.as_ref().map(|u| u.prompt_tokens), Some(10));
    let _ = server.shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_streaming_call_reports_its_usage() {
    let (recorder, sink) = recorder();
    let served = Arc::new(AtomicUsize::new(0));
    let server = serve(chat_server(Arc::clone(&served), 0)).await;
    let client = SamplingClient::new(config(&server.base_url, ApiBackend::ChatCompletions, sink))
        .expect("client");

    client.conversation(hi()).await.expect("completion");

    let calls = recorder.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().and_then(|c| c.cost_usd_ticks), Some(11));
    let _ = server.shutdown.send(());
}

/// The actor holds the slot itself and calls the client under it. The call is metered once.
/// A try that fails after its usage arrived still reports it, and the retry reports its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_actor_path_meters_each_attempt_once() {
    let (recorder, sink) = recorder();
    let served = Arc::new(AtomicUsize::new(0));
    let server = serve(chat_server(Arc::clone(&served), 1)).await;
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        config(&server.base_url, ApiBackend::ChatCompletions, sink),
        RetryPolicy::default(),
        event_tx,
    );

    let (response, _metrics) = handle
        .submit_and_collect(RequestId::from("metered"), hi())
        .await
        .expect("the retry succeeds");
    assert_eq!(response.assistant_text(), "answer");
    assert_eq!(
        served.load(Ordering::SeqCst),
        2,
        "one failed try, one retry"
    );

    let ticks: Vec<Option<i64>> = recorder.calls().iter().map(|c| c.cost_usd_ticks).collect();
    assert_eq!(ticks, vec![Some(5), Some(7)]);
    let _ = server.shutdown.send(());
}

/// The Responses stream reports on its terminal event, with the cost the provider put on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_responses_stream_reports_the_provider_cost() {
    let (recorder, sink) = recorder();
    let app = Router::new().route(
        "/v1/responses",
        post(|| async {
            let completed = json!({
                "type": "response.completed",
                "sequence_number": 1,
                "response": {
                    "id": "r", "object": "response", "created_at": 0, "status": "completed",
                    "model": "served-model", "output": [{
                        "type": "message", "id": "m", "role": "assistant", "status": "completed",
                        "content": [{ "type": "output_text", "text": "done", "annotations": [] }]
                    }],
                    "usage": {
                        "input_tokens": 40, "output_tokens": 4, "total_tokens": 44,
                        "input_tokens_details": { "cached_tokens": 8 },
                        "output_tokens_details": { "reasoning_tokens": 0 },
                        "cost_in_usd_ticks": 123
                    }
                }
            });
            Sse::new(stream::iter(vec![Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event("response.completed")
                    .data(completed.to_string()),
            )]))
        }),
    );
    let server = serve(app).await;
    let client =
        SamplingClient::new(config(&server.base_url, ApiBackend::Responses, sink)).expect("client");

    let response = client.conversation_collect(hi()).await.expect("collect");
    assert_eq!(response.assistant_text(), "done");

    let calls = recorder.calls();
    assert_eq!(calls.len(), 1);
    let call = calls.first().expect("one call");
    assert_eq!(call.cost_usd_ticks, Some(123));
    assert_eq!(call.model, "served-model");
    assert_eq!(call.usage.as_ref().map(|u| u.cached_prompt_tokens), Some(8));
    let _ = server.shutdown.send(());
}
