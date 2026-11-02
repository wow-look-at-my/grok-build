//! Wire tests for response compression.

mod support;

use std::io::Write;
use std::sync::Arc;

use flate2::Compression;
use flate2::write::GzEncoder;
use futures_util::StreamExt;
use xai_grok_sampler::SamplingClient;
use xai_grok_sampling_types::{
    ChatCompletionChunk, ChatCompletionResponse, ContentPart, ConversationItem,
    ConversationRequest, SamplingError, UserItem,
};
use xai_grok_test_support::counting_server::{spawn_counting_server, spawn_http_server};
use xai_grok_test_support::sse::chat_completion_script_exact;

const COMPLETION_TEXT: &str = "gzip decodes to this text";
const MODEL: &str = "test-model";

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(body).expect("write gzip body");
    encoder.finish().expect("finish gzip body")
}

/// An HTTP/1.1 response carrying `body`, optionally gzip-compressed and
/// labelled with `content-encoding` the way a compressing endpoint does.
fn http_response(content_type: &str, body: &[u8], encoding: Option<&str>) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n",
        body.len()
    );
    if let Some(encoding) = encoding {
        head.push_str(&format!("content-encoding: {encoding}\r\n"));
    }
    head.push_str("\r\n");
    let mut response = head.into_bytes();
    response.extend_from_slice(body);
    response
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn completion_json() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "id": "chatcmpl-gzip",
        "object": "chat.completion",
        "created": 1,
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": COMPLETION_TEXT},
            "finish_reason": "stop",
        }],
    }))
    .expect("serialize completion fixture")
}

fn completion_stream_body() -> Vec<u8> {
    let mut body = String::new();
    for event in chat_completion_script_exact(COMPLETION_TEXT, MODEL) {
        if let Some(name) = &event.event {
            body.push_str(&format!("event: {name}\n"));
        }
        body.push_str(&format!("data: {}\n\n", event.data));
    }
    body.into_bytes()
}

fn conversation_request() -> ConversationRequest {
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

async fn conversation(base_url: &str) -> ChatCompletionResponse {
    let client =
        SamplingClient::new(support::test_config(base_url, "test-token")).expect("client builds");
    client
        .conversation(conversation_request())
        .await
        .expect("non-streaming completion parses")
}

async fn stream_chunks(base_url: &str) -> Vec<ChatCompletionChunk> {
    let client =
        SamplingClient::new(support::test_config(base_url, "test-token")).expect("client builds");
    let (mut stream, _metadata) = client
        .conversation_stream(conversation_request())
        .await
        .expect("stream opens");
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => chunks.push(chunk),
            Err(error) => panic!("stream chunk parses: {error:?}"),
        }
    }
    chunks
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_advertise_gzip() {
    support::pin_env();
    let (base_url, _accepts, heads) = spawn_counting_server().await;
    let client =
        SamplingClient::new(support::test_config(&base_url, "test-token")).expect("client builds");
    support::send_one(&client).await;

    let heads = heads.lock().unwrap();
    assert_eq!(heads.len(), 1, "one request was captured");
    let accept_encoding = header_value(&heads[0], "accept-encoding")
        .expect("the client sends an accept-encoding header");
    assert!(
        accept_encoding
            .split(',')
            .any(|coding| coding.trim() == "gzip"),
        "accept-encoding must offer gzip, got {accept_encoding:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gzip_json_response_parses_on_the_non_streaming_path() {
    support::pin_env();
    let compressed = gzip(&completion_json());
    let base_url = spawn_http_server(move |_head| {
        http_response("application/json", &compressed, Some("gzip"))
    })
    .await;

    let response = conversation(&base_url).await;
    assert_eq!(
        response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref()),
        Some(COMPLETION_TEXT)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_json_response_parses_on_the_non_streaming_path() {
    support::pin_env();
    let body = completion_json();
    let base_url =
        spawn_http_server(move |_head| http_response("application/json", &body, None)).await;

    let response = conversation(&base_url).await;
    assert_eq!(
        response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref()),
        Some(COMPLETION_TEXT)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gzip_sse_response_streams_the_same_events_as_the_plain_control() {
    support::pin_env();
    let compressed = gzip(&completion_stream_body());
    let gzip_base_url = spawn_http_server(move |_head| {
        http_response("text/event-stream", &compressed, Some("gzip"))
    })
    .await;
    let plain = completion_stream_body();
    let plain_base_url =
        spawn_http_server(move |_head| http_response("text/event-stream", &plain, None)).await;

    let gzip_chunks = stream_chunks(&gzip_base_url).await;
    let plain_chunks = stream_chunks(&plain_base_url).await;

    assert!(
        !gzip_chunks.is_empty(),
        "the stream yielded at least a chunk"
    );
    assert_eq!(gzip_chunks.len(), plain_chunks.len());
    let rendered = |chunks: &[ChatCompletionChunk]| -> Vec<serde_json::Value> {
        chunks
            .iter()
            .map(|chunk| serde_json::to_value(chunk).expect("chunk serializes"))
            .collect()
    };
    assert_eq!(rendered(&gzip_chunks), rendered(&plain_chunks));

    let text: String = gzip_chunks
        .iter()
        .filter_map(|chunk| chunk.choices.first())
        .filter_map(|choice| choice.delta.content.clone())
        .collect();
    assert_eq!(text, COMPLETION_TEXT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_gzip_request_reports_a_transport_error_rather_than_garbage() {
    support::pin_env();
    // A declaration the body does not honour: the client must fail loudly.
    let body = completion_json();
    let base_url =
        spawn_http_server(move |_head| http_response("application/json", &body, Some("gzip")))
            .await;

    let client =
        SamplingClient::new(support::test_config(&base_url, "test-token")).expect("client builds");
    match client.conversation(conversation_request()).await {
        Err(SamplingError::Http(_)) => {}
        other => panic!("an undecodable gzip body must fail as an HTTP error, got {other:?}"),
    }
}
