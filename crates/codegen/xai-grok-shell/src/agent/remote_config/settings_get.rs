//! Each function here calls the `xai-grok-cloud-config` startup settings getter with the shell's `policy_repair_pending` check.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use xai_grok_login::{GrokAuth, GrokComConfig};

use crate::util::config::RemoteSettings;
use xai_grok_cloud_config::managed_config::policy_repair_pending;

#[cfg(any(test, feature = "test-support"))]
pub use xai_grok_cloud_config::settings_get::reset_startup_settings_for_tests;

pub use xai_grok_cloud_config::settings_get::{SettingsOutcome, SettingsQuery, SettingsWait};

pub fn is_eligible(query: &SettingsQuery) -> bool {
    xai_grok_cloud_config::settings_get::is_eligible(query, policy_repair_pending)
}

#[cfg(any(test, feature = "test-support"))]
pub async fn get_settings(query: SettingsQuery) -> SettingsOutcome {
    xai_grok_cloud_config::settings_get::get_settings(query, policy_repair_pending).await
}

pub async fn await_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
) -> SettingsWait {
    xai_grok_cloud_config::settings_get::await_startup_settings(
        query,
        deadline,
        cancel,
        policy_repair_pending,
    )
    .await
}

pub fn consume_wait(
    wait: SettingsWait,
    auth: Option<&GrokAuth>,
    grok_com_config: &GrokComConfig,
) -> Option<RemoteSettings> {
    xai_grok_cloud_config::settings_get::consume_wait(
        wait,
        auth,
        grok_com_config,
        policy_repair_pending,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub async fn get_startup_settings(query: SettingsQuery) -> SettingsOutcome {
    xai_grok_cloud_config::settings_get::get_startup_settings(query, policy_repair_pending).await
}

pub fn warm_startup_settings(query: SettingsQuery) {
    xai_grok_cloud_config::settings_get::warm_startup_settings(query, policy_repair_pending);
}

pub fn block_on_startup_settings(
    query: SettingsQuery,
    deadline: Duration,
    cancel: &CancellationToken,
) -> SettingsWait {
<<<<<<< fork
        let owner = xai_spawn::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            flag.store(true, Ordering::SeqCst);
            task_tx.send_replace(Some(SettingsOutcome {
                settings: None,
                attempted: true,
                origin: String::new(),
                identity: String::new(),
            }));
        });

        let wait = super::observe(
            tx.subscribe(),
            Some(Duration::from_millis(20)),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(wait, SettingsWait::TimedOut),
            "a short observation must time out, got {wait:?}"
        );
        assert!(
            !published.load(Ordering::SeqCst),
            "timing out the observation must not stop the owner"
        );

        // Bounded so a regression fails fast instead of wedging the shard.
        let later = tokio::time::timeout(
            Duration::from_secs(10),
            super::observe(tx.subscribe(), None, &CancellationToken::new()),
        )
        .await
        .expect("later publish must land");
        assert!(matches!(later, SettingsWait::Ready(_)));
        assert!(published.load(Ordering::SeqCst));
        owner.abort();
        reset_startup_settings_for_tests();
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn cancel_drops_only_the_observation() {
        reset_startup_settings_for_tests();
        let (tx, _rx) = tokio::sync::watch::channel(None);
        let cancel = CancellationToken::new();
        let cancel_for_wait = cancel.clone();
        let rx = tx.subscribe();
        let wait = xai_spawn::spawn(async move {
            super::observe(rx, Some(Duration::from_secs(30)), &cancel_for_wait).await
        });
        cancel.cancel();
        match wait.await.expect("observe task") {
            SettingsWait::Cancelled => {}
            other => panic!("expected cancel, got {other:?}"),
        }
        drop(tx);
        reset_startup_settings_for_tests();
    }
||||||| base
    if !is_eligible(&query) {
        return SettingsWait::Ready(Box::new(SettingsOutcome::skipped()));
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::info!("settings wait has no tokio runtime; falling open");
        return SettingsWait::TimedOut;
    };
    // Match before waiting: subscribe to this scope's owner, replacing a stale
    // owner for another scope rather than waiting out its deadline.
    let Some(rx) = warm_and_subscribe(&query) else {
        return SettingsWait::TimedOut;
    };
    if let Some(ready) = peek_published(&rx) {
        return ready;
    }
    match handle.runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(observe(rx, Some(deadline), cancel)))
        }
        _ => {
            tracing::info!(
                "settings wait on current-thread runtime has no published value; not blocking the runtime"
            );
            SettingsWait::TimedOut
        }
    }
}

