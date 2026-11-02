use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{mpsc, watch};

use xai_acp_lib::AcpAgentGatewaySender as GatewaySender;

use crate::session::persistence::PersistenceMsg;

/// Session-scoped client gates shared by [`NotificationSender`] and [`crate::session::SessionHandle`].
#[derive(Clone)]
pub struct SessionClientCaps {
    pub status_line: Arc<AtomicBool>,
    pub user_message_echo: Arc<AtomicBool>,
}

impl SessionClientCaps {
    pub fn new(status_line: bool, user_message_echo: bool) -> Self {
        Self {
            status_line: Arc::new(AtomicBool::new(status_line)),
            user_message_echo: Arc::new(AtomicBool::new(user_message_echo)),
        }
    }
}

pub struct NotificationSender {
    pub gateway: GatewaySender,
    /// When false, notifications are persisted but NOT forwarded to the client.
    /// Opened by `MvpAgent::load_session` when the client explicitly loads the session.
    pub gateway_enabled: Arc<AtomicBool>,
    pub persistence_tx: mpsc::UnboundedSender<PersistenceMsg>,
    pub disk_full: watch::Receiver<bool>,
    pub client_caps: SessionClientCaps,
}

impl NotificationSender {
    pub fn is_disk_full(&self) -> bool {
        *self.disk_full.borrow()
    }

    pub fn live_user_message_echo(&self) -> bool {
        self.client_caps
            .user_message_echo
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn for_tests(
        gateway: GatewaySender,
        persistence_tx: mpsc::UnboundedSender<PersistenceMsg>,
    ) -> Self {
        Self {
            gateway,
            gateway_enabled: Arc::new(AtomicBool::new(true)),
            persistence_tx,
            disk_full: idle_disk_full_rx(),
            client_caps: SessionClientCaps::new(false, true),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub fn idle_disk_full_rx() -> watch::Receiver<bool> {
    watch::channel(false).1
}
