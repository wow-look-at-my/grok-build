//! An active goal waits on background work by ending the turn. The goal checker and the continuation hold until that work finishes.

use super::goal_hold::{GoalCheckHold, LiveBackgroundWork, count_live_tasks};
use super::support::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentEvent, SubagentInspection, SubagentSnapshot, SubagentSnapshotStatus,
};

fn running_inspection(id: &str) -> SubagentInspection {
    SubagentInspection {
        snapshot: SubagentSnapshot {
            subagent_id: id.to_string(),
            description: "task".to_string(),
            subagent_type: "general-purpose".to_string(),
            status: SubagentSnapshotStatus::Running {
                turn_count: 1,
                tool_call_count: 0,
                tokens_used: 0,
                context_window_tokens: 0,
                context_usage_pct: 0,
                tools_used: Vec::new(),
                error_count: 0,
            },
            started_at_epoch_ms: 0,
            duration_ms: 50,
            persona: None,
        },
        parent_session_id: String::new(),
        child_session_id: format!("child-{id}"),
        fork_parent_prompt_id: None,
        resumed_from: None,
    }
}

/// Answers `ListRunning` with as many running subagents as `running` holds.
fn spawn_running_subagents(actor: &mut SessionActor, running: Arc<AtomicUsize>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    actor.tool_context.subagent_event_tx = Some(tx);
    xai_spawn::spawn_local(async move {
        while let Some(event) = rx.recv().await {
            if let SubagentEvent::ListRunning(request) = event {
                let list = (0..running.load(Ordering::Relaxed))
                    .map(|i| running_inspection(&format!("sa-{i}")))
                    .collect();
                let _ = request.respond_to.send(list);
            }
        }
    });
}

