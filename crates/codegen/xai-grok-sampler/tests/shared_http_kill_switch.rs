//! Kill-switch test in its own integration binary: a separate test binary is a separate process under cargo test, nextest.

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use support::{send_one, test_config};
use xai_grok_sampler::SamplingClient;
use xai_grok_test_support::spawn_counting_server;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_switch_builds_fresh_client_per_sampling_client() {
    // Safety: the only test in this binary, set before any client exists; no concurrent env reads are possible.
    unsafe { std::env::set_var("GROK_SAMPLER_SHARED_CLIENT", "0") };
    let (base_url, accepts, _heads) = spawn_counting_server().await;
    let a = SamplingClient::new(test_config(&base_url, "token-a")).unwrap();
    let b = SamplingClient::new(test_config(&base_url, "token-b")).unwrap();
    send_one(&a).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    send_one(&b).await;
    assert_eq!(accepts.load(Ordering::SeqCst), 2);
}