fn peek_published(rx: &watch::Receiver<Option<SettingsOutcome>>) -> Option<SettingsWait> {
    rx.borrow()
        .clone()
        .map(|outcome| SettingsWait::Ready(Box::new(outcome)))
}

/// Observe the scope's load. `deadline` bounds the wait when `Some`; `None`
/// waits until the owner publishes, still bounded by the fetch timeouts.
async fn observe(
    mut rx: watch::Receiver<Option<SettingsOutcome>>,
    deadline: Option<Duration>,
    cancel: &CancellationToken,
) -> SettingsWait {
    if let Some(ready) = peek_published(&rx) {
        return ready;
    }
    let cancel = cancel.clone();
    match deadline {
        Some(deadline) => tokio::select! {
            biased;
            _ = cancel.cancelled() => SettingsWait::Cancelled,
            result = tokio::time::timeout(deadline, wait_for_published(&mut rx)) => match result {
                Ok(outcome) => SettingsWait::Ready(Box::new(outcome)),
                Err(_elapsed) => SettingsWait::TimedOut,
            },
        },
        None => tokio::select! {
            biased;
            _ = cancel.cancelled() => SettingsWait::Cancelled,
            outcome = wait_for_published(&mut rx) => SettingsWait::Ready(Box::new(outcome)),
        },
    }
}

async fn wait_for_published(rx: &mut watch::Receiver<Option<SettingsOutcome>>) -> SettingsOutcome {
    loop {
        if let Some(outcome) = rx.borrow_and_update().clone() {
            return outcome;
        }
        if rx.changed().await.is_err() {
            return SettingsOutcome::failed();
        }
    }
}

async fn load_settings(query: SettingsQuery) -> SettingsOutcome {
    let loaded = tokio::task::spawn_blocking(move || load_settings_blocking(query)).await;
    match loaded {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(error = %err, "settings getter task failed");
            SettingsOutcome::failed()
        }
    }
}