/// An actor with an active goal and a coordinator that reports `running` live subagents.
async fn goal_actor(running: Arc<AtomicUsize>) -> Arc<SessionActor> {
    let (gateway_tx, mut gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    xai_spawn::spawn_local(async move {
        while let Some(message) = gateway_rx.recv().await {
            if let xai_acp_lib::AcpClientMessage::SessionNotification(args) = message {
                let _ = args.response_tx.send(Ok(()));
            }
        }
    });
    let (persistence_tx, mut persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    xai_spawn::spawn_local(async move {
        while let Some(message) = persistence_rx.recv().await {
            if let PersistenceMsg::FlushAndAck { respond_to } = message {
                let _ = respond_to.send(Ok(()));
            }
        }
    });
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    set_goal_harness_for_tests(&actor);
    actor.goal_tracker.lock().create_goal(
        "goal".into(),
        "objective".into(),
        None,
        0,
        "2026-01-01T00:00:00Z".into(),
        None,
    );
    spawn_running_subagents(&mut actor, running);
    Arc::new(actor)
}

fn terminal_task(
    id: &str,
    completed: bool,
    owner: Option<&str>,
) -> xai_grok_tools::computer::types::TaskSnapshot {
    xai_grok_tools::computer::types::TaskSnapshot {
        task_id: id.into(),
        command: "sleep 100".into(),
        display_command: None,
        cwd: String::new(),
        start_time: std::time::SystemTime::now(),
        end_time: completed.then(std::time::SystemTime::now),
        output: String::new(),
        output_file: std::path::PathBuf::new(),
        truncated: false,
        exit_code: completed.then_some(0),
        signal: None,
        completed,
        kind: Default::default(),
        block_waited: false,
        explicitly_killed: false,
        kill_result_delivered: false,
        owner_session_id: owner.map(str::to_owned),
        description: None,
        is_backgrounded: true,
        output_total_bytes: 0,
    }
}

fn queued_ids(state: &State) -> Vec<String> {
    state
        .pending_inputs
        .iter()
        .map(|item| item.prompt_id.clone())
        .collect()
}

#[test]
fn live_task_count_takes_only_unfinished_tasks_of_this_session() {
    let tasks = [
        terminal_task("mine-running", false, Some("me")),
        terminal_task("mine-done", true, Some("me")),
        terminal_task("other-running", false, Some("other")),
        terminal_task("no-owner-running", false, None),
    ];
    let none = std::collections::HashSet::new();
    assert_eq!(count_live_tasks(&tasks, "me", &none), 2);
    assert_eq!(count_live_tasks(&tasks, "other", &none), 2);
    assert_eq!(count_live_tasks(&[], "me", &none), 0);
}

#[test]
fn a_task_left_by_a_harness_subagent_is_not_live_work() {
    let tasks = [
        terminal_task("verifier-server", false, Some("me")),
        terminal_task("model-build", false, Some("me")),
    ];
    let harness = std::collections::HashSet::from(["verifier-server".to_string()]);
    assert_eq!(count_live_tasks(&tasks, "me", &harness), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_running_subagent_holds_the_goal_checker() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let running = Arc::new(AtomicUsize::new(1));
            let actor = goal_actor(Arc::clone(&running)).await;
            assert_eq!(
                actor.goal_check_hold().await,
                Some(GoalCheckHold::BackgroundWork(LiveBackgroundWork {
                    tasks: 0,
                    subagents: 1,
                    workflows: 0,
                })),
                "a live subagent must hold the goal checker"
            );
            running.store(0, Ordering::Relaxed);
            assert_eq!(
                actor.goal_check_hold().await,
                None,
                "nothing in the background and nothing queued: the goal checker may run"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_queued_wake_or_notification_holds_the_goal_checker() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            let wake = |prompt_id: &str| {
                input_with_origin_rx(
                    prompt_id,
                    crate::session::PromptOrigin::from_prompt_id(prompt_id),
                )
                .0
            };

            actor
                .state
                .lock()
                .await
                .pending_inputs
                .push_back(wake("task-completed-bg-1"));
            assert_eq!(
                actor.goal_check_hold().await,
                Some(GoalCheckHold::WakePending),
                "a queued completion wake runs before the goal checker"
            );
            actor.state.lock().await.pending_inputs.clear();

            actor
                .state
                .lock()
                .await
                .pending_notifications
                .push(PendingNotification {
                    prompt_id: "bash-completed-bg-2".into(),
                    prompt_blocks: vec![],
                    priority: NotificationPriority::Later,
                    source: NotificationSource::BashTaskCompleted {
                        task_id: "bg-2".into(),
                    },
                });
            assert_eq!(
                actor.goal_check_hold().await,
                Some(GoalCheckHold::WakePending),
                "a pending notification runs before the goal checker"
            );
            actor.state.lock().await.pending_notifications.clear();

            {
                let mut state = actor.state.lock().await;
                state.pending_inputs.push_back(
                    input_with_origin_rx(
                        "goal-summary-1",
                        crate::session::PromptOrigin::GoalSummary,
                    )
                    .0,
                );
                state.pending_inputs.push_back(user_item("p1", "alice"));
            }
            assert_eq!(
                actor.goal_check_hold().await,
                None,
                "a queued goal continuation or user row is not a wake"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn turn_end_queues_no_continuation_while_background_work_runs() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let running = Arc::new(AtomicUsize::new(1));
            let actor = goal_actor(Arc::clone(&running)).await;

            actor.handle_turn_end(true, false).await;
            assert!(
                queued_ids(&*actor.state.lock().await).is_empty(),
                "the session must stay idle while the subagent runs"
            );
            assert!(
                actor.goal_background.waiting.load(Ordering::Relaxed),
                "the wait is recorded so the user is told one time"
            );

            running.store(0, Ordering::Relaxed);
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            SessionActor::resume_goal_after_background_settled(actor.clone(), completion_tx).await;
            let mut state = actor.state.try_lock().expect("no await since promote");
            assert!(
                state
                    .running_prompt_id()
                    .is_some_and(|id| id.starts_with("goal-summary-")),
                "the goal continues when the last background work ends: {:?}",
                queued_ids(&state)
            );
            assert!(
                !actor.goal_background.waiting.load(Ordering::Relaxed),
                "the wait ends with the continuation"
            );
            if let Some(task) = state.running_task.take() {
                task.handle.abort();
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn turn_end_continues_the_goal_when_nothing_runs_in_the_background() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor.handle_turn_end(true, false).await;
            let ids = queued_ids(&*actor.state.lock().await);
            assert!(
                matches!(ids.as_slice(), [id] if id.starts_with("goal-summary-")),
                "an idle goal with no background work queues one continuation: {ids:?}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settled_background_work_starts_nothing_while_a_turn_runs() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            {
                let mut state = actor.state.lock().await;
                state
                    .pending_inputs
                    .push_back(user_item("running", "alice"));
                state.running_task = Some(running_task_stub("running"));
            }
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            SessionActor::resume_goal_after_background_settled(actor.clone(), completion_tx).await;
            let mut state = actor.state.lock().await;
            assert_eq!(
                queued_ids(&state),
                ["running"],
                "the running turn decides at its own end"
            );
            if let Some(task) = state.running_task.take() {
                task.handle.abort();
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settled_background_work_does_nothing_without_an_active_goal() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor.goal_tracker.lock().complete();
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            SessionActor::resume_goal_after_background_settled(actor.clone(), completion_tx).await;
            let state = actor.state.lock().await;
            assert!(queued_ids(&state).is_empty());
            assert!(state.running_task.is_none());
        })
        .await;
}

/// The goal model waits for the task it started, so its completion notification must start a turn.
#[tokio::test(flavor = "current_thread")]
async fn an_active_goal_surfaces_the_notification_of_its_own_task() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor
                .goal_turn_task_ids
                .lock()
                .insert("bg-goal".to_string());
            actor
                .state
                .lock()
                .await
                .pending_notifications
                .push(PendingNotification {
                    prompt_id: "bash-completed-bg-goal".into(),
                    prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                        "Background task bg-goal completed.",
                    ))],
                    priority: NotificationPriority::Later,
                    source: NotificationSource::BashTaskCompleted {
                        task_id: "bg-goal".into(),
                    },
                });
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            actor.clone().maybe_drain_notifications(completion_tx).await;
            let mut state = actor.state.try_lock().expect("no await since promote");
            assert!(
                state
                    .running_prompt_id()
                    .is_some_and(|id| id.starts_with("notifications-")),
                "the notification must wake the goal session: {:?}",
                queued_ids(&state)
            );
            if let Some(task) = state.running_task.take() {
                task.handle.abort();
            }
        })
        .await;
}

