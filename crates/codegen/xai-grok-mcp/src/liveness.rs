//! Polls each `Ready` client for a closed transport.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::servers::{LivenessCheck, McpClient, McpClientEvent, McpServerName};

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The same Arc lives on the [`McpClient`] and is passed into the polling task so the task can clear the slot.
pub(crate) type SharedLivenessSlot = Arc<parking_lot::Mutex<Option<TransportLivenessHandle>>>;

/// Release the client's liveness slot, dropping any handle it held.
fn clear_liveness_slot(slot: &SharedLivenessSlot) {
    let stale_handle = slot.lock().take();
    drop(stale_handle);
}

/// RAII handle for the per-client liveness task.
pub struct TransportLivenessHandle {
    /// Exposed for diagnostics and log lines.
    pub server_name: McpServerName,
    /// On drop, cancels the spawned task. Field is held purely for its `Drop`; never read.
    _cancel: DropGuard,
}

impl std::fmt::Debug for TransportLivenessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportLivenessHandle")
            .field("server_name", &self.server_name)
            .finish()
    }
}

impl TransportLivenessHandle {
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

/// Spawn a one-shot transport-liveness poller for a `Ready` client.
/// Caller must already have observed `Ready`; only `Ready` with a closed transport emits, and a dropped receiver exits without retry.
/// `interval` ticks immediately so a transport already closed at spawn is detected; missed ticks are skipped.
pub fn spawn_transport_liveness(
    server_name: McpServerName,
    client: Arc<McpClient>,
    poll_interval: Duration,
    on_event: UnboundedSender<McpClientEvent>,
    liveness_slot: SharedLivenessSlot,
) -> TransportLivenessHandle {
    let token = CancellationToken::new();
    let drop_guard = token.clone().drop_guard();

    let server_name_for_task = server_name.clone();
    let slot_for_task = Arc::clone(&liveness_slot);
    // The handle parked in `liveness_slot` stands for "a watcher is alive for
    // this client". A task that unwound is not alive, so it clears the slot the
    // way every other exit does: `McpClient::arm_liveness_watcher` will not
    // install a fresh handle over the one already there, so a slot left held by
    // a dead watcher means that client's transport is never again detected as
    // closed.
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move {
        let watched = xai_grok_tools::util::detached::guarded(
            "mcp transport liveness watcher",
            watch_transport(
                server_name_for_task,
                client,
                poll_interval,
                on_event,
                token,
                liveness_slot,
            ),
        )
        .await;
        if let Err(panic) = watched {
            tracing::error!(
                panic = %panic,
                "transport liveness watcher died; clearing its slot so the client can be watched again"
            );
            clear_liveness_slot(&slot_for_task);
        }
    });

    TransportLivenessHandle {
        server_name,
        _cancel: drop_guard,
    }
}

/// Poll one client until its transport reads closed, until its state drifts
/// out of `Ready`, or until the handle's drop guard cancels `token`.
async fn watch_transport(
    server_name: McpServerName,
    client: Arc<McpClient>,
    poll_interval: Duration,
    on_event: UnboundedSender<McpClientEvent>,
    token: CancellationToken,
    liveness_slot: SharedLivenessSlot,
) {
    let server_name_for_task = server_name;
    let mut tick = tokio::time::interval(poll_interval);
    // Skip missed ticks under runtime stall — see fn doc.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                // Cancelled by the handle's `DropGuard`.
                tracing::trace!(
                    server = %server_name_for_task,
                    "transport liveness watcher cancelled by handle drop",
                );
                return;
            }
            _ = tick.tick() => {
                match client.liveness_check().await {
                    LivenessCheck::Healthy => continue,
                    LivenessCheck::TransportClosed => {
                        tracing::info!(
                            server = %server_name_for_task,
                            "transport liveness watcher detected closed transport",
                        );
                        // Clear our own slot before exiting so a subsequent `arm_liveness_watcher` can install a fresh handle. Self-cancel-by-drop.
                        clear_liveness_slot(&liveness_slot);

                        if on_event
                            .send(McpClientEvent::TransportClosed {
                                server: server_name_for_task.clone(),
                                // Bind the event to THIS client instance so the dispatcher can skip evicting a replacement registered.
                                client_id: client.client_id(),
                            })
                            .is_err()
                        {
                            tracing::debug!(
                                server = %server_name_for_task,
                                "dispatcher receiver dropped; liveness watcher exiting silently",
                            );
                        }
                        return;
                    }
                    LivenessCheck::Transient => {
                        // State moved out of `Ready` (re-handshake started, or the transport was reset externally).
                        tracing::debug!(
                            server = %server_name_for_task,
                            "transport liveness watcher: state drifted out of Ready, exiting silently",
                        );
                        clear_liveness_slot(&liveness_slot);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::McpClient;
    use tokio::sync::mpsc::unbounded_channel;

    /// Stub client whose `liveness_check()` returns
    /// `LivenessCheck::Transient`.
    fn make_stub_client() -> Arc<McpClient> {
        Arc::new(McpClient::stub("test-server"))
    }

    /// Contract: a watcher whose owning client never reaches `Ready` with a closed transport (here the stub is `Empty`) exits **silently**.
    /// No `TransportClosed` event is emitted, and the slot is cleared.
    /// The watcher must not emit a false positive on non-`Ready` states.
    #[tokio::test(start_paused = true)]
    async fn poller_silent_exit_on_non_ready_state() {
        let (tx, mut rx) = unbounded_channel::<McpClientEvent>();
        let slot: SharedLivenessSlot = Arc::new(parking_lot::Mutex::new(None));
        let client = make_stub_client();
        let handle = spawn_transport_liveness(
            "test-server".to_string(),
            client,
            Duration::from_millis(500),
            tx,
            Arc::clone(&slot),
        );
        // Pre-populate the slot so we can assert the watcher clears it on exit
        *slot.lock() = Some(handle);

        // First `interval.tick()` fires immediately under paused time The watcher classifies `Empty` as `Transient`.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        assert!(
            rx.try_recv().is_err(),
            "non-Ready states must not produce TransportClosed",
        );

        // Slot is cleared so re-arming wouldn't be blocked.
        assert!(
            slot.lock().is_none(),
            "watcher must clear its own slot on exit",
        );
    }
}
