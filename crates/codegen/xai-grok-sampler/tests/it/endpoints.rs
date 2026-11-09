//! The non-chat endpoints take a request slot and report the usage their answer carries.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message as AxumMessage, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::json;
use tokio::net::TcpListener;

use xai_grok_sampler::{
    BilledCall, SamplerConfig, SamplingClient, SharedUsageSink, UsageSink,
    set_max_parallel_requests,
};

#[derive(Debug, Default)]
struct Recorder {
    calls: Mutex<Vec<BilledCall>>,
}

impl UsageSink for Recorder {
    fn record_call(&self, call: BilledCall) {
        self.calls.lock().push(call);
    }
}

async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    xai_spawn::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/v1")
}

fn client(base_url: &str, sink: SharedUsageSink) -> SamplingClient {
    SamplingClient::new(SamplerConfig {
        api_key: Some("test-key".into()),
        base_url: base_url.to_owned(),
        model: "client-model".into(),
        usage_sink: Some(sink),
        ..Default::default()
    })
    .expect("client")
}

fn priced(model: &str, ticks: i64) -> axum::Json<serde_json::Value> {
    axum::Json(json!({ "model": model, "usage": { "cost_in_usd_ticks": ticks } }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_endpoint_reports_the_usage_its_answer_carries() {
    let app = Router::new()
        .route(
            "/v1/images/generations",
            post(|| async { priced("image-model", 10) }),
        )
        .route(
            "/v1/images/edits",
            post(|| async { priced("edit-model", 20) }),
        )
        .route(
            "/v1/videos/generations",
            post(|| async { axum::Json(json!({ "request_id": "r1" })) }),
        )
        .route("/v1/videos/r1", get(|| async { priced("video-model", 30) }))
        .route(
            "/v1/embeddings",
            post(|| async {
                axum::Json(json!({
                    "model": "embed-model",
                    "data": [],
                    "usage": { "prompt_tokens": 7, "total_tokens": 7 }
                }))
            }),
        )
        .route(
            "/v1/tokenize-text",
            post(|| async { axum::Json(json!({ "token_ids": [1, 2] })) }),
        );
    let base = serve(app).await;
    let recorder = Arc::new(Recorder::default());
    let client = client(&base, recorder.clone());
    let body = json!({ "model": "asked-model" });

    for response in [
        client.generate_images(&body, None).await,
        client.edit_images(&body, None).await,
        client.generate_video(&body, None).await,
        client.video_status("r1", None).await,
        client.create_embeddings(&body).await,
        client.tokenize(&body).await,
    ] {
        assert_eq!(response.expect("answer").status, StatusCode::OK);
    }

    let calls = recorder.calls.lock().clone();
    let seen: Vec<(String, Option<i64>, Option<u32>)> = calls
        .iter()
        .map(|c| {
            (
                c.model.clone(),
                c.cost_usd_ticks,
                c.usage.as_ref().map(|u| u.prompt_tokens),
            )
        })
        .collect();
    // The video start and the tokenizer carry no usage, so they report nothing.
    assert_eq!(
        seen,
        vec![
            ("image-model".to_owned(), Some(10), None),
            ("edit-model".to_owned(), Some(20), None),
            ("video-model".to_owned(), Some(30), None),
            ("embed-model".to_owned(), None, Some(7)),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_answer_comes_back_with_its_status_and_bills_nothing() {
    let app = Router::new().route(
        "/v1/images/generations",
        post(|| async { (StatusCode::BAD_REQUEST, "bad prompt") }),
    );
    let base = serve(app).await;
    let recorder = Arc::new(Recorder::default());
    let response = client(&base, recorder.clone())
        .generate_images(&json!({}), None)
        .await
        .expect("an answer");
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.text(), "bad prompt");
    assert!(recorder.calls.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_endpoint_request_waits_for_a_request_slot() {
    own_process!(an_endpoint_request_waits_for_a_request_slot);
    set_max_parallel_requests(1);
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let app = Router::new().route(
        "/v1/embeddings",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({ "data": [] }))
            }
        }),
    );
    let base = serve(app).await;
    let client = client(&base, Arc::new(Recorder::default()));

    let held = xai_grok_sampler::request_slots::global().acquire().await;
    let call = xai_spawn::spawn(async move { client.create_embeddings(&json!({})).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(served.load(Ordering::SeqCst), 0, "no slot, no request");

    drop(held);
    let response = call.await.expect("join").expect("answer");
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_websocket_holds_its_slot_until_it_is_dropped() {
    own_process!(a_websocket_holds_its_slot_until_it_is_dropped);
    set_max_parallel_requests(1);
    let auth_seen = Arc::new(Mutex::new(None::<String>));
    let seen = Arc::clone(&auth_seen);
    let app = Router::new().route(
        "/v1/stt",
        any(move |ws: WebSocketUpgrade, headers: HeaderMap| {
            let seen = Arc::clone(&seen);
            async move {
                *seen.lock() = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                ws.on_upgrade(|mut socket| async move {
                    let _ = socket.send(AxumMessage::Text("ready".into())).await;
                    while socket.recv().await.is_some() {}
                })
                .into_response()
            }
        }),
    );
    let base = serve(app).await;
    let client = client(&base, Arc::new(Recorder::default()));

    let mut session = client
        .connect_websocket("stt?sample_rate=16000", Default::default())
        .await
        .expect("socket");
    let first = session.socket.next().await.expect("frame").expect("ok");
    assert_eq!(first.into_text().expect("text").as_str(), "ready");
    assert_eq!(auth_seen.lock().as_deref(), Some("Bearer test-key"));
    assert!(session.slot.is_some());
    let slots = xai_grok_sampler::request_slots::global();
    assert!(
        slots.try_acquire().is_none(),
        "the socket holds the only slot"
    );

    let _ = session.socket.close(None).await;
    drop(session);
    assert!(
        slots.try_acquire().is_some(),
        "the slot comes back with the socket"
    );
}
