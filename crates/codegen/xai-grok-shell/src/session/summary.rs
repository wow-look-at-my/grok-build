//! Session summary (title) generation.
//!
//! Checks whether a summary exists, generates one via the LLM, persists it, syncs to remote, updates the session registry, and notifies the client.
//! The persistence actor just calls [`SummaryGenerator::update`]; all state transitions are internal.

use crate::extensions::notification::{SessionNotification, SessionUpdate as XaiSessionUpdate};
use crate::session::helpers::session_summary::generate_session_summary;
use crate::session::info::Info;
use crate::session::persistence::PersistenceMsg;
use agent_client_protocol as acp;
use tokio::sync::mpsc;
use xai_acp_lib::AcpAgentGatewaySender as GatewaySender;

enum State {
    /// No summary generated yet. The next [`SummaryGenerator::update`] call will attempt one.
    Idle,
    /// Summary generation has been attempted (spawned or already on disk). No further work needed.
    Done,
}

pub(crate) struct SummaryConfig {
    /// The title model's config.
    pub(crate) sampler_config: Option<xai_grok_sampler::SamplerConfig>,
    pub(crate) model: String,
    /// Channel back to the persistence actor for sequential storage writes.
    /// Weak: a strong sender here would keep the actor's own channel and task alive.
    pub(crate) persistence_tx: mpsc::WeakUnboundedSender<PersistenceMsg>,
}

/// Created once per persistence actor. The only public method is [`update`], which is called from the `ContentChunk` handler.
pub(crate) struct SummaryGenerator {
    state: State,
    config: SummaryConfig,
    /// The session ledger the title call bills to. `None` until the session sends it.
    ledger: Option<xai_chat_state::ChatStateHandle>,
}

impl SummaryGenerator {
    pub(crate) fn new(config: SummaryConfig) -> Self {
        Self {
            state: State::Idle,
            config,
            ledger: None,
        }
    }

    pub(crate) fn set_ledger(&mut self, ledger: xai_chat_state::ChatStateHandle) {
        self.ledger = Some(ledger);
    }

    /// The title client, billed to the session ledger. With no ledger yet, the build fails loud and the title comes from the prompt.
    fn title_client(&self) -> Option<crate::sampling::Client> {
        let mut cfg = self.config.sampler_config.clone()?;
        cfg.usage_sink = self
            .ledger
            .as_ref()
            .map(crate::session::billed_client::session_usage_sink);
        crate::session::billed_client::billed_sampling_client(cfg)
            .inspect_err(|error| {
                tracing::error!(%error, "no session title client; the title comes from the prompt");
            })
            .ok()
    }

    /// Generate a session summary from the first content chunk.
    /// Idle: checks disk for an existing summary, spawns a background task for LLM title generation so the persistence actor is not blocked.
    /// Empty content is skipped (stays Idle) so the next chunk can retry.
    pub(crate) fn update(&mut self, content: String) {
        match self.state {
            State::Done => {}
            State::Idle => {
                // No text to generate a title from (e.g. image-only message).
                // Stay Idle so the next ContentChunk with actual text retries.
                if content.trim().is_empty() {
                    return;
                }

                // Transition to Done so subsequent ContentChunk messages don't spawn duplicate title generation tasks
                self.state = State::Done;

                let sampling_client = self.title_client();
                let model = self.config.model.clone();
                let persistence_tx = self.config.persistence_tx.clone();

                // Spawn title generation as a background task so the
                // persistence actor can continue processing messages
                // (updates, flushes) without waiting for the LLM call.
                xai_spawn::spawn(xai_grok_tools::util::detached::fire_and_forget(
                    "session title generation",
                    async move {
                        let mut title = match sampling_client {
                            Some(client) => {
                                generate_session_summary(content.clone(), client, &model).await
                            }
                            None => String::new(),
                        };
                        if title.trim().is_empty() {
                            title =
                            crate::session::helpers::session_summary::title_fallback_from_user_text(
                                &content,
                            );
                        }

                        // Route the result through the persistence channel.
                        match persistence_tx.upgrade() {
                            Some(tx) => {
                                let _ = tx.send(PersistenceMsg::GeneratedTitle(title));
                            }
                            None => {
                                tracing::debug!("session closed before its title was generated")
                            }
                        }
                    },
                ));
            }
        }
    }

    /// Mark as Done (e.g. when disk already has a summary during load).
    pub(crate) fn mark_done(&mut self) {
        self.state = State::Done;
    }

