// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// Per-session count reset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn undo_tip_resets_each_new_session() {
    let content = ContentController::start().await.expect("start content");
    let binary = pager_binary().expect("resolve pager binary");
    // Both spawns share the env and the same $HOME TempDir Contextual hints ship disabled by default.
    let env_refs = CONTEXTUAL_HINTS_ENV;

    {
        let mut harness = PtyHarness::spawn_with_content_env(
            &binary,
            DEFAULT_ROWS,
            DEFAULT_COLS,
            &content,
            &[],
            env_refs,
        )
        .expect("spawn run 1");
        harness
            .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
            .expect("welcome run 1");
        for i in 1..=3 {
            wipe_substantial_draft(&mut harness);
            harness
                .wait_for_text(UNDO_TIP_SENTINEL, Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("tip must show on run-1 wipe #{i}: {e}"));
            // Wait out the TTL so the banner clears and the next show counts
            wait_for_labels_absent(&mut harness, &[UNDO_TIP_SENTINEL], Duration::from_secs(25));
            assert!(
                !harness.contains_text(UNDO_TIP_SENTINEL),
                "run-1 tip should expire via TTL before wipe #{}",
                i + 1
            );
        }
        // Cap reached: a further wipe shows nothing for the rest of this run.
        wipe_substantial_draft(&mut harness);
        harness.update(Duration::from_millis(1000));
        assert!(
            !harness.contains_text(UNDO_TIP_SENTINEL),
            "run-1 cap reached: the 4th wipe must be gated; screen:\n{}",
            harness.screen_contents()
        );
        harness.quit().expect("quit run 1");
    }

    // A persisted cap would suppress the tip here; per-session in-memory state means it shows again.
    {
        let mut harness = PtyHarness::spawn_with_content_env(
            &binary,
            DEFAULT_ROWS,
            DEFAULT_COLS,
            &content,
            &[],
            env_refs,
        )
        .expect("spawn run 2");
        harness
            .wait_for_text(WELCOME_SCREEN_SENTINEL, WELCOME_TIMEOUT)
            .expect("welcome run 2");
        wipe_substantial_draft(&mut harness);
        harness
            .wait_for_text(UNDO_TIP_SENTINEL, Duration::from_secs(10))
            .expect("tip MUST show again in a fresh session (nothing persisted)");
        harness.quit().expect("quit run 2");
    }
}
