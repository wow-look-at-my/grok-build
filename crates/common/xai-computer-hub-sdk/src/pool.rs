//! Process-wide connection pool keyed by `(url, principal)`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;
use url::Url;
use xai_tool_protocol::ConnectionKind;

use crate::auth::AuthProvider;
use crate::connection::{
    ConnKey, ConnectCallback, ConnectionConfig, ConnectionTuning, DisconnectCallback,
    HandshakeRefusedCallback, HubConnection, ReconnectCallback, TerminalCloseCallback,
};
use crate::error::ClientError;

/// Idle window for the reaper: a pooled connection is evictable once it is unused (`Arc::strong_count == 1`, i.e. only the pool holds it).
pub const DEFAULT_POOL_IDLE_TTL: Duration = Duration::from_secs(300);

/// How often the shared pool's idle reaper scans for evictable entries.
pub const DEFAULT_POOL_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// A pooled connection plus the last time it was handed out to a caller.
struct Pooled {
    conn: Arc<HubConnection>,
    last_handout: Instant,
}

/// The process-global pool used by [`HubConnectionPool::shared`].
static SHARED: OnceCell<Arc<HubConnectionPool>> = OnceCell::const_new();

/// Pool of live server connections.
pub struct HubConnectionPool {
    connections: DashMap<ConnKey, Pooled>,
}

impl std::fmt::Debug for HubConnectionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubConnectionPool")
            .field("connection_count", &self.connections.len())
            .finish()
    }
}