    /// Inverse of [`mark_done`]: `/rename --auto` calls this so the next content chunk regenerates a title through the normal if-absent path.
    pub(crate) fn reset(&mut self) {
        self.state = State::Idle;
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTitleNotificationKind {
    Automatic,
    Manual,
    Reset,
}

/// Builds the canonical extension and ACP title notifications with matching manual-pin metadata.
pub fn session_title_notifications(
    session_id: acp::SessionId,
    title: &str,
    kind: SessionTitleNotificationKind,
) -> (SessionNotification, acp::SessionNotification) {
    let meta = match kind {
        SessionTitleNotificationKind::Automatic => None,
        SessionTitleNotificationKind::Manual => {
            Some(crate::extensions::notification::title_is_manual_meta())
        }
        SessionTitleNotificationKind::Reset => {
            Some(crate::extensions::notification::title_is_unpinned_meta())
        }
    };
    let summary = SessionNotification {
        session_id: session_id.clone(),
        update: XaiSessionUpdate::SessionSummaryGenerated {
            session_summary: title.to_owned(),
        },
        meta: meta.clone(),
    };
    let info = session_info_update(session_id, title)
        .meta(meta.and_then(|meta| meta.as_object().cloned()));
    (summary, info)
}

/// Notify the client that a session summary is available.
pub(crate) fn notify_client(gateway: &Option<GatewaySender>, info: &Info, title: &str) {
    let Some(gateway) = gateway else {
        return;
    };

    let (notification, info_update) = session_title_notifications(
        info.id.clone(),
        title,
        SessionTitleNotificationKind::Automatic,
    );
    if let Ok(params) = serde_json::value::to_raw_value(&notification) {
        gateway.forward_fire_and_forget(acp::ExtNotification::new(
            "example.invalid/session_notification",
            params.into(),
        ));
    }

    gateway.forward_fire_and_forget(info_update);
}

pub(crate) fn session_info_update(
    session_id: acp::SessionId,
    title: &str,
) -> acp::SessionNotification {
    // `updatedAt` is omitted, not refreshed: renaming is not activity, and `session/list` sorts on `last_active_at`, which a title write never moves
    acp::SessionNotification::new(
        session_id,
        acp::SessionUpdate::SessionInfoUpdate(
            acp::SessionInfoUpdate::new().title(title.to_owned()),
        ),
    )
}

/// Unpin fan-out: no title (avoid blanking list-driven clients) plus `_meta.example.invalid/titleIsManual: false`.
pub(crate) fn session_info_update_unpinned(session_id: acp::SessionId) -> acp::SessionNotification {
    acp::SessionNotification::new(
        session_id,
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new()),
    )
    .meta(
        crate::extensions::notification::title_is_unpinned_meta()
            .as_object()
            .cloned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_title_notifications_stamp_the_pin_meta_on_both_forms() {
        let (summary, info) = session_title_notifications(
            acp::SessionId::new("s"),
            "a &amp; b",
            SessionTitleNotificationKind::Manual,
        );
        let key = crate::extensions::notification::TITLE_IS_MANUAL_META_KEY;

        assert_eq!(
            summary.meta.as_ref().and_then(|m| m.get(key)),
            Some(&serde_json::Value::Bool(true))
        );
        let info = serde_json::to_value(&info).unwrap();
        assert_eq!(
            info.get("_meta").and_then(|m| m.get(key)),
            Some(&serde_json::Value::Bool(true))
        );
        let title = info
            .pointer("/update/title")
            .or_else(|| info.pointer("/update/sessionInfoUpdate/title"))
            .cloned();
        assert_eq!(title, Some(serde_json::json!("a &amp; b")), "{info}");
    }

    #[test]
    fn session_info_update_unpinned_stamps_false_meta_without_title() {
        let n = session_info_update_unpinned(acp::SessionId::new("s"));
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(
            v.get("_meta")
                .and_then(|m| m.get(crate::extensions::notification::TITLE_IS_MANUAL_META_KEY)),
            Some(&serde_json::Value::Bool(false))
        );
        let title = v
            .pointer("/update/title")
            .or_else(|| v.pointer("/update/sessionInfoUpdate/title"));
        assert!(
            title.is_none(),
            "unpin SessionInfoUpdate must omit title: {v}"
        );
    }

    #[test]
    fn auto_session_info_update_omits_manual_meta() {
        let n = session_info_update(acp::SessionId::new("s"), "Auto");
        let v = serde_json::to_value(&n).unwrap();
        assert!(
            v.get("_meta")
                .and_then(|m| m.get(crate::extensions::notification::TITLE_IS_MANUAL_META_KEY))
                .is_none(),
            "auto-title fan-out must not stamp titleIsManual: {v}"
        );
    }

    #[test]
    fn reset_returns_generator_to_idle() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // The discard port answers nothing.
        let mut generator = SummaryGenerator::new(SummaryConfig {
            sampler_config: Some(xai_grok_sampler::SamplerConfig {
                base_url: "http://127.0.0.1:9/v1".to_owned(),
                ..xai_grok_sampler::SamplerConfig::default()
            }),
            model: String::new(),
            persistence_tx: tx.downgrade(),
        });
        assert!(generator.is_idle());
        generator.mark_done();
        assert!(!generator.is_idle());
        generator.reset();
        assert!(generator.is_idle());
        generator.reset();
        assert!(generator.is_idle());
    }
}
