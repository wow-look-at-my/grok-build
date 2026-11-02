//! Integration test: the shell re-parks `exit_plan_mode` on resume, so approval chrome reappears after quit/`--continue`.
//! Approving then leaves plan mode and starts the implement turn.
//!
//! CI stages the pager binary via `PAGER_BINARY`.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_approval_restored_after_resume() {
    xai_grok_pager_pty_harness::scenarios::plan_approval_resume::assert_plan_approval_restored_after_resume()
        .await
        .expect("shell must re-park exit_plan_mode on resume so approval chrome returns");
}