impl HubConnectionPool {
    /// Build a fresh, unshared pool. Tests typically use this so each
    /// test sees an isolated registry.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            connections: DashMap::new(),
        })
    }

    /// Return the process-wide shared pool, lazily initialising it on the
    /// first call. Subsequent callers in the same process observe the same
    /// `Arc`.
    pub async fn shared() -> Arc<Self> {
        SHARED
            .get_or_init(|| async {
                let pool = Self::new();
                pool.spawn_idle_reaper(DEFAULT_POOL_IDLE_TTL, DEFAULT_POOL_SWEEP_INTERVAL);
                pool
            })
            .await
            .clone()
    }

    /// Look up an existing pooled connection for `(url, credential)`, or open
    /// a fresh one if no pooled entry exists. `kind` is the connection role
    /// announced in the hello frame. The pool is keyed by `(url, principal)`
    /// only; mixing [`ConnectionKind`] values for the same `(url, principal)`
    /// is a caller error and surfaces as a [`ClientError::InvalidConfig`].
    /// The optional extra access key is not part of the pool key, so the
    /// first caller's key is the one carried on a shared connection's
    /// handshake (in practice it is a per-deployment constant).
    pub async fn get_or_connect(
        self: &Arc<Self>,
        url: Url,
        credential: Arc<dyn AuthProvider>,
        kind: ConnectionKind,
        on_reconnect: Option<Arc<ReconnectCallback>>,
        on_disconnect: Option<Arc<DisconnectCallback>>,
        server_id: Option<xai_tool_protocol::ServerId>,
        alpha_test_key: Option<String>,
        allow_insecure_ws: bool,
    ) -> Result<Arc<HubConnection>, ClientError> {
        self.get_or_connect_tuned(
            url,
            credential,
            kind,
            on_reconnect,
            on_disconnect,
            None, // on_connect (unused by the simple wrapper)
            None, // on_terminal_close (unused by the simple wrapper)
            None, // on_handshake_refused (unused by the simple wrapper)
            server_id,
            None,
            None,
            alpha_test_key,
            allow_insecure_ws,
            ConnectionTuning::default(),
        )
        .await
    }

    /// Like [`Self::get_or_connect`] but carries optional connection-tuning
    /// knobs ([`ConnectionTuning`]) onto a freshly-opened connection. A
    /// `Default` tuning is behaviourally identical to `get_or_connect`, so
    /// existing callers are unaffected.
    ///
    /// Tuning binds to the socket at open time: it takes effect only when
    /// THIS call opens the connection. Because the pool dedups by
    /// `(url, principal)`, a hit on an existing entry returns that
    /// connection as-is and the `tuning` argument is ignored — the first
    /// opener's ping/backoff settings win for the lifetime of the pooled
    /// connection. Callers that need distinct tuning must use a distinct
    /// `(url, principal)` or an unpooled [`HubConnection::connect`].
    pub(crate) async fn get_or_connect_tuned(
        self: &Arc<Self>,
        url: Url,
        credential: Arc<dyn AuthProvider>,
        kind: ConnectionKind,
        on_reconnect: Option<Arc<ReconnectCallback>>,
        on_disconnect: Option<Arc<DisconnectCallback>>,
        on_connect: Option<Arc<ConnectCallback>>,
        on_terminal_close: Option<Arc<TerminalCloseCallback>>,
        on_handshake_refused: Option<Arc<HandshakeRefusedCallback>>,
        server_id: Option<xai_tool_protocol::ServerId>,
        server_description: Option<String>,
        server_metadata: Option<serde_json::Value>,
        alpha_test_key: Option<String>,
        allow_insecure_ws: bool,
        tuning: ConnectionTuning,
    ) -> Result<Arc<HubConnection>, ClientError> {
        if url.scheme() != "wss" && !crate::connection::host_is_loopback(&url) && !allow_insecure_ws
        {
            return Err(ClientError::InsecureScheme { url });
        }
        let key = ConnKey {
            url: url.as_str().to_owned(),
            principal: credential.principal_key(),
        };
        if let Some(mut existing) = self.connections.get_mut(&key) {
            existing.last_handout = Instant::now();
            let conn = existing.conn.clone();
            drop(existing);
            if conn.kind() != kind {
                return Err(ClientError::InvalidConfig(format!(
                    "pool entry for {} bound to {:?}; rebuild requested {:?}",
                    key.url,
                    conn.kind(),
                    kind
                )));
            }
            return Ok(conn);
        }
        let config = ConnectionConfig {
            url,
            credential,
            kind,
            on_reconnect,
            on_disconnect,
            on_terminal_close,
            on_handshake_refused,
            on_connect,
            server_id,
            server_description,
            server_metadata,
            outbound_buffer: None,
            tuning,
            alpha_test_key,
            allow_insecure_ws,
            on_fatal: Some(Arc::downgrade(self)),
        };
        let conn = HubConnection::connect(config).await?;
        // Race window: another caller may have inserted between our
        // `get` and `connect`. Resolve via `entry().or_insert_with`
        // semantics — if we lose the race we drop our fresh
        // connection and adopt the winning one.
        match self.connections.entry(key.clone()) {
            dashmap::Entry::Occupied(mut existing) => {
                existing.get_mut().last_handout = Instant::now();
                let winner = existing.get().conn.clone();
                drop(conn);
                if winner.kind() != kind {
                    return Err(ClientError::InvalidConfig(format!(
                        "pool entry for {} bound to {:?}; rebuild requested {:?}",
                        key.url,
                        winner.kind(),
                        kind
                    )));
                }
                Ok(winner)
            }
            dashmap::Entry::Vacant(slot) => {
                crate::metrics::pool_connections_inc();
                slot.insert(Pooled {
                    conn: conn.clone(),
                    last_handout: Instant::now(),
                });
                Ok(conn)
            }
        }
    }

    /// Number of pooled connections. Intended for tests and metrics.
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    /// `true` when no connection is pooled.
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Forget the pooled connection for `key`.
    pub fn forget(&self, key: &ConnKey) {
        if self.connections.remove(key).is_some() {
            crate::metrics::pool_connections_dec();
        }
    }

    /// Close and remove every pooled connection that is BOTH unused (no live
    /// consumer holds an `Arc` — only the pool does, so `strong_count ==
    /// 1`) AND idle longer than `idle_ttl` (no hand-out within the window).
    /// Removing the entry drops the pool's last `Arc<HubConnection>`, whose
    /// `Drop` closes the socket.
    pub fn sweep_idle(&self, idle_ttl: Duration) -> usize {
        let now = Instant::now();
        let mut evicted = 0usize;
        self.connections.retain(|_key, pooled| {
            let idle_for = now.saturating_duration_since(pooled.last_handout);
            // `strong_count == 1` ⇒ only this pool entry references the connection.
            let unused = Arc::strong_count(&pooled.conn) == 1;
            let evict = unused && idle_for >= idle_ttl;
            if evict {
                evicted += 1;
            }
            !evict
        });
        for _ in 0..evicted {
            crate::metrics::pool_connections_dec();
            crate::metrics::pool_evictions_inc();
        }
        evicted
    }

    /// Spawn a background task that calls [`Self::sweep_idle`] every
    /// `sweep_interval`, closing connections idle longer than `idle_ttl`.
    ///
    /// The task holds a [`std::sync::Weak`] to the pool, so it exits on its
    /// own once the last strong `Arc<HubConnectionPool>` is dropped (it never
    /// keeps the pool alive). The first interval tick is skipped so a
    /// freshly-handed-out connection is never swept on the immediate tick.
    pub fn spawn_idle_reaper(
        self: &Arc<Self>,
        idle_ttl: Duration,
        sweep_interval: Duration,
    ) -> JoinHandle<()> {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(sweep_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // `interval`'s first tick resolves immediately; skip it.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Some(pool) = weak.upgrade() else { break };
                pool.sweep_idle(idle_ttl);
            }
        })
    }

    /// Like [`Self::forget`] but identity-checked: only removes the slot when
    /// `predicate` accepts the currently-stored connection.
    pub(crate) fn forget_if(
        &self,
        key: &ConnKey,
        predicate: impl FnOnce(&Arc<HubConnection>) -> bool,
    ) {
        if self
            .connections
            .remove_if(key, |_, pooled| predicate(&pooled.conn))
            .is_some()
        {
            crate::metrics::pool_connections_dec();
        }
    }
}
