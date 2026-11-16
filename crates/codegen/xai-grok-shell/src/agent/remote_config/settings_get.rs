//! Each function here calls the `xai-grok-cloud-config` startup settings getter with the shell's `policy_repair_pending` check.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use xai_grok_login::{GrokAuth, GrokComConfig};

<<<<<<< fork
/// A settings load request. Warm a load with [`resolve`](Self::resolve) or
/// [`from_config`](Self::from_config); a consume-time recheck reuses the warmed
/// [`auth`](Self::auth) rather than re-reading disk.
#[derive(Clone)]
pub struct SettingsQuery {
    auth: Option<GrokAuth>,
    origin: String,
    alpha_test_key: Option<String>,
    /// The grok.invalid config disk auth was resolved under. The commit-time
    /// identity re-check must resolve through the same config: the default
    /// config only sees env, so a file- or managed-configured IdP would
    /// otherwise read as a credential change on every load.
    auth_config: Option<GrokComConfig>,
}

impl SettingsQuery {
    pub(crate) fn from_config(cfg: &Config, auth: Option<GrokAuth>) -> Self {
        Self::resolve(auth, Some(cfg.grok_com_config.clone()))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_auth(auth: Option<GrokAuth>) -> Self {
        Self::resolve(auth, None)
    }

    /// Build from the live parts a caller already holds, skipping the disk-auth
    /// and endpoint resolution `resolve` does.
    pub(crate) fn from_parts(
        auth: Option<GrokAuth>,
        origin: String,
        alpha_test_key: Option<String>,
        auth_config: Option<GrokComConfig>,
    ) -> Self {
        Self {
            auth,
            origin,
            alpha_test_key,
            auth_config,
        }
    }

    /// The auth this query resolved to. A consume-time recheck reuses this
    /// warmed credential instead of re-reading disk, so a just-refreshed
    /// session that has not yet been persisted is not mistaken for a change.
    pub fn auth(&self) -> Option<&GrokAuth> {
        self.auth.as_ref()
    }

    /// `auth` wins; otherwise the on-disk session for `grok_com_config`.
    pub fn resolve(auth: Option<GrokAuth>, grok_com_config: Option<GrokComConfig>) -> Self {
        let endpoints = super::resolve_startup_endpoints();
        let auth = auth.or_else(|| super::resolve_disk_auth(grok_com_config.clone()));
        Self {
            auth,
            origin: endpoints.proxy_url(),
            alpha_test_key: endpoints.alpha_test_key,
            auth_config: grok_com_config,
        }
    }
}

/// Result of a settings load. Absence is not a permanent empty policy and does
/// not spend a later refresh.
#[derive(Clone, Debug)]
pub struct SettingsOutcome {
    pub(crate) settings: Option<RemoteSettings>,
    pub(crate) attempted: bool,
    origin: String,
    identity: String,
}
||||||| base
/// A settings load request. Warm a load with [`resolve`](Self::resolve) or
/// [`from_config`](Self::from_config); a consume-time recheck reuses the warmed
/// [`auth`](Self::auth) rather than re-reading disk.
#[derive(Clone)]
pub struct SettingsQuery {
    auth: Option<GrokAuth>,
    origin: String,
    alpha_test_key: Option<String>,
    /// The grok.com config disk auth was resolved under. The commit-time
    /// identity re-check must resolve through the same config: the default
    /// config only sees env, so a file- or managed-configured IdP would
    /// otherwise read as a credential change on every load.
    auth_config: Option<GrokComConfig>,
}

impl SettingsQuery {
    pub(crate) fn from_config(cfg: &Config, auth: Option<GrokAuth>) -> Self {
        Self::resolve(auth, Some(cfg.grok_com_config.clone()))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_auth(auth: Option<GrokAuth>) -> Self {
        Self::resolve(auth, None)
    }

    /// Build from the live parts a caller already holds, skipping the disk-auth
    /// and endpoint resolution `resolve` does.
    pub(crate) fn from_parts(
        auth: Option<GrokAuth>,
        origin: String,
        alpha_test_key: Option<String>,
        auth_config: Option<GrokComConfig>,
    ) -> Self {
        Self {
            auth,
            origin,
            alpha_test_key,
            auth_config,
        }
    }

    /// The auth this query resolved to. A consume-time recheck reuses this
    /// warmed credential instead of re-reading disk, so a just-refreshed
    /// session that has not yet been persisted is not mistaken for a change.
    pub fn auth(&self) -> Option<&GrokAuth> {
        self.auth.as_ref()
    }

    /// `auth` wins; otherwise the on-disk session for `grok_com_config`.
    pub fn resolve(auth: Option<GrokAuth>, grok_com_config: Option<GrokComConfig>) -> Self {
        let endpoints = super::resolve_startup_endpoints();
        let auth = auth.or_else(|| super::resolve_disk_auth(grok_com_config.clone()));
        Self {
            auth,
            origin: endpoints.proxy_url(),
            alpha_test_key: endpoints.alpha_test_key,
            auth_config: grok_com_config,
        }
    }
}

/// Result of a settings load. Absence is not a permanent empty policy and does
/// not spend a later refresh.
#[derive(Clone, Debug)]
pub struct SettingsOutcome {
    pub(crate) settings: Option<RemoteSettings>,
    pub(crate) attempted: bool,
    origin: String,
    identity: String,
}
=======
use crate::util::config::RemoteSettings;
use xai_grok_cloud_config::managed_config::policy_repair_pending;
>>>>>>> upstream

#[cfg(any(test, feature = "test-support"))]
pub use xai_grok_cloud_config::settings_get::reset_startup_settings_for_tests;

<<<<<<< fork
fn lock_state() -> std::sync::MutexGuard<'static, StartupState> {
    startup_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A completed skip (not attempted, no settings) published because the scope
/// went ineligible after the owner was chosen. A now-eligible scope must not
/// reuse it.
fn owner_published_skip(owner: &Owner) -> bool {
    owner
        .tx
        .borrow()
        .as_ref()
        .is_some_and(|outcome| !outcome.attempted && outcome.settings.is_none())
}

/// Choose one owner per scope and return a receiver on its outcome. The state
/// lock is held across the check, spawn, and store, so two concurrent starts
/// cannot both fetch; a start for a different scope aborts the stale owner
/// first. Returns `None` only when there is no runtime to spawn on.
fn warm_and_subscribe(query: &SettingsQuery) -> Option<watch::Receiver<Option<SettingsOutcome>>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let key = OwnerKey::of(query);
    let mut state = lock_state();
    if let Some(owner) = &state.owner {
        if owner.key == key && !owner_published_skip(owner) {
            return Some(owner.tx.subscribe());
        }
        // Different scope, or a completed skip from transient ineligibility (a
        // repair). A now-eligible scope must start a fresh load instead of
        // reusing that skip for the process lifetime.
        owner.abort.abort();
    }
    let (tx, rx) = watch::channel(None);
    let task_tx = tx.clone();
    let task_query = query.clone();
    let task = xai_spawn::spawn_on(&runtime, async move {
        let outcome = load_settings(task_query).await;
        // send_replace stores the value with no live receiver, so it is never lost.
        task_tx.send_replace(Some(outcome));
    });
    state.owner = Some(Owner {
        key,
        tx,
        abort: task.abort_handle(),
    });
    Some(rx)
}
||||||| base
fn lock_state() -> std::sync::MutexGuard<'static, StartupState> {
    startup_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A completed skip (not attempted, no settings) published because the scope
/// went ineligible after the owner was chosen. A now-eligible scope must not
/// reuse it.
fn owner_published_skip(owner: &Owner) -> bool {
    owner
        .tx
        .borrow()
        .as_ref()
        .is_some_and(|outcome| !outcome.attempted && outcome.settings.is_none())
}

/// Choose one owner per scope and return a receiver on its outcome. The state
/// lock is held across the check, spawn, and store, so two concurrent starts
/// cannot both fetch; a start for a different scope aborts the stale owner
/// first. Returns `None` only when there is no runtime to spawn on.
fn warm_and_subscribe(query: &SettingsQuery) -> Option<watch::Receiver<Option<SettingsOutcome>>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let key = OwnerKey::of(query);
    let mut state = lock_state();
    if let Some(owner) = &state.owner {
        if owner.key == key && !owner_published_skip(owner) {
            return Some(owner.tx.subscribe());
        }
        // Different scope, or a completed skip from transient ineligibility (a
        // repair). A now-eligible scope must start a fresh load instead of
        // reusing that skip for the process lifetime.
        owner.abort.abort();
    }
    let (tx, rx) = watch::channel(None);
    let task_tx = tx.clone();
    let task_query = query.clone();
    let task = runtime.spawn(async move {
        let outcome = load_settings(task_query).await;
        // send_replace stores the value with no live receiver, so it is never lost.
        task_tx.send_replace(Some(outcome));
    });
    state.owner = Some(Owner {
        key,
        tx,
        abort: task.abort_handle(),
    });
    Some(rx)
}
=======
pub use xai_grok_cloud_config::settings_get::{SettingsOutcome, SettingsQuery, SettingsWait};
>>>>>>> upstream

