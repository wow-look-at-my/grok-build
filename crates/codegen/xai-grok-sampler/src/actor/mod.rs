//! The actor task itself is single-threaded: it processes one command at a time.
//! The actual streaming work happens in `tokio::spawn` per-request tasks, so multiple requests can be in flight concurrently.

pub(crate) mod request_metadata;
pub(crate) mod request_task;
pub(crate) mod state;

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_util::FutureExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::commands::SamplerCommand;
use crate::config::{RetryPolicy, SamplerConfig};
use crate::events::SamplingEvent;
use crate::handle::SamplerHandle;
use crate::request_slots;
use state::{ActiveRequest, ActorState};

use crate::types::RequestId;

/// Construct via [`SamplerActor::spawn`]; the returned [`SamplerHandle`] is the only supported way to interact with it.
pub struct SamplerActor {
    cmd_rx: mpsc::UnboundedReceiver<SamplerCommand>,
    event_tx: mpsc::UnboundedSender<SamplingEvent>,
    state: ActorState,
    /// The actor's run loop selects on `cmd_rx.recv()` and `tasks.join_next()`.
    /// A finished task returns its `RequestId` so the actor can clean up `active_requests`.
    tasks: JoinSet<RequestId>,
}

/// Spawn one request round onto the actor's set, reporting its id even when
/// the round unwinds. The actor clears `active_requests` from the id a
/// finished round returns, and a `JoinError` carries none. Spawned bare, a
/// round that panicked would leave `IsActive` answering true and
/// `ActiveCount` counting a request that stopped existing. That bare is for
/// the rest of the sampler's life.
fn spawn_tracked_round(
    tasks: &mut JoinSet<RequestId>,
    tracked_id: RequestId,
    task: impl Future<Output = RequestId> + Send + 'static,
) {
    tasks.spawn(async move {
        match AssertUnwindSafe(task).catch_unwind().await {
            Ok(request_id) => request_id,
            Err(panic) => {
                tracing::error!(
                    request_id = tracked_id.as_str(),
                    panic = %panic_payload(&*panic),
                    "sampling request task panicked; its completion is answered as a dropped sender"
                );
                tracked_id
            }
        }
    });
}

/// Text describing what a panic carried, for a log line. Both payloads
/// `panic!` itself produces are a `&'static str` (a literal) and a `String`
/// (a formatted one). Anything else is named as a non-message rather than
/// reported as nothing.
fn panic_payload(panic: &(dyn Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&'static str>() {
        return (*text).to_string();
    }
    if let Some(text) = panic.downcast_ref::<String>() {
        return text.clone();
    }
    "panicked with a payload that is not a message".to_string()
}

#[cfg(test)]
mod round_tests {
    use super::*;

    /// A round that unwinds is reported by the set as a finished id, not as a
    /// join failure. That is what lets the actor drop the request from
    /// `active_requests`: `JoinError` carries no id to remove.
    #[tokio::test]
    async fn a_panicking_round_still_reports_its_id() {
        let mut tasks: JoinSet<RequestId> = JoinSet::new();
        let id = RequestId::random();
        let expected = id.clone();
        spawn_tracked_round(&mut tasks, id, async {
            tokio::task::yield_now().await;
            panic!("the request round died");
        });

        let joined = tokio::time::timeout(std::time::Duration::from_secs(5), tasks.join_next())
            .await
            .expect("the round must finish, one way or another")
            .expect("a round was in the set");
        let reported = joined.expect("a panicked round must report its id, not fail its join");
        assert_eq!(reported, expected);
    }

    /// The success path hands back whatever the round returned.
    #[tokio::test]
    async fn a_completed_round_reports_its_own_id() {
        let mut tasks: JoinSet<RequestId> = JoinSet::new();
        let tracked = RequestId::random();
        let returned = RequestId::random();
        let expected = returned.clone();
        spawn_tracked_round(&mut tasks, tracked, async move {
            tokio::task::yield_now().await;
            returned
        });

        let joined = tokio::time::timeout(std::time::Duration::from_secs(5), tasks.join_next())
            .await
            .expect("the round must finish")
            .expect("a round was in the set");
        assert_eq!(
            joined.expect("the round completed").as_str(),
            expected.as_str()
        );
    }
}

