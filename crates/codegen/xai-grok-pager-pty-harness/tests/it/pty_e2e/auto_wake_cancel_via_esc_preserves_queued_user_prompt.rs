//! Same scenario as `auto_wake_cancel_preserves_queued_user_prompt`, cancelled by a bare Esc.
#[allow(unused_imports)]
use super::common::*;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "PTY e2e; run with cargo test -p xai-grok-pager-pty-harness --test it -- --ignored"]
async fn auto_wake_cancel_via_esc_preserves_queued_user_prompt() {
    use super::auto_wake_cancel_preserves_queued_user_prompt::{
        WakeCancelGesture, run_wake_cancel_scenario,
    };
    run_wake_cancel_scenario(WakeCancelGesture::Esc, "auto_wake_esc").await;
}