pub fn is_eligible(query: &SettingsQuery) -> bool {
    xai_grok_cloud_config::settings_get::is_eligible(query, policy_repair_pending)
}

#[cfg(any(test, feature = "test-support"))]
pub async fn get_settings(query: SettingsQuery) -> SettingsOutcome {
    xai_grok_cloud_config::settings_get::get_settings(query, policy_repair_pending).await
}

<<<<<<< fork
/// Live `/v1/settings` read for mid-session refresh. Does not consult the
/// startup cache. A successful fetch is written through so a later startup
/// can hit disk. `SettingsFetch` variants are preserved, including `Rejected`.
pub(crate) async fn fetch_settings_live(query: SettingsQuery) -> crate::remote::SettingsFetch {
    let Some(auth) = query.auth.clone() else {
        return crate::remote::SettingsFetch::Retry;
    };
    let origin = query.origin.clone();
    let alpha = query.alpha_test_key.clone();
    let auth_config = query.auth_config.clone();
    let loaded = xai_spawn::spawn_blocking(move || {
        let fetched_at = chrono::Utc::now();
        let fetch = crate::remote::fetch_settings_blocking(&origin, &auth, alpha.as_deref());
        if let crate::remote::SettingsFetch::Fetched(settings) = &fetch {
            let current_origin = super::resolve_startup_endpoints().proxy_url();
            let identity = SettingsCacheManager::identity(&auth, alpha.as_deref());
            // Write the cache only when the whole scope is still current; a
            // not-yet-persisted session is still served in memory by the caller.
            if matches!(
                super::evaluate_commit(
                    &current_origin,
                    &origin,
                    &identity,
                    auth_config.as_ref(),
                    alpha.as_deref(),
                ),
                super::Commit::CacheAndServe
            ) {
                SettingsCacheManager::new().write_through(
                    &auth,
                    &origin,
                    alpha.as_deref(),
                    settings,
                    fetched_at,
                );
            }
        }
        fetch
    })
    .await;

    match loaded {
        Ok(fetch) => fetch,
        Err(err) => {
            tracing::warn!(error = %err, "settings refresh task failed");
            crate::remote::SettingsFetch::Retry
        }
    }
}

