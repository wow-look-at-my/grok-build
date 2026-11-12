//! Holds the goal checker while the session waits on background work.

use super::*;

/// What the goal loop tracks about background work.
#[derive(Default)]
pub(crate) struct GoalBackground {
    /// True while an active goal waits on background work and the user was told so.
    pub(crate) waiting: std::sync::atomic::AtomicBool,
    /// Tasks that a harness subagent (planner, verifier) left running when it ended.
    pub(crate) harness_task_ids: parking_lot::Mutex<std::collections::HashSet<String>>,
}

/// The background work of this session that has not finished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LiveBackgroundWork {
    /// Background commands and monitors.
    pub(super) tasks: usize,
    pub(super) subagents: usize,
    pub(super) workflows: usize,
}

impl LiveBackgroundWork {
    pub(super) fn is_empty(&self) -> bool {
        self.tasks + self.subagents + self.workflows == 0
    }

    /// One line for the user, for example `2 background tasks and 1 subagent`.
    pub(super) fn describe(&self) -> String {
        let part = |count: usize, noun: &str| {
            (count > 0).then(|| format!("{count} {noun}{}", if count == 1 { "" } else { "s" }))
        };
        let parts: Vec<String> = [
            part(self.tasks, "background task"),
            part(self.subagents, "subagent"),
            part(self.workflows, "workflow"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match parts.as_slice() {
            [] => "no background work".to_string(),
            [one] => one.clone(),
            [head @ .., last] => format!("{} and {last}", head.join(", ")),
        }
    }
}

/// Why a finished round does not go to the goal checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GoalCheckHold {
    /// A completion wake or a notification is queued. It runs as the next turn.
    WakePending,
    /// Background work still runs. Its completion wakes the session.
    BackgroundWork(LiveBackgroundWork),
}

/// The pending inputs that hold the goal checker: every queued synthetic input but a goal continuation.
/// A queued user row is not one. The turn loop yields to it on its own.
pub(super) fn is_goal_hold_input(origin: &crate::session::PromptOrigin) -> bool {
    origin.is_synthetic()
        && !matches!(
            origin,
            crate::session::PromptOrigin::GoalSummary
                | crate::session::PromptOrigin::GoalClassifierNudge
        )
}

/// Counts the unfinished terminal tasks of `session_id`. The terminal is shared, so another session's task does not count.
/// A task with no recorded owner counts. A task in `harness_task_ids` does not.
pub(super) fn count_live_tasks(
    tasks: &[xai_grok_tools::computer::types::TaskSnapshot],
    session_id: &str,
    harness_task_ids: &std::collections::HashSet<String>,
) -> usize {
    tasks
        .iter()
        .filter(|task| {
            task.is_outstanding()
                && !harness_task_ids.contains(&task.task_id)
                && task
                    .owner_session_id
                    .as_deref()
                    .is_none_or(|owner| owner == session_id)
        })
        .count()
}

impl SessionActor {
    /// Counts the background commands, monitors, subagents and workflows of this session that still run.
    pub(super) async fn live_background_work(&self) -> LiveBackgroundWork {
        let session_id = self.session_info.id.0.as_ref();
        let listed = self.tool_bridge_handle().list_background_tasks().await;
        let tasks = count_live_tasks(
            &listed,
            session_id,
            &self.goal_background.harness_task_ids.lock(),
        );
        let subagents = match &self.tool_context.subagent_event_tx {
            Some(tx) => {
                xai_grok_tools::implementations::grok_build::task::backend::ChannelBackend::new(
                    tx.clone(),
                )
                .list_running(session_id)
                .await
                .len()
            }
            None => 0,
        };
        let workflows = self
            .workflow_tracker()
            .await
            .lock()
            .list()
            .into_iter()
            .filter(|run| {
                run.status == crate::session::workflow::tracker::WorkflowRunStatus::Active
            })
            .count();
        LiveBackgroundWork {
            tasks,
            subagents,
            workflows,
        }
    }