/// The goal model did not start a task that a verifier left running. Its completion must not start a turn.
#[tokio::test(flavor = "current_thread")]
async fn an_active_goal_drops_the_notification_of_a_harness_task() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor.record_reparented_goal_turn_task_ids(["verifier-server".to_string()]);
            actor
                .state
                .lock()
                .await
                .pending_notifications
                .push(PendingNotification {
                    prompt_id: "bash-completed-verifier-server".into(),
                    prompt_blocks: vec![],
                    priority: NotificationPriority::Later,
                    source: NotificationSource::BashTaskCompleted {
                        task_id: "verifier-server".into(),
                    },
                });
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            actor.clone().maybe_drain_notifications(completion_tx).await;
            let state = actor.state.lock().await;
            assert!(state.pending_notifications.is_empty());
            assert!(state.running_task.is_none());
            assert!(queued_ids(&state).is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_harness_task_completion_is_refused_as_a_wake() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor.record_reparented_goal_turn_task_ids(["verifier-server".to_string()]);
            let admission = |task_id: &str| {
                let (respond_to, admitted) = oneshot::channel();
                (
                    crate::session::commands::TaskWakeAdmission {
                        respond_to,
                        fallback: crate::session::commands::TaskWakeFallback {
                            prompt_id: format!("bash-completed-{task_id}"),
                            prompt_blocks: vec![],
                            source: NotificationSource::BashTaskCompleted {
                                task_id: task_id.to_string(),
                            },
                        },
                    },
                    admitted,
                )
            };

            let (harness, harness_admitted) = admission("verifier-server");
            let origin =
                crate::session::PromptOrigin::from_prompt_id("task-completed-verifier-server");
            assert!(
                actor
                    .admit_task_completion_wake(&origin, harness)
                    .await
                    .is_none()
            );
            assert_eq!(harness_admitted.await, Ok(false));
            assert!(
                actor.state.lock().await.pending_notifications.is_empty(),
                "a refused harness task leaves no fallback notification"
            );

            let (own, own_admitted) = admission("model-build");
            let origin = crate::session::PromptOrigin::from_prompt_id("task-completed-model-build");
            assert!(
                actor
                    .admit_task_completion_wake(&origin, own)
                    .await
                    .is_some()
            );
            assert_eq!(own_admitted.await, Ok(true));
        })
        .await;
}

/// A completed goal keeps the blanket drop: a late completion leaves the session idle.
#[tokio::test(flavor = "current_thread")]
async fn a_completed_goal_still_drops_late_notifications() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let actor = goal_actor(Arc::new(AtomicUsize::new(0))).await;
            actor.goal_tracker.lock().complete();
            actor
                .state
                .lock()
                .await
                .pending_notifications
                .push(PendingNotification {
                    prompt_id: "bash-completed-late".into(),
                    prompt_blocks: vec![],
                    priority: NotificationPriority::Later,
                    source: NotificationSource::BashTaskCompleted {
                        task_id: "late".into(),
                    },
                });
            let (completion_tx, _completion_rx) = tokio::sync::mpsc::unbounded_channel();
            actor.clone().maybe_drain_notifications(completion_tx).await;
            let state = actor.state.lock().await;
            assert!(state.pending_notifications.is_empty());
            assert!(state.running_task.is_none());
            assert!(queued_ids(&state).is_empty());
        })
        .await;
}
