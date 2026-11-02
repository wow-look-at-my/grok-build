//! Wire tests for the shared non-sampling client.

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use xai_grok_http::shared_client;
use xai_grok_net::Auth;
use xai_grok_test_support::counting_server::{spawn_counting_server, spawn_http_server};

const BODY: &str = r#"{"settings":{"theme":"dark"},"note":"gzip decodes to this text"}"#;

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(body).expect("write gzip body");
    encoder.finish().expect("finish gzip body")
}

fn http_response(body: &[u8], encoding: Option<&str>) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n",
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

async fn fetch_text(base_url: &str) -> String {
    shared_client()
        .get(format!("{base_url}/settings"), Auth::None)
        .send()
        .await
        .expect("request completes")
        .error_for_status()
        .expect("response status is success")
        .text()
        .await
        .expect("body reads as text")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_client_advertises_gzip() {
    let (base_url, _accepts, heads) = spawn_counting_server().await;
    let _ = fetch_text(&base_url).await;

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
async fn shared_client_returns_a_gzip_response_as_its_original_text() {
    let compressed = gzip(BODY.as_bytes());
    let base_url = spawn_http_server(move |_head| http_response(&compressed, Some("gzip"))).await;

    assert_eq!(fetch_text(&base_url).await, BODY);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_client_returns_a_plain_response_unchanged() {
    let plain = BODY.as_bytes().to_vec();
    let base_url = spawn_http_server(move |_head| http_response(&plain, None)).await;

    assert_eq!(fetch_text(&base_url).await, BODY);
}
