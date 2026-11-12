//! Mid-turn interrupt of a foreground call: polls for a pending human interjection or parent interject (`wait_for_wait_interrupt`) and names its cause.

use std::time::Duration;

use xai_interjection_core::InterjectionBuffer;

use crate::session::acp_session::parent_interject::ParentInterjectSignal;

/// Why a foreground call stepped aside before it produced its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaitInterruptCause {
    HumanInterjection,
    ParentInterject,
}

const WAIT_INTERRUPT_POLL: Duration = Duration::from_millis(50);

/// Human first, then parent.
pub(super) async fn wait_for_wait_interrupt(
    human: &InterjectionBuffer<agent_client_protocol::ImageContent>,
    parent: &ParentInterjectSignal,
) -> WaitInterruptCause {
    loop {
        if !human.is_empty() {
            return WaitInterruptCause::HumanInterjection;
        }
        if parent.is_pending() {
            return WaitInterruptCause::ParentInterject;
        }
        tokio::time::sleep(WAIT_INTERRUPT_POLL).await;
    }
}

#[cfg(test)]
#[path = "wait_interrupt_tests.rs"]
mod wait_interrupt_tests;