    /// True when a completion wake or a notification waits for its own turn.
    pub(super) async fn has_pending_wake(&self) -> bool {
        let mut state = self.state.lock().await;
        self.sweep_monitor_buffer_into_pending(&mut state, "monitor-goal-hold-drain");
        if !state.pending_notifications.is_empty() {
            return true;
        }
        let running = state.running_prompt_id();
        state
            .pending_inputs
            .iter()
            .filter(|item| running != Some(item.prompt_id.as_str()))
            .any(|item| is_goal_hold_input(item.input_origin.as_prompt_origin()))
    }

    /// The reason the goal checker must not run now, or `None` when the session has nothing left to wait for.
    pub(super) async fn goal_check_hold(&self) -> Option<GoalCheckHold> {
        if self.has_pending_wake().await {
            return Some(GoalCheckHold::WakePending);
        }
        let live = self.live_background_work().await;
        (!live.is_empty()).then_some(GoalCheckHold::BackgroundWork(live))
    }

    /// Continues an active goal from an idle session, or leaves the session idle while a hold is in effect.
    /// A held session queues no continuation. The completion wake starts the next turn.
    pub(super) async fn resume_goal_or_wait(&self) {
        match self.goal_check_hold().await {
            Some(GoalCheckHold::WakePending) => {
                tracing::info!("goal idle: a wake is queued, no continuation");
            }
            Some(GoalCheckHold::BackgroundWork(live)) => {
                self.announce_goal_wait(live).await;
            }
            None => {
                self.goal_background
                    .waiting
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                self.maybe_queue_goal_continuation().await;
            }
        }
    }

    /// Tells the user one time per wait that the goal waits on background work.
    async fn announce_goal_wait(&self, live: LiveBackgroundWork) {
        xai_grok_telemetry::unified_log::info(
            "shell.goal.waiting_on_background",
            Some(self.session_info.id.0.as_ref()),
            Some(serde_json::json!({
                "tasks": live.tasks,
                "subagents": live.subagents,
                "workflows": live.workflows,
            })),
        );
        if self
            .goal_background
            .waiting
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        self.send_slash_command_output(&format!(
            "Goal is waiting on {}. It continues when that work finishes.",
            live.describe()
        ))
        .await;
    }

    /// Runs when background work ends, or a queued wake is dropped, while no turn runs.
    /// A wake that was admitted is in the queue by now, so this does nothing then.
    pub(super) async fn resume_goal_after_background_settled(
        self: Arc<Self>,
        completion_tx: mpsc::UnboundedSender<super::turn_task::TurnCompletionMsg>,
    ) {
        if !self.goal_loop_active() {
            return;
        }
        {
            let state = self.state.lock().await;
            if !is_session_idle_for_injection(&state) {
                return;
            }
        }
        self.resume_goal_or_wait().await;
        SessionActor::maybe_start_running_task(self, completion_tx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::PromptOrigin;

    #[test]
    fn describe_names_each_kind_of_live_work() {
        let none = LiveBackgroundWork::default();
        assert!(none.is_empty());
        let one = LiveBackgroundWork {
            tasks: 1,
            ..Default::default()
        };
        assert!(!one.is_empty());
        assert_eq!(one.describe(), "1 background task");
        let all = LiveBackgroundWork {
            tasks: 2,
            subagents: 1,
            workflows: 3,
        };
        assert_eq!(
            all.describe(),
            "2 background tasks, 1 subagent and 3 workflows"
        );
    }

    #[test]
    fn completion_wakes_hold_the_goal_checker_and_goal_inputs_do_not() {
        for held in [
            "task-completed-t1",
            "subagent-completed-s1",
            "workflow-completed-wf_1-2",
            "todo-added-cap-1",
        ] {
            assert!(
                is_goal_hold_input(&PromptOrigin::from_prompt_id(held)),
                "{held} must hold the goal checker"
            );
        }
        assert!(is_goal_hold_input(&PromptOrigin::NotificationDrain));
        for free in [
            PromptOrigin::User,
            PromptOrigin::GoalSummary,
            PromptOrigin::GoalClassifierNudge,
        ] {
            assert!(
                !is_goal_hold_input(&free),
                "{free:?} must not hold the goal checker"
            );
        }
    }
}