impl SamplerActor {
    /// Spawn the actor on the current tokio runtime and return a handle.
    /// The actor stops when the returned handle (and all its clones) are dropped.
    pub fn spawn(
        config: SamplerConfig,
        retry_policy: RetryPolicy,
        event_tx: mpsc::UnboundedSender<SamplingEvent>,
    ) -> SamplerHandle {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let actor = Self {
            cmd_rx,
            event_tx,
            state: ActorState::new(config, retry_policy),
            tasks: JoinSet::new(),
        };
        // The actor owns the command half every `SamplerHandle` sends to and every in-flight request reports through.
        let run = tokio::spawn(actor.run());
        tokio::spawn(async move {
            if let Err(error) = run.await {
                tracing::error!(
                    task = "sampler actor",
                    error = %error,
                    "sampler actor is no longer serving requests"
                );
            }
        });
        SamplerHandle::new(cmd_tx)
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                // Prefer cleaning up finished tasks before processing new commands, so `active_requests` does not stay stale longer than necessary
                Some(joined) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    match joined {
                        Ok(request_id) => {
                            // Task finished normally; remove from active set unless the user has already cancelled it (Cancel removes it too)
                            self.state.remove(&request_id);
                        }
                        Err(join_err) => {
                            tracing::warn!(
                                error = %join_err,
                                "request task panicked or was aborted"
                            );
                        }
                    }
                }
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.handle_command(cmd),
                        None => break, // all handles dropped
                    }
                }
            }
        }

        // Cancel any still-running tasks before exiting so they don't leak
        // The cancellation token shutdown is best-effort
        for (_, active) in self.state.active_requests.drain() {
            active.cancel_token.cancel();
        }
        self.tasks.shutdown().await;
    }

    fn handle_command(&mut self, cmd: SamplerCommand) {
        match cmd {
            SamplerCommand::Submit {
                request_id,
                request,
                config,
                completion_tx,
                queue_clock,
            } => {
                let cancel_token = CancellationToken::new();
                let active = ActiveRequest {
                    cancel_token: cancel_token.clone(),
                };
                if let Some(prev) = self.state.register(request_id.clone(), active) {
                    // Caller submitted a duplicate id; cancel the previous one so we don't leak its task
                    prev.cancel_token.cancel();
                }
                let effective_config = config
                    .map(|b| *b)
                    .unwrap_or_else(|| self.state.config.clone());
                let event_tx = self.event_tx.clone();
                let retry_policy = self.state.retry_policy.clone();
                let mut request_inner = *request;
                let rejections = self.state.rejections.clone();
                // Skip what this model already rejected: images it cannot
                // read, and tool schema forms it does not accept.
                rejections
                    .images
                    .strip_if_rejected(&effective_config.model, &mut request_inner);
                rejections
                    .tool_schemas
                    .apply(&effective_config.model, &mut request_inner);
                // The id is what the actor needs back to clear `active_requests`.
                let tracked_id = request_id.clone();
                // The round runs on its own task, so the submitter's queue
                // clock is carried over by hand.
                spawn_tracked_round(
                    &mut self.tasks,
                    tracked_id,
                    request_slots::in_queue_clock(
                        queue_clock,
                        request_task::run_request_task(
                            request_id,
                            request_inner,
                            effective_config,
                            retry_policy,
                            event_tx,
                            cancel_token,
                            completion_tx,
                            rejections,
                            Arc::clone(request_slots::global()),
                        ),
                    ),
                );
            }
            SamplerCommand::Cancel { request_id } => {
                self.state.cancel(&request_id);
            }
            SamplerCommand::UpdateConfig { config } => {
                self.state.update_config(*config);
            }
            SamplerCommand::IsActive { request_id, reply } => {
                let _ = reply.send(self.state.active_requests.contains_key(&request_id));
            }
            SamplerCommand::ActiveCount { reply } => {
                let _ = reply.send(self.state.active_requests.len());
            }
        }
    }
}