fn load_settings_blocking(query: SettingsQuery) -> SettingsOutcome {
    if !is_eligible(&query) {
        tracing::info!("settings getter skipped: ineligible");
        return SettingsOutcome::skipped();
    }
    let Some(auth) = query.auth.clone() else {
        return SettingsOutcome::skipped();
    };
    let _timer = crate::instrumentation_timer!("startup.settings_get");
    let origin = query.origin.clone();
    let alpha = query.alpha_test_key.clone();
    let auth_config = query.auth_config.clone();
    let (settings, write) =
        SettingsCacheManager::new().load_or_fetch(&auth, &origin, alpha.as_deref(), || {
            crate::remote::fetch_settings_blocking(&origin, &auth, alpha.as_deref()).into_option()
        });
    let identity = SettingsCacheManager::identity(&auth, alpha.as_deref());
    let Some(settings) = settings else {
        return SettingsOutcome::failed();
    };
    // Live fetch: a pending write, or the cache was disabled (no write). A cache
    // hit has no write, so the commit scope only gates live fetches.
    let is_live_fetch = write.is_some() || super::settings_cache_disabled();
    if is_live_fetch {
        let current_origin = super::resolve_startup_endpoints().proxy_url();
        match super::evaluate_commit(
            &current_origin,
            &origin,
            &identity,
            auth_config.as_ref(),
            alpha.as_deref(),
        ) {
            super::Commit::CacheAndServe => {
                if let Some(write) = write {
                    write.commit();
                }
            }
            super::Commit::ServeInMemory => {
                tracing::info!(
                    "settings fetch served in memory; not cached until the session persists"
                );
            }
            super::Commit::Retry => {
                tracing::info!("settings getter discarded fetch: policy repair pending");
                return SettingsOutcome::skipped();
            }
            super::Commit::Abandon => {
                tracing::info!("settings getter discarded fetch: origin or policy changed");
                return SettingsOutcome::failed();
            }
        }
    }
    SettingsOutcome {
        settings: Some(settings),
        attempted: is_live_fetch,
        origin,
        identity,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{
        SettingsOutcome, SettingsQuery, SettingsWait, await_startup_settings,
        reset_startup_settings_for_tests, warm_startup_settings,
    };

    fn ineligible_query() -> SettingsQuery {
        SettingsQuery::from_parts(None, "https://proxy.example".to_string(), None, None)
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn ineligible_warm_does_not_publish_or_seal() {
        reset_startup_settings_for_tests();
        let query = ineligible_query();
        warm_startup_settings(query.clone());
        assert!(
            super::lock_state().owner.is_none(),
            "an ineligible warm must leave the slot free"
        );
        let wait =
            await_startup_settings(query, Duration::from_secs(5), &CancellationToken::new()).await;
        let SettingsWait::Ready(outcome) = wait else {
            panic!("ineligible await must return immediately, got {wait:?}");
        };
        assert!(outcome.settings.is_none());
        assert!(!outcome.attempted);
        assert!(
            super::lock_state().owner.is_none(),
            "a skipped await must not seal the startup watch"
        );
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn caller_timeout_does_not_drop_a_later_publish() {
        reset_startup_settings_for_tests();
        let (tx, _rx0) = tokio::sync::watch::channel(None);
        let published = std::sync::Arc::new(AtomicBool::new(false));
        let flag = published.clone();
        let task_tx = tx.clone();
        // The owner stays private to this test: a global install could be aborted by any concurrent eligible warm.
        let owner = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            flag.store(true, Ordering::SeqCst);
            task_tx.send_replace(Some(SettingsOutcome {
                settings: None,
                attempted: true,
                origin: String::new(),
                identity: String::new(),
            }));
        });

        let wait = super::observe(
            tx.subscribe(),
            Some(Duration::from_millis(20)),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(wait, SettingsWait::TimedOut),
            "a short observation must time out, got {wait:?}"
        );
        assert!(
            !published.load(Ordering::SeqCst),
            "timing out the observation must not stop the owner"
        );

        // Bounded so a regression fails fast instead of wedging the shard.
        let later = tokio::time::timeout(
            Duration::from_secs(10),
            super::observe(tx.subscribe(), None, &CancellationToken::new()),
        )
        .await
        .expect("later publish must land");
        assert!(matches!(later, SettingsWait::Ready(_)));
        assert!(published.load(Ordering::SeqCst));
        owner.abort();
        reset_startup_settings_for_tests();
    }

    #[tokio::test]
    #[serial_test::serial(startup_settings)]
    async fn cancel_drops_only_the_observation() {
        reset_startup_settings_for_tests();
        let (tx, _rx) = tokio::sync::watch::channel(None);
        let cancel = CancellationToken::new();
        let cancel_for_wait = cancel.clone();
        let rx = tx.subscribe();
        let wait = tokio::spawn(async move {
            super::observe(rx, Some(Duration::from_secs(30)), &cancel_for_wait).await
        });
        cancel.cancel();
        match wait.await.expect("observe task") {
            SettingsWait::Cancelled => {}
            other => panic!("expected cancel, got {other:?}"),
        }
        drop(tx);
        reset_startup_settings_for_tests();
    }
=======
    xai_grok_cloud_config::settings_get::block_on_startup_settings(
        query,
        deadline,
        cancel,
        policy_repair_pending,
    )
>>>>>>> upstream
}