/// Observe this scope's startup load, warming it first when eligible. An
/// ineligible query returns immediately. The observed value is always for the
/// caller's own scope, so no post-hoc match is needed.
||||||| base
/// Live `/v1/settings` read for mid-session refresh. Does not consult the
/// startup cache. A successful fetch is written through so a later startup
/// can hit disk. `SettingsFetch` variants are preserved, including `Rejected`.
pub(crate) async fn fetch_settings_live(query: SettingsQuery) -> crate::remote::SettingsFetch {
    let Some(auth) = query.auth.clone() else {
        return crate::remote::SettingsFetch::Retry;
    };
    let origin = query.origin.clone();
    let alpha = query.alpha_test_key.clone();
    let auth_config = query.auth_config.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let fetched_at = chrono::Utc::now();
        let fetch = crate::remote::fetch_settings_blocking(&origin, &auth, alpha.as_deref());
        if let crate::remote::SettingsFetch::Fetched(settings) = &fetch {
            let current_origin = super::resolve_startup_endpoints().proxy_url();
            let identity = SettingsCacheManager::identity(&auth, alpha.as_deref());
            // Write the cache only when the whole scope is still current; a
            // not-yet-persisted session is still served in memory by the caller.
            if matches!(
                super::evaluate_commit(
                    &current_origin,
                    &origin,
                    &identity,
                    auth_config.as_ref(),
                    alpha.as_deref(),
                ),
                super::Commit::CacheAndServe
            ) {
                SettingsCacheManager::new().write_through(
                    &auth,
                    &origin,
                    alpha.as_deref(),
                    settings,
                    fetched_at,
                );
            }
        }
        fetch
    })
    .await;

    match loaded {
        Ok(fetch) => fetch,
        Err(err) => {
            tracing::warn!(error = %err, "settings refresh task failed");
            crate::remote::SettingsFetch::Retry
        }
    }
}

/// Observe this scope's startup load, warming it first when eligible. An
/// ineligible query returns immediately. The observed value is always for the
/// caller's own scope, so no post-hoc match is needed.
=======
>>>>>>> upstream
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
    let loaded = xai_spawn::spawn_blocking(move || load_settings_blocking(query)).await;
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
