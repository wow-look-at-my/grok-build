//! Integration coverage for the planner trigger inside `setup_goal` and the session-load hook `maybe_reconcile_active_goal_without_plan`.
//! Uses the same single-thread runtime and LocalSet pattern as the verification-stage e2e suite.

use super::support::*;
use super::*;
use crate::session::goal_tracker::GoalMode;
use serial_test::serial;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering as SeqOrd};
use tempfile::TempDir;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentCancelTarget, SubagentEvent, SubagentResult, SubagentSpawnRequest,
};

/// Pull the planner's plan-file path from the prompt by its backtick-quoted `.md` token.
/// Rewording the surrounding sentence therefore can't silently break the fake, which would otherwise write nothing and fail far from the cause.
fn plan_path_from_prompt(prompt: &str) -> Option<String> {
    // Backtick-delimited tokens sit at the odd indices of the split.
    prompt
        .split('`')
        .skip(1)
        .step_by(2)
        .find(|token| token.ends_with(".md"))
        .map(str::to_owned)
}

/// Spawn behaviour knobs for the planner-coordinator stub.
enum SpawnBehaviour {
    /// Parse `{PLAN_FILE}` out of the prompt, write `body` there, then respond `Done`.
    WritePlanThenDone { body: &'static [u8] },
    /// Like [`Self::WritePlanThenDone`], but the planner child also reports the
    /// todo list it built for ITSELF with `todo_write` while planning — the
    /// items a real child would have left on its own `State<TodoState>`.
    WritePlanThenDoneWithTodos {
        body: &'static [u8],
        todos: &'static [&'static str],
    },
    WaitForCancelsThenWrite {
        cancels: usize,
        started: tokio::sync::mpsc::UnboundedSender<usize>,
        objectives: StdArc<std::sync::Mutex<Vec<String>>>,
        body: &'static [u8],
    },
    /// Hold the planner open until the test fires `notify`, reporting every
    /// `Interject` addressed to it on `context` as it arrives. An arriving
    /// interjection never releases the planner: only the test decides when
    /// the plan is written.
    WaitForContextThenWrite {
        started: tokio::sync::mpsc::UnboundedSender<usize>,
        objectives: StdArc<std::sync::Mutex<Vec<String>>>,
        context: tokio::sync::mpsc::UnboundedSender<String>,
        notify: StdArc<tokio::sync::Notify>,
        body: &'static [u8],
    },
    /// Reply success but never write the file.
    NoWriteThenDone,
    /// Reply with subagent runtime failure.
    Runtime { message: String, cancelled: bool },
    /// Mimic the real coordinator after a user Stop with `cancel_subagents`:
    /// every Task spawn is rejected as cancelled — `"parent session is
    /// stopped"` — until an `OpenSpawnAdmission` arrives, and accept=True
    /// afterwards (writes `body` to the plan file). Records whether admission
    /// was ever opened, so a test can prove the planner asked to reopen it.
    AdmissionGatedThenWrite {
        opened: StdArc<std::sync::atomic::AtomicBool>,
        body: &'static [u8],
    },
    /// First spawn fails with `Runtime { message, cancelled: false }`; every later spawn behaves as `WritePlanThenDone`.
    RuntimeThenWritePlan {
        message: String,
        body: &'static [u8],
    },
}

/// Captured planner spawn flags (harness-internal `SubagentRequest` fields).
#[derive(Default)]
struct PlannerSpawnCapture {
    fork_context: StdArc<std::sync::Mutex<Vec<bool>>>,
    surface_completion: StdArc<std::sync::Mutex<Vec<bool>>>,
    model: StdArc<std::sync::Mutex<Vec<Option<String>>>>,
    /// Every prompt the coordinator was asked to spawn a planner with, so a
    /// test can assert what the planner's run was actually told to do.
    prompt: StdArc<std::sync::Mutex<Vec<String>>>,
}

/// Write `body` at the plan path the prompt names, then reply the planner's `Done`.
fn plan_written(
    req: &SubagentSpawnRequest,
    plan_path: Option<&str>,
    body: &[u8],
) -> SubagentResult {
    if let Some(p) = plan_path {
        let _ = std::fs::create_dir_all(std::path::Path::new(p).parent().unwrap());
        let _ = std::fs::write(p, body);
    }
    SubagentResult {
        success: true,
        output: StdArc::from("Done"),
        subagent_id: req.id.clone(),
        child_session_id: req.id.clone(),
        ..Default::default()
    }
}

/// Stand up a coordinator that handles exactly the spawn behaviours the planner exercises: each `Spawn` is answered on its `result_tx`.
/// Returns the sender half and a spawn-count counter the test reads at the end.
fn spawn_planner_coordinator(
    behaviour: SpawnBehaviour,
) -> (
    tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    StdArc<AtomicUsize>,
) {
    let (tx, count, _capture) = spawn_planner_coordinator_capturing(behaviour);
    (tx, count)
}

/// Like [`spawn_planner_coordinator`] but also records harness flags (`fork_context`, `surface_completion`) from each spawn.
fn spawn_planner_coordinator_capturing(
    behaviour: SpawnBehaviour,
) -> (
    tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    StdArc<AtomicUsize>,
    PlannerSpawnCapture,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    let spawn_count = StdArc::new(AtomicUsize::new(0));
    let count_task = StdArc::clone(&spawn_count);
    let capture = PlannerSpawnCapture::default();
    let fork_log = StdArc::clone(&capture.fork_context);
    let surface_log = StdArc::clone(&capture.surface_completion);
    let prompt_log = StdArc::clone(&capture.prompt);
    let model_log = StdArc::clone(&capture.model);
    tokio::task::spawn_local(async move {
        while let Some(ev) = rx.recv().await {
            if let SubagentEvent::OpenSpawnAdmission { .. } = ev {
                if let SpawnBehaviour::AdmissionGatedThenWrite { opened, .. } = &behaviour {
                    opened.store(true, SeqOrd::SeqCst);
                }
                continue;
            }
            if let SubagentEvent::Interject { text, .. } = &ev {
                if let SpawnBehaviour::WaitForContextThenWrite { context, .. } = &behaviour {
                    let _ = context.send(text.clone());
                }
                continue;
            }
            if let SubagentEvent::Spawn(req) = ev {
                count_task.fetch_add(1, SeqOrd::SeqCst);
                fork_log.lock().unwrap().push(req.fork_context);
                surface_log.lock().unwrap().push(req.surface_completion);
                prompt_log.lock().unwrap().push(req.prompt.clone());
                model_log
                    .lock()
                    .unwrap()
                    .push(req.runtime_overrides.model.clone());
                let plan_path = plan_path_from_prompt(&req.prompt);
                if let SpawnBehaviour::WaitForContextThenWrite { objectives, .. } = &behaviour {
                    // Record what the planner was actually spawned with, so the
                    // test can prove the objective carries no folded-in steering.
                    objectives.lock().unwrap().push(req.prompt.clone());
                }
                if let SpawnBehaviour::WaitForContextThenWrite { notify, body, .. } = &behaviour {
                    let notify = StdArc::clone(notify);
                    let body = *body;
                    let subagent_id = req.id.clone();
                    let result_tx = req.result_tx;
                    let plan_path = plan_path.clone();
                    let spawn = count_task.load(SeqOrd::SeqCst);
                    let started = match &behaviour {
                        SpawnBehaviour::WaitForContextThenWrite { started, .. } => started,
                        _ => unreachable!(),
                    };
                    let _ = started.send(spawn);
                    tokio::task::spawn_local(async move {
                        notify.notified().await;
                        if let Some(p) = plan_path.as_deref() {
                            let _ =
                                std::fs::create_dir_all(std::path::Path::new(p).parent().unwrap());
                            let _ = std::fs::write(p, body);
                        }
                        let _ = result_tx.send(SubagentResult {
                            success: true,
                            output: StdArc::from("Done"),
                            subagent_id: subagent_id.clone(),
                            child_session_id: subagent_id,
                            ..Default::default()
                        });
                    });
                    continue;
                }
                let result = match &behaviour {
                    // Handled above: waits for the Send Now context, then writes.
                    SpawnBehaviour::WaitForContextThenWrite { .. } => unreachable!(),
                    SpawnBehaviour::WritePlanThenDone { body }
                    | SpawnBehaviour::WritePlanThenDoneWithTodos { body, .. } => {
                        if let Some(p) = plan_path.as_deref() {
                            let _ =
                                std::fs::create_dir_all(std::path::Path::new(p).parent().unwrap());
                            let _ = std::fs::write(p, body);
                        }
                        let todos = match &behaviour {
                            SpawnBehaviour::WritePlanThenDoneWithTodos { todos, .. } => {
                                todos.iter().map(|t| (*t).to_string()).collect()
                            }
                            _ => Vec::new(),
                        };
                        SubagentResult {
                            success: true,
                            output: StdArc::from("Done"),
                            subagent_id: req.id.clone(),
                            child_session_id: req.id.clone(),
                            todos,
                            ..Default::default()
                        }
                    }
                    SpawnBehaviour::WaitForCancelsThenWrite {
                        cancels,
                        started,
                        objectives,
                        body,
                    } => {
                        objectives.lock().unwrap().push(req.prompt.clone());
                        let spawn = count_task.load(SeqOrd::SeqCst);
                        let _ = started.send(spawn);
                        if spawn <= *cancels {
                            req.cancel_token.cancelled().await;
                            SubagentResult {
                                success: false,
                                error: Some("cancelled".into()),
                                cancelled: true,
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        } else {
                            plan_written(&req, plan_path.as_deref(), body)
                        }
                    }
                    SpawnBehaviour::NoWriteThenDone => SubagentResult {
                        success: true,
                        output: StdArc::from("Done"),
                        subagent_id: req.id.clone(),
                        child_session_id: req.id.clone(),
                        ..Default::default()
                    },
                    SpawnBehaviour::Runtime { message, cancelled } => SubagentResult {
                        success: false,
                        error: Some(message.clone()),
                        cancelled: *cancelled,
                        subagent_id: req.id.clone(),
                        child_session_id: req.id.clone(),
                        ..Default::default()
                    },
                    SpawnBehaviour::AdmissionGatedThenWrite { opened, body } => {
                        if !opened.load(SeqOrd::SeqCst) {
                            // The real coordinator's spawn_blocked_sessions
                            // rejection: instant cancel, no attempt.
                            SubagentResult {
                                success: false,
                                error: Some("parent session is stopped".into()),
                                cancelled: true,
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        } else {
                            if let Some(p) = plan_path.as_deref() {
                                let _ = std::fs::create_dir_all(
                                    std::path::Path::new(p).parent().unwrap(),
                                );
                                let _ = std::fs::write(p, body);
                            }
                            SubagentResult {
                                success: true,
                                output: StdArc::from("Done"),
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        }
                    }
                    SpawnBehaviour::RuntimeThenWritePlan { message, body } => {
                        if count_task.load(SeqOrd::SeqCst) == 1 {
                            SubagentResult {
                                success: false,
                                error: Some(message.clone()),
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        } else {
                            plan_written(&req, plan_path.as_deref(), body)
                        }
                    }
                };
                let _ = req.result_tx.send(result);
            }
        }
    });
    (tx, spawn_count, capture)
}

/// Build a `SessionActor` with goal harness enabled, planner enabled, the supplied coordinator, and a unique tempdir session dir.
/// The tempdir isolates each test's `plan_path()`.
/// There is **no active goal yet**; the caller drives `setup_goal` or `create_goal` directly.
async fn make_planner_actor(
    coordinator_tx: Option<tokio::sync::mpsc::UnboundedSender<SubagentEvent>>,
    planner_enabled: bool,
) -> (StdArc<SessionActor>, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.events = crate::session::events::EventTracker::new(tmp.path());
    actor.goal_enabled = true;
    set_goal_harness_for_tests(&actor);
    actor.goal_planner_enabled = planner_enabled;
    actor.goal_tracker = Arc::new(parking_lot::Mutex::new(
        crate::session::goal_tracker::GoalTracker::new(tmp.path().to_path_buf()),
    ));
    if let Some(tx) = coordinator_tx {
        actor.tool_context.subagent_event_tx = Some(tx);
    }
    (StdArc::new(actor), tmp)
}

/// Like [`make_planner_actor`] but retains the persistence receiver so a test can inspect the `GoalUpdated` notifications the planner run emits.
/// Tests use it to assert the wire-only `planning` flag is set then cleared.
async fn make_planner_actor_capturing(
    coordinator_tx: Option<tokio::sync::mpsc::UnboundedSender<SubagentEvent>>,
    planner_enabled: bool,
) -> (
    StdArc<SessionActor>,
    TempDir,
    tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>,
) {
    let tmp = TempDir::new().expect("tempdir");
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, persistence_rx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.events = crate::session::events::EventTracker::new(tmp.path());
    actor.goal_enabled = true;
    set_goal_harness_for_tests(&actor);
    actor.goal_planner_enabled = planner_enabled;
    actor.goal_tracker = Arc::new(parking_lot::Mutex::new(
        crate::session::goal_tracker::GoalTracker::new(tmp.path().to_path_buf()),
    ));
    if let Some(tx) = coordinator_tx {
        actor.tool_context.subagent_event_tx = Some(tx);
    }
    (StdArc::new(actor), tmp, persistence_rx)
}

/// Drain every persisted `GoalUpdated` notification and project to its wire-only `planning` flag, preserving emission order.
fn drain_goal_planning_flags(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>,
) -> Vec<Option<bool>> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PersistenceMsg::Update(crate::session::storage::SessionUpdate::Xai(n)) = msg
            && let crate::extensions::notification::SessionUpdate::GoalUpdated { planning, .. } =
                n.update
        {
            out.push(planning);
        }
    }
    out
}

fn create_test_goal(actor: &SessionActor) {
    actor.goal_tracker.lock().create_goal(
        "g-test".into(),
        "test objective".into(),
        None,
        0,
        "2026-01-01T00:00:00Z".into(),
        None,
    );
}

/// Every todo on the session's LIVE list, as `(id, content, status)`.
///
/// Read through the session's own tool bridge, which is where `todo_write`
/// keeps it — the same state the model and the pager see.
async fn live_todos(actor: &SessionActor) -> Vec<(String, String, crate::tools::todo::TodoStatus)> {
    use crate::tools::todo::TodoState;
    use xai_grok_tools::types::resources::State;
    actor
        .agent
        .borrow()
        .tool_bridge()
        .read_resource::<State<TodoState>>()
        .await
        .map(|state| {
            state
                .0
                .todo_items_with_ids()
                .map(|(id, item)| (id.clone(), item.content.clone(), item.status))
                .collect()
        })
        .unwrap_or_default()
}

/// Just the contents of the live list, in order.
async fn live_todo_contents(actor: &SessionActor) -> Vec<String> {
    live_todos(actor)
        .await
        .into_iter()
        .map(|(_id, content, _status)| content)
        .collect()
}

/// Register the real grok-build `todo_write` tool and bind this session's
/// toolset into the workspace, so a seeded append dispatches onto the session's
/// live `State<TodoState>` exactly as it does in a real session.
async fn arm_todo_writes(actor: &SessionActor) {
    *actor.agent.borrow_mut() = test_grok_build_agent_with_todo().await;
    actor
        .workspace_ops
        .bind_local_session(
            &actor.session_id_string(),
            actor.tool_context.cwd.as_path().to_path_buf(),
            actor.tool_context.hunk_tracker_handle.clone(),
            actor.agent.borrow().tool_bridge().toolset(),
            None,
        )
        .expect("bind_local_session");
}

/// Write one item the way the main agent (or the user) writes it, so a seeding
/// test can prove it survives untouched.
async fn write_existing_todo(actor: &SessionActor, id: &str, content: &str) {
    actor
        .workspace_ops
        .call_tool(
            "todo_write",
            serde_json::json!({
                "merge": true,
                "todos": [{"id": id, "content": content, "status": "in_progress"}],
            }),
            "test-seed",
            Some(&actor.session_info.id.0),
        )
        .await
        .expect("an existing todo must be writable");
}

/// The three work items the scripted planner puts on its OWN todo list with
/// `todo_write` while it plans — the source of the session's seeded list.
const PLANNER_TODOS: &[&str] = &[
    "add the plan parser",
    "wire it into the publish path",
    "cover it with an end-to-end test",
];

/// A plan whose `## Task checklist` names those same three steps, the way the
/// planner prompt asks a real planner to write it.
const CHECKLIST_PLAN: &[u8] = b"# Plan: ship the exporter\n\n## Goal kind\ncode-change\n\n\
## Task checklist\n\
- [ ] add the plan parser\n\
- [ ] wire it into the publish path\n\
- [ ] cover it with an end-to-end test\n";

/// The scripted planner the seeding tests drive: it writes [`CHECKLIST_PLAN`]
/// and reports the same steps on ITS OWN list with `todo_write`, which is what
/// the planner prompt asks a real planner to do.
fn scripted_planner_with_todos() -> SpawnBehaviour {
    SpawnBehaviour::WritePlanThenDoneWithTodos {
        body: CHECKLIST_PLAN,
        todos: PLANNER_TODOS,
    }
}

/// Like [`make_planner_actor`] but retains the session's event receiver, so a
/// test can read the notifications the actor enqueued for the client — the same
/// FIFO the session's event loop forwards to the pager.
async fn make_planner_actor_with_events(
    coordinator_tx: Option<tokio::sync::mpsc::UnboundedSender<SubagentEvent>>,
    planner_enabled: bool,
) -> (
    StdArc<SessionActor>,
    TempDir,
    tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
) {
    let tmp = TempDir::new().expect("tempdir");
    let (gateway_tx, _gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _persistence_rx) =
        tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let (mut actor, event_rx) =
        create_test_actor_ex(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.events = crate::session::events::EventTracker::new(tmp.path());
    actor.goal_enabled = true;
    set_goal_harness_for_tests(&actor);
    actor.goal_planner_enabled = planner_enabled;
    actor.goal_tracker = Arc::new(parking_lot::Mutex::new(
        crate::session::goal_tracker::GoalTracker::new(tmp.path().to_path_buf()),
    ));
    if let Some(tx) = coordinator_tx {
        actor.tool_context.subagent_event_tx = Some(tx);
    }
    (StdArc::new(actor), tmp, event_rx)
}

/// Every `Plan` session update the actor enqueued for the client, in order — the
/// view the pager renders the todo list from.
fn plan_updates(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
) -> Vec<Vec<acp::PlanEntry>> {
    let mut plans = Vec::new();
    while let Ok(event) = rx.try_recv() {
        let SessionEvent::Notification(SessionNotification::Acp(n)) = event else {
            continue;
        };
        if let acp::SessionUpdate::Plan(plan) = n.update {
            plans.push(plan.entries);
        }
    }
    plans
}

/// A seeded item is a todo like any other: the seed dispatches through the
/// session's own `todo_write`, so the same `Plan` session update a
/// model-written list produces is enqueued for the client, carrying the items
/// the planner listed.
#[tokio::test(flavor = "current_thread")]
async fn the_seed_reaches_the_client_as_a_plan_update() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(scripted_planner_with_todos());
            let (actor, _tmp, mut event_rx) = make_planner_actor_with_events(Some(tx), true).await;
            arm_todo_writes(&actor).await;

            let _ = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await;

            let plans = plan_updates(&mut event_rx);
            let entries = plans
                .last()
                .expect("the seed must enqueue a client-visible Plan update");
            assert_eq!(
                entries
                    .iter()
                    .map(|e| e.content.as_str())
                    .collect::<Vec<_>>(),
                PLANNER_TODOS.to_vec(),
                "the Plan update carries the planner's items",
            );
            assert!(
                entries
                    .iter()
                    .all(|e| matches!(e.status, acp::PlanEntryStatus::Pending)),
                "seeded items are pending: {entries:?}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn a_lite_goal_never_spawns_the_planner() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _count, capture) =
                spawn_planner_coordinator_capturing(scripted_planner_with_todos());
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;

            let GoalSetupOutcome::Inference { reminder } =
                actor.setup_goal("ship it", None, GoalMode::Lite).await
            else {
                panic!("a lite goal must flow through to inference");
            };
            assert!(!reminder.contains("Plan:"), "no plan block: {reminder}");
            actor
                .goal_tracker
                .lock()
                .pause(crate::session::goal_tracker::GoalPauseReason::User);
            let _ = actor.resume_goal().await;

            assert!(
                capture.prompt.lock().unwrap().is_empty(),
                "a lite goal must not spawn the planner"
            );
            let tracker = actor.goal_tracker.lock();
            let o = tracker.snapshot().expect("goal exists");
            assert_eq!(o.status, crate::session::goal_tracker::GoalStatus::Active);
            assert!(o.plan_file.is_none());
        })
        .await;
}

/// path hands the planner a prompt that tells it to list the plan's work with
/// the session's own todo tool — the instruction the whole feature rests on,
/// asserted on the prompt the coordinator was actually given rather than on a
/// template rendered in isolation.
#[tokio::test(flavor = "current_thread")]
async fn the_planner_is_spawned_with_the_todo_instruction() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _count, capture) =
                spawn_planner_coordinator_capturing(scripted_planner_with_todos());
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;

            let _ = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await;

            let prompts = capture.prompt.lock().unwrap().clone();
            assert_eq!(prompts.len(), 1, "one planner spawn for one goal");
            let prompt = &prompts[0];
            assert!(
                prompt.contains("## Todo list — REQUIRED"),
                "the planner's own prompt must carry the todo-list section",
            );
            assert!(
                prompt.contains("`todo_write`"),
                "and name the session's todo tool — the session advertises it as \
                 `todo_write`, so `{{TODO_TOOL}}` must have resolved through the live \
                 bridge: {prompt}",
            );
            assert!(
                prompt.contains("one item per `## Task checklist` line"),
                "one item per plan step, in order",
            );
            assert!(
                !prompt.contains("{TODO_TOOL}"),
                "an unresolved placeholder would name no tool at all",
            );
        })
        .await;
}

/// The shipped reader that carries a planner child's items back
/// ([`crate::agent::subagent::session_todo_contents`]), driven against a real
/// bound session whose list was written through the real tool — and against an
/// id that was never bound, which is the proxy-mode / no-child case.
#[tokio::test(flavor = "current_thread")]
async fn the_child_todo_reader_reads_a_bound_sessions_live_list() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, false).await;
            arm_todo_writes(&actor).await;
            write_existing_todo(&actor, "t1", "the user's own item").await;
            write_existing_todo(&actor, "t2", "a second item").await;

            let read = crate::agent::subagent::session_todo_contents(
                &actor.workspace_ops,
                &actor.session_id_string(),
            )
            .await;
            assert_eq!(
                read,
                vec![
                    "the user's own item".to_string(),
                    "a second item".to_string()
                ],
                "the reader must return the session's own live items, in order",
            );
            assert!(
                crate::agent::subagent::session_todo_contents(&actor.workspace_ops, "never-bound")
                    .await
                    .is_empty(),
                "an unbound session reads empty, which is what the merge treats as \
                 `nothing to carry over`",
            );
        })
        .await;
}

/// The gate for the objective's first two criteria: one `setup_goal` call —
/// with no model turn of its own — leaves the session's LIVE todo list carrying
/// the planner's items, each a fresh pending harness-minted item.
#[tokio::test(flavor = "current_thread")]
async fn setup_goal_seeds_the_planners_own_items_without_a_model_turn() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(scripted_planner_with_todos());
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;

            // The whole point: `setup_goal` returns the reminder, so no model
            // turn has run — and the list is already populated when it does.
            let GoalSetupOutcome::Inference { reminder } = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await
            else {
                panic!("a published plan must flow through to inference");
            };

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(
                snap.plan_file.is_some(),
                "the scripted planner wrote a plan, so the outcome must be Planned",
            );
            assert!(
                snap.plan_todos_seeded,
                "the publish path must record the seed"
            );

            let todos = live_todos(&actor).await;
            // Printed so `--nocapture` captures the observed list: the
            // assertions are the gate, this is the evidence a reader audits.
            println!(
                "=== session todo list after setup_goal, from the planner's own items ===\n\
                 {todos:?}\n"
            );
            assert_eq!(
                todos
                    .iter()
                    .map(|(_id, content, _status)| content.as_str())
                    .collect::<Vec<_>>(),
                PLANNER_TODOS.to_vec(),
                "the planner's items, in its own order: {todos:?}",
            );
            for (id, _content, status) in &todos {
                assert!(
                    id.starts_with("plan-"),
                    "the harness minted this id, so the main agent did not write it: {id}",
                );
                assert_eq!(*status, crate::tools::todo::TodoStatus::Pending);
            }
            assert!(
                reminder.contains("Plan: "),
                "the reminder still points at the plan:\n{reminder}",
            );
        })
        .await;
}

/// The items come from the planner's own `todo_write`, not from the harness
/// reading the plan: the plan body names a different step, and only the child's
/// items land.
#[tokio::test(flavor = "current_thread")]
async fn the_planner_childs_own_items_are_the_source_not_the_plan_prose() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDoneWithTodos {
                body: b"# Plan\n\n## Task checklist\n- [ ] a step the plan body names\n",
                todos: &["the child's first step", "the child's second step"],
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;

            let _ = actor.setup_goal("ship it", None, GoalMode::Full).await;

            let landed = live_todo_contents(&actor).await;
            println!(
                "=== plan prose names `a step the plan body names`; session list ===\n{landed:?}\n"
            );
            assert_eq!(
                landed,
                vec![
                    "the child's first step".to_string(),
                    "the child's second step".to_string(),
                ],
                "the planner's own list is what lands on the session's list",
            );
        })
        .await;
}

/// The negative half of the same gate: a planning run that made no todo call
/// leaves the list alone. The harness must not mine the plan prose for items —
/// an unfollowed instruction is not silently replaced by machinery.
#[tokio::test(flavor = "current_thread")]
async fn a_planner_that_named_no_items_leaves_the_list_untouched() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Writes a plan with a full checklist, but reports no todo list:
            // the planner never called `todo_write`.
            let (tx, _c) = spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone {
                body: CHECKLIST_PLAN,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;

            let _ = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_some(), "the plan was still published");
            assert!(
                !snap.plan_todos_seeded,
                "nothing was seeded, so the goal must not claim it was",
            );
            println!(
                "=== plan published, but the planner listed no items; session list ===\n{:?}\n",
                live_todos(&actor).await
            );
            assert!(
                live_todos(&actor).await.is_empty(),
                "no todo call means no items — the plan prose is not mined for them",
            );
        })
        .await;
}

/// Seeding is append-only and once per goal: a pre-existing item keeps its id,
/// text and status, and re-running the seed over the same items adds nothing.
#[tokio::test(flavor = "current_thread")]
async fn goal_seeding_is_append_only_and_idempotent() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(scripted_planner_with_todos());
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;
            write_existing_todo(&actor, "t1", "the user's own item").await;
            // Printed so `--nocapture` captures the before/after lists: the
            // assertions are the gate, this is the evidence a reader audits.
            println!(
                "=== todo list BEFORE planning ===\n{:?}\n",
                live_todos(&actor).await
            );

            let _ = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await;
            let after_publish = live_todos(&actor).await;
            println!("=== todo list AFTER planning ===\n{after_publish:?}\n");

            let (seed_id, seed_content, seed_status) = after_publish
                .iter()
                .find(|(id, _, _)| id == "t1")
                .expect("the pre-existing item must survive seeding")
                .clone();
            assert_eq!(seed_id, "t1");
            assert_eq!(seed_content, "the user's own item");
            assert_eq!(
                seed_status,
                crate::tools::todo::TodoStatus::InProgress,
                "seeding must not touch a status it did not write",
            );
            assert_eq!(
                after_publish
                    .iter()
                    .filter(|(id, _, _)| id != "t1")
                    .map(|(id, _, _)| id.as_str())
                    .count(),
                PLANNER_TODOS.len(),
                "one new item per item the planner listed: {after_publish:?}",
            );
            let seeded_ids: Vec<&str> = after_publish
                .iter()
                .filter(|(id, _, _)| id != "t1")
                .map(|(id, _, _)| id.as_str())
                .collect();
            assert!(
                seeded_ids.iter().all(|id| id.starts_with("plan-")),
                "seeded ids are harness-minted, so none collides with an existing one: \
                 {seeded_ids:?}",
            );

            // A resume/retry reaches the planner through this entry point. The
            // plan is already published, so it is a no-op — and even a direct
            // re-run of the seed must be too.
            actor.maybe_run_goal_planner("ship the exporter").await;
            let items: Vec<String> = PLANNER_TODOS.iter().map(|s| (*s).to_string()).collect();
            actor.apply_planner_todos("g-test", &items).await;

            let second = live_todos(&actor).await;
            println!("=== todo list AFTER the retry ===\n{second:?}\n");
            assert_eq!(
                second
                    .iter()
                    .map(|(id, _, _)| id.clone())
                    .collect::<Vec<_>>(),
                after_publish
                    .iter()
                    .map(|(id, _, _)| id.clone())
                    .collect::<Vec<_>>(),
                "a retry must add no new item ids: {second:?}",
            );
            assert_eq!(
                live_todo_contents(&actor).await,
                after_publish
                    .iter()
                    .map(|(_, content, _)| content.clone())
                    .collect::<Vec<_>>(),
                "and must not duplicate contents",
            );
        })
        .await;
}

/// A planner that fails closed writes no plan, so there is nothing to seed and
/// the list is exactly as it was.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_planner_leaves_the_todo_list_untouched() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(SpawnBehaviour::NoWriteThenDone);
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            arm_todo_writes(&actor).await;
            write_existing_todo(&actor, "t1", "the user's own item").await;

            let _ = actor
                .setup_goal("ship the exporter", None, GoalMode::Full)
                .await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none(), "fail-closed publishes no plan");
            assert!(
                !snap.plan_todos_seeded,
                "nothing was seeded, so the goal must not claim it was",
            );
            assert_eq!(
                live_todos(&actor).await,
                vec![(
                    "t1".to_string(),
                    "the user's own item".to_string(),
                    crate::tools::todo::TodoStatus::InProgress,
                )],
                "the list must be exactly as it was",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn send_now_queues_planner_context_without_restart() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
            let objectives = StdArc::new(std::sync::Mutex::new(Vec::new()));
            let (context_tx, mut context_rx) = tokio::sync::mpsc::unbounded_channel();
            let notify = StdArc::new(tokio::sync::Notify::new());
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WaitForContextThenWrite {
                    started: started_tx,
                    objectives: StdArc::clone(&objectives),
                    context: context_tx,
                    notify: StdArc::clone(&notify),
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            {
                let mut state = actor.state.lock().await;
                state
                    .pending_inputs
                    .push_back(user_item("goal-running", "A"));
                state.running_task = Some(running_task_stub("goal-running"));
            }
            *actor
                .current_prompt_id
                .lock()
                .expect("current_prompt_id mutex poisoned") = Some("goal-running".into());

            let planner = {
                let actor = StdArc::clone(&actor);
                tokio::task::spawn_local(async move {
                    actor.setup_goal("do X", None, GoalMode::Full).await
                })
            };

            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), started_rx.recv())
                    .await
                    .expect("planner spawn"),
                Some(1),
            );
            for text in ["first", "second"] {
                let (respond_to, response_rx) = tokio::sync::oneshot::channel();
                assert!(
                    !actor
                        .queue_input(QueueInputRequest {
                            send_now: true,
                            ..queue_input_request(
                                vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
                                &format!("user-steer-{text}"),
                                respond_to,
                            )
                        })
                        .await
                );
                assert!(matches!(
                    response_rx.await.unwrap().unwrap().completion_kind,
                    PromptCompletionKind::RemovedFromQueue
                ));
            }

            // Both Send Nows reach the planner while it is still running; it is
            // released only after they have been delivered.
            let mut delivered = Vec::new();
            for _ in 0..2 {
                delivered.push(
                    tokio::time::timeout(std::time::Duration::from_secs(5), context_rx.recv())
                        .await
                        .expect("planner context delivered")
                        .expect("planner coordinator stub alive"),
                );
            }
            assert_eq!(
                delivered,
                [
                    "Additional user context for the current plan:\n\nfirst",
                    "Additional user context for the current plan:\n\nsecond",
                ]
            );
            notify.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(5), planner)
                .await
                .expect("planner completion")
                .unwrap();
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            assert!(
                context_rx.try_recv().is_err(),
                "each Send Now reaches the planner exactly once"
            );
            let prompts = objectives.lock().unwrap();
            let objectives = prompts
                .iter()
                .map(|prompt| {
                    prompt
                        .split_once("\n\nOBJECTIVE:\n")
                        .unwrap()
                        .1
                        .split_once("\n\nCONTEXT:\n")
                        .unwrap()
                        .0
                })
                .collect::<Vec<_>>();
            assert_eq!(objectives, ["do X"]);

            // Staged files (`plan-<uuid>.md`, `plan-baseline-<uuid>.md`) are `TempPath`s
            // Interrupted attempts drop theirs and the winner renames onto `plan.md`, so nothing matching `plan-*.md` survives
            let goal_dir = actor.goal_tracker.lock().plan_path();
            let goal_dir = goal_dir.parent().expect("plan path has a parent");
            let leaked: Vec<String> = std::fs::read_dir(goal_dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("plan-") && name.ends_with(".md"))
                .collect();
            assert!(leaked.is_empty(), "staged planner files leaked: {leaked:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_early_exit_clears_planning_latch() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, true).await;
            create_test_goal(&actor);
            {
                let mut tracker = actor.goal_tracker.lock();
                tracker.snapshot_mut().unwrap().planning_in_flight = true;
                assert!(tracker.pause(crate::session::goal_tracker::GoalPauseReason::User));
            }

            actor.maybe_run_goal_planner("do X").await;

            assert!(
                !actor
                    .goal_tracker
                    .lock()
                    .snapshot()
                    .unwrap()
                    .planning_in_flight
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_success_stamps_plan_file_on_orchestration() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"# Plan\n" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            let plan_path = actor.goal_tracker.lock().plan_path();
            let baseline_path = actor.goal_tracker.lock().plan_baseline_path();
            std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
            std::fs::write(&plan_path, "stale plan").unwrap();
            std::fs::write(&baseline_path, "stale baseline").unwrap();

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert_eq!(snap.plan_file.as_deref(), Some(plan_path.as_path()));
            assert_eq!(
                snap.plan_baseline_file.as_deref(),
                Some(baseline_path.as_path())
            );
            assert_eq!(std::fs::read_to_string(plan_path).unwrap(), "# Plan\n");
            assert_eq!(std::fs::read_to_string(baseline_path).unwrap(), "# Plan\n");
            assert_eq!(
                snap.status,
                crate::session::goal_tracker::GoalStatus::Active,
                "success must NOT pause the goal",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_spawn_sets_harness_only_fork_context() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count, capture) =
                spawn_planner_coordinator_capturing(SpawnBehaviour::WritePlanThenDone {
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let forks = capture.fork_context.lock().unwrap().clone();
            let surfaces = capture.surface_completion.lock().unwrap().clone();
            assert_eq!(forks, vec![true], "planner must request chat-prefix fork");
            assert_eq!(
                surfaces,
                vec![false],
                "planner remains harness-internal (no idle reminder)"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_fork_inherits_parent_model() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count, capture) =
                spawn_planner_coordinator_capturing(SpawnBehaviour::WritePlanThenDone {
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            // Configure an EXPLICIT planner role model different from the parent.
            // The mirror-child fork must IGNORE it and inherit the parent model, since the radix prefix is per-model
            // Without that forcing, the configured model would flow through and the assertion below would catch the regression
            let actor = StdArc::new(SessionActor {
                transient_retry_enabled: true,
                transient_retries_prompt_total: std::cell::Cell::new(0),
                transient_episode_start: std::cell::Cell::new(None),
                status_wake: Default::default(),
                goal_role_models: crate::session::GoalRoleModelConfig {
                    planner: crate::agent::config::GoalRoleModelChoice::Explicit(
                        crate::util::config::GoalRoleModel {
                            model: "some-other-planner-model".to_string(),
                            agent_type: "general-purpose".to_string(),
                        },
                    ),
                    ..Default::default()
                },
                ..StdArc::try_unwrap(actor).ok().expect("single-owner actor")
            });
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let models = capture.model.lock().unwrap().clone();
            assert_eq!(
                models,
                vec![None],
                "planner fork must ignore the configured role model and inherit the parent (None)"
            );
        })
        .await;
}

/// The planner's ORIGINAL plan is snapshotted to `plan.baseline.md` once, right after the plan is written, and is NOT re-synced to later edits.
/// A second `maybe_run_goal_planner` early-returns because a plan already exists.
/// It must leave the baseline pinned to the original body even after `plan.md` itself is edited on disk.
#[tokio::test(flavor = "current_thread")]
async fn planner_snapshots_plan_baseline_once_and_does_not_overwrite() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) = spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone {
                body: b"# Plan v1\n",
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            let baseline_path = actor.goal_tracker.lock().plan_baseline_path();

            actor.maybe_run_goal_planner("do X").await;

            // Baseline recorded on the orchestration and written to disk with the planner's original body
            let recorded = actor
                .goal_tracker
                .lock()
                .snapshot()
                .unwrap()
                .plan_baseline_file
                .clone();
            assert_eq!(
                recorded.as_deref(),
                Some(baseline_path.as_path()),
                "plan_baseline_file must point at plan.baseline.md",
            );
            assert_eq!(
                std::fs::read_to_string(&baseline_path).unwrap(),
                "# Plan v1\n",
                "baseline must hold the planner's ORIGINAL plan body",
            );

            // The agent edits plan.md mid-run; a second planner invocation early-returns (plan already present) and must NOT re-snapshot
            let plan_path = actor.goal_tracker.lock().plan_path();
            std::fs::write(&plan_path, "# Plan v2 (agent edited)\n").unwrap();
            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(
                std::fs::read_to_string(&baseline_path).unwrap(),
                "# Plan v1\n",
                "baseline must remain the ORIGINAL plan, never overwritten",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_records_own_harness_trace_turn_with_footer() {
    // The planner subagent is represented by its OWN trace turn.
    // After `maybe_run_goal_planner`, the chat-state side buffer holds exactly one sealed harness trace turn.
    // The result keeps the `<subagent_result>` footer (with the child session id) so the trace viewer can discover the planner subagent.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"# Plan\n" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            let turns = actor.chat_state_handle.take_harness_trace_turns().await;
            assert_eq!(turns.len(), 1, "planner rides its own trace turn");
            let [items] = turns.as_slice() else {
                panic!("planner rides its own trace turn: {turns:?}");
            };
            let [call, result] = items.as_slice() else {
                panic!("synthetic task call + result pair: {items:?}");
            };
            assert!(
                matches!(
                    call,
                    crate::sampling::ConversationItem::Assistant(a) if !a.tool_calls.is_empty()
                ),
                "first item is the synthetic task call",
            );
            let result_text = result.text_content();
            assert!(
                result_text.contains("<subagent_result>"),
                "footer present for trace-viewer / subagent discovery: {result_text}",
            );
            assert!(
                result_text.contains("subagent_id:"),
                "subagent_id present in footer: {result_text}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_disabled_records_no_harness_trace_turn() {
    // When there is no goal or the planner is off, the fast path produces no harness trace turn
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, false).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            let turns = actor.chat_state_handle.take_harness_trace_turns().await;
            assert!(turns.is_empty(), "planner disabled — no trace turn");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_success_sets_then_clears_planning_flag() {
    // The transient "planning…" badge fires before the subagent runs (planning=Some(true))
    // The success exit path clears it with a snapshot-derived GoalUpdated (planning=None)
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"# Plan\n" });
            let (actor, _tmp, mut persistence_rx) =
                make_planner_actor_capturing(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            let flags = drain_goal_planning_flags(&mut persistence_rx);
            assert_eq!(
                flags.first(),
                Some(&Some(true)),
                "planner must emit planning=true first; got {flags:?}",
            );
            assert_eq!(
                flags.last(),
                Some(&None),
                "success path must clear the planning badge; got {flags:?}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_clears_planning_latch_before_publishing_the_plan() {
    // Regression: the "planning…" badge must be cleared at the moment the planner run is taken and we commit to publishing the produced plan.
    // Clearing it only at the very end, after the plan/baseline I/O, is too late.
    // Before the fix the latch stayed set through the publish window, so the pager advertised "planning" while steering could no longer replan.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"# Plan\n" });
            let (actor, _tmp, mut persistence_rx) =
                make_planner_actor_capturing(Some(tx), true).await;
            create_test_goal(&actor);
            let plan_path = actor.goal_tracker.lock().plan_path();

            let observed_at_publish = StdArc::new(std::sync::Mutex::new(None::<bool>));
            let observer = {
                let actor = StdArc::clone(&actor);
                let plan_path = plan_path.clone();
                let observed = StdArc::clone(&observed_at_publish);
                tokio::task::spawn_local(async move {
                    loop {
                        if plan_path.exists() {
                            *observed.lock().unwrap() = actor
                                .goal_tracker
                                .lock()
                                .snapshot()
                                .map(|g| g.planning_in_flight);
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
            };

            actor.maybe_run_goal_planner("do X").await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), observer).await;

            assert_eq!(
                *observed_at_publish.lock().unwrap(),
                Some(false),
                "planning_in_flight must already be cleared by the time the plan is published \
                 (badge must not linger through the publish window)",
            );
            // End state: plan published, latch cleared, and the emitted flag sequence is exactly the transient badge on then off
            // There is no duplicate `planning=None` from the earlier clear plus the final catch-all
            assert!(
                actor
                    .goal_tracker
                    .lock()
                    .snapshot()
                    .unwrap()
                    .plan_file
                    .is_some()
            );
            let flags = drain_goal_planning_flags(&mut persistence_rx);
            assert_eq!(
                flags,
                vec![Some(true), None],
                "expected badge on then a single clear; got {flags:?}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_fail_closed_clears_planning_flag() {
    // Even when the planner fails closed (goal paused), the last GoalUpdated must clear planning so the "planning…" badge never sticks on screen
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "model rejected".into(),
                cancelled: false,
            });
            let (actor, _tmp, mut persistence_rx) =
                make_planner_actor_capturing(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert!(actor.goal_tracker.lock().status().unwrap().is_paused());
            let flags = drain_goal_planning_flags(&mut persistence_rx);
            assert_eq!(
                flags.first(),
                Some(&Some(true)),
                "planner must emit planning=true first; got {flags:?}",
            );
            assert_eq!(
                flags.last(),
                Some(&None),
                "fail-closed path must clear the planning badge; got {flags:?}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planning_badge_survives_intervening_goal_update() {
    // Regression: the subagent-spawn / token-accounting `GoalUpdated` that fires while the planner runs must NOT clear the "planning…" badge
    // The latch keeps every snapshot-derived update carrying it
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp, mut persistence_rx) = make_planner_actor_capturing(None, true).await;
            create_test_goal(&actor);

            actor.emit_goal_planning(0);
            // Intervening snapshot-derived emit (mirrors the spawn path).
            let (tokens_used, finished_marginal) = actor.goal_tokens(0);
            actor.goal_notify_sender().emit_goal_updated(
                &mut actor.goal_tracker.lock(),
                tokens_used,
                finished_marginal,
            );

            let flags = drain_goal_planning_flags(&mut persistence_rx);
            assert_eq!(flags.len(), 2, "expected both emits; got {flags:?}");
            assert!(
                flags.iter().all(|f| *f == Some(true)),
                "badge must persist across intervening updates; got {flags:?}",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_runtime_failure_pauses_goal_with_canonical_message() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "model rejected".into(),
                cancelled: false,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none(), "no plan written");
            assert!(
                snap.status.is_paused(),
                "fail-closed must pause; got {:?}",
                snap.status,
            );
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_failure_pause_message().as_str()),
            );
        })
        .await;
}

/// Regression: after a user Stop (ESC / Ctrl-C) the real coordinator latches
/// the session in `spawn_blocked_sessions`, and every Task spawn — the goal
/// planner included — is rejected as cancelled until an `OpenSpawnAdmission`
/// arrives. The only reopen site runs after the goal-slash dispatch, and the
/// resume path early-returns before it, which wedged goals until restart.
/// Setting/resuming a goal is user re-engagement, so the planner must open
/// admission itself before spawning.
#[tokio::test(flavor = "current_thread")]
async fn planner_reopens_spawn_admission_blocked_by_prior_cancel() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let opened = StdArc::new(std::sync::atomic::AtomicBool::new(false));
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::AdmissionGatedThenWrite {
                    opened: StdArc::clone(&opened),
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert!(
                opened.load(SeqOrd::SeqCst),
                "the planner must reopen spawn admission before spawning — \
                 without it the coordinator rejects the spawn as cancelled and \
                 the goal fails closed (\"Planning failed; resume with /goal to retry.\")",
            );
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(
                snap.plan_file.is_some(),
                "with admission reopened the planner run must succeed; got {snap:?}",
            );
            assert_eq!(
                snap.status,
                crate::session::goal_tracker::GoalStatus::Active
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_runtime_cancelled_pauses_as_planner() {
    // `cancelled: true` (max turns, rewind, dequeue) is a harness failure: the wire reason stays `aborted`, the pause is the planner's, not the user's
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "max turns".into(),
                cancelled: true,
            });
            let (actor, tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert_eq!(
                snap.status,
                crate::session::goal_tracker::GoalStatus::InfraPaused
            );
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_failure_pause_message().as_str()),
            );
            // `InfraPaused` is shared with `Infra`; the history is what distinguishes the planner pause.
            assert!(
                snap.history.iter().any(|entry| {
                    matches!(
                        entry.event,
                        crate::session::goal_tracker::GoalEvent::PlanningFailed
                    ) && entry.detail.as_deref() == Some("aborted")
                }),
                "{:?}",
                snap.history
            );
            assert!(
                snap.history
                    .iter()
                    .any(|entry| entry.detail.as_deref() == Some("planner")),
                "{:?}",
                snap.history
            );
            let events =
                std::fs::read_to_string(tmp.path().join("events.jsonl")).expect("events.jsonl");
            assert!(
                has_event_with(&events, "goal_auto_paused", |v| v["reason"] == "planner"),
                "{events}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_missing_plan_file_pauses_goal() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _spawn_count) = spawn_planner_coordinator(SpawnBehaviour::NoWriteThenDone);
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none());
            assert!(snap.status.is_paused());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_disabled_short_circuits_no_spawn_no_attempt() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"x" });
            let (actor, _tmp) = make_planner_actor(Some(tx), false).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 0);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_no_coordinator_skips_silently() {
    // External harness path: planner enabled but no subagent_event_tx.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none());
            assert_eq!(
                snap.status,
                crate::session::goal_tracker::GoalStatus::Active
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn planner_existing_plan_does_not_re_fire() {
    // Defensive: if `plan_file` is already populated (e.g. future re-trigger path that calls the helper twice), we do NOT re-spawn.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"x" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            {
                let mut tracker = actor.goal_tracker.lock();
                tracker.snapshot_mut().unwrap().plan_file =
                    Some(std::path::PathBuf::from("/tmp/preexisting/plan.md"));
            }

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 0);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_pauses_active_goal_with_no_plan() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, true).await;
            create_test_goal(&actor);
            assert!(
                actor
                    .goal_tracker
                    .lock()
                    .snapshot()
                    .unwrap()
                    .plan_file
                    .is_none()
            );

            actor.maybe_reconcile_active_goal_without_plan().await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.status.is_paused());
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_failure_pause_message().as_str()),
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_skips_active_goal_with_plan() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, true).await;
            create_test_goal(&actor);
            {
                let mut tracker = actor.goal_tracker.lock();
                tracker.snapshot_mut().unwrap().plan_file =
                    Some(std::path::PathBuf::from("/tmp/has-plan/plan.md"));
            }

            actor.maybe_reconcile_active_goal_without_plan().await;

            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::Active),
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_is_idempotent_via_atomic_flag() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, true).await;
            create_test_goal(&actor);
            actor.maybe_reconcile_active_goal_without_plan().await;
            assert!(actor.goal_tracker.lock().status().unwrap().is_paused());

            // Re-activate the tracker directly and re-run
            // The atomic short-circuit must skip the work; a regression that removed the swap would re-pause the goal
            actor.goal_tracker.lock().resume();
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::Active),
            );
            actor.maybe_reconcile_active_goal_without_plan().await;
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::Active),
                "atomic short-circuit must prevent re-pause; reconciler ran twice",
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_skips_when_planner_disabled() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, false).await;
            create_test_goal(&actor);

            actor.maybe_reconcile_active_goal_without_plan().await;

            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::Active),
            );
        })
        .await;
}

/// Planner subagent tokens fold into the goal's total via the shared `subagent_token_records` map.
/// The planner spawn routes through the same `SubagentSpawned` notification path the classifier uses.
/// The actor's notification handler tags the record with the active goal_id and `goal_tokens()` sums every matching record.
#[tokio::test(flavor = "current_thread")]
async fn planner_subagent_tokens_fold_into_goal_total() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"# Plan\n" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            let goal_id = actor
                .goal_tracker
                .lock()
                .snapshot()
                .unwrap()
                .goal_id
                .clone();

            actor.maybe_run_goal_planner("do X").await;
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);

            // Simulate the SubagentSpawned notification handler after the planner reports 12,000 cumulative tokens.
            let child_attempt = xai_message_delivery_core::AttemptId::mint(0x11).to_string();
            let mut record = SubagentTokenRecord::new(0);
            assert_eq!(
                record.spawn(Some(goal_id), Some(child_attempt.clone()), None),
                SubagentSpawnOutcome::Accepted
            );
            record.last_cumulative_reported = 12_000;
            actor
                .subagent_token_records
                .lock()
                .insert("planner-subagent-id".to_string(), record);

            // The pager adds active subagent tokens to the wire's finished-only field, so the active attempt is excluded there.
            let (tokens_used, finished_marginal) = actor.goal_tokens(0);
            assert_eq!(
                finished_marginal, 0,
                "an in-flight subagent must be excluded from `finished_marginal`",
            );
            assert!(
                tokens_used >= 12_000,
                "goal chip tokens_used must include planner marginal (got {tokens_used})",
            );
            // This mirrors the pager's live combine while the subagent runs
            // The pager sums parent_delta (0 here), finished_subagent_tokens, and its own active-subagent sum
            // That sum must never exceed the shell's total
            let active_subagent_tokens = 12_000i64;
            assert!(
                finished_marginal.saturating_add(active_subagent_tokens) <= tokens_used,
                "pager live combine must not exceed goal_tokens total while a subagent runs",
            );

            // Once the planner finishes, the attempt tokens move into completed spend.
            let mut records = actor.subagent_token_records.lock();
            let record = records
                .get_mut("planner-subagent-id")
                .expect("planner record present");
            assert_eq!(
                record.finish(
                    Some(&child_attempt),
                    SubagentFinishPayload {
                        tokens_used: 12_000,
                        ..Default::default()
                    }
                ),
                SubagentFinishOutcome::Accepted
            );
            drop(records);
            let (tokens_used, finished_marginal) = actor.goal_tokens(0);
            assert_eq!(
                finished_marginal, 12_000,
                "a sealed planner subagent marginal must reach `finished_marginal`",
            );
            assert!(tokens_used >= 12_000);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn lifecycle_fail_pause_resume_retry_success() {
    use crate::session::goal_tracker::GoalStatus;
    use std::sync::Mutex;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
            let spawn_count = StdArc::new(AtomicUsize::new(0));
            let count_task = StdArc::clone(&spawn_count);
            let plan_targets: StdArc<Mutex<Vec<String>>> = StdArc::new(Mutex::new(Vec::new()));
            let targets_task = StdArc::clone(&plan_targets);
            tokio::task::spawn_local(async move {
                while let Some(ev) = rx.recv().await {
                    if let SubagentEvent::Spawn(req) = ev {
                        let n = count_task.fetch_add(1, SeqOrd::SeqCst);
                        let plan_path = plan_path_from_prompt(&req.prompt);
                        if let Some(ref p) = plan_path {
                            targets_task.lock().unwrap().push(p.clone());
                        }
                        let result = if n == 0 {
                            SubagentResult {
                                success: false,
                                error: Some("planner failed".into()),
                                cancelled: false,
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        } else {
                            if let Some(p) = plan_path.as_deref() {
                                let _ = std::fs::create_dir_all(
                                    std::path::Path::new(p).parent().unwrap(),
                                );
                                let _ = std::fs::write(p, b"# Plan\n");
                            }
                            SubagentResult {
                                success: true,
                                output: StdArc::from("Done"),
                                subagent_id: req.id.clone(),
                                child_session_id: req.id.clone(),
                                ..Default::default()
                            }
                        };
                        let _ = req.result_tx.send(result);
                    }
                }
            });

            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;
            {
                let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
                assert!(snap.plan_file.is_none());
                assert!(snap.status.is_paused(), "got {:?}", snap.status);
                assert_eq!(
                    snap.pause_message.as_deref(),
                    Some(planner_failure_pause_message().as_str()),
                );
            }
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);

            let _ = actor.resume_goal().await;

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert_eq!(
                spawn_count.load(SeqOrd::SeqCst),
                2,
                "retry must re-fire planner"
            );
            assert_eq!(snap.status, GoalStatus::Active);
            assert!(
                snap.plan_file.is_some(),
                "successful retry writes plan_file"
            );
            let plan_path = actor.goal_tracker.lock().plan_path();
            assert_eq!(snap.plan_file.as_deref(), Some(plan_path.as_path()));
            let targets = plan_targets.lock().unwrap();
            let [first, second] = targets.as_slice() else {
                panic!("expected two plan targets: {targets:?}");
            };
            assert_ne!(first, second, "each attempt must use an isolated plan path");
            assert!(targets.iter().all(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("plan-"))
            }));
        })
        .await;
}

/// Repeated-failure variant: the planner fails, resume retries, the planner fails again, and the goal re-pauses with the canonical message.
/// This pins the retry path to the same fail-closed handling.
#[tokio::test(flavor = "current_thread")]
async fn lifecycle_fail_pause_resume_retry_fail_repauses() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            // Coordinator that always fails.
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "still broken".into(),
                cancelled: false,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;
            assert!(actor.goal_tracker.lock().status().unwrap().is_paused());

            let outcome = actor.resume_goal().await;
            // The planner re-failed and the goal re-paused, so resume must end the turn (Message), not flow through to inference on a paused goal
            assert!(
                matches!(outcome, GoalResumeOutcome::Message(_)),
                "re-paused resume must end the turn, not run inference",
            );

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 2);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none());
            assert!(snap.status.is_paused(), "got {:?}", snap.status);
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_failure_pause_message().as_str()),
            );
        })
        .await;
}

/// Resume of a goal that already has a `plan_file` must NOT re-fire the planner (defensive: the retry path keys off `plan_file.is_none()`).
#[tokio::test(flavor = "current_thread")]
async fn lifecycle_resume_with_plan_does_not_re_fire_planner() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"x" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            {
                let mut tracker = actor.goal_tracker.lock();
                let snap = tracker.snapshot_mut().unwrap();
                snap.plan_file = Some(std::path::PathBuf::from("/tmp/has-plan/plan.md"));
            }
            // Pause the goal manually (planner-failure simulation).
            let _ = actor
                .auto_pause_goal_if_active_with_message(
                    crate::session::goal_tracker::GoalPauseReason::User,
                    "user pause".into(),
                )
                .await;

            let _ = actor.resume_goal().await;

            assert_eq!(
                spawn_count.load(SeqOrd::SeqCst),
                0,
                "resume with plan present must not spawn planner",
            );
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::Active),
            );
        })
        .await;
}

#[derive(Debug, PartialEq)]
enum FakeEvent {
    CancelParentSession,
    OpenAdmission,
    Spawn { latched: bool },
}

fn spawn_latching_planner_coordinator() -> (
    tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    StdArc<std::sync::Mutex<Vec<FakeEvent>>>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    let log: StdArc<std::sync::Mutex<Vec<FakeEvent>>> =
        StdArc::new(std::sync::Mutex::new(Vec::new()));
    let log_task = StdArc::clone(&log);
    tokio::task::spawn_local(async move {
        let mut latched = false;
        while let Some(ev) = rx.recv().await {
            match ev {
                SubagentEvent::Cancel(req)
                    if matches!(req.target, SubagentCancelTarget::ParentSession) =>
                {
                    latched = true;
                    log_task
                        .lock()
                        .unwrap()
                        .push(FakeEvent::CancelParentSession);
                }
                SubagentEvent::OpenSpawnAdmission { .. } => {
                    latched = false;
                    log_task.lock().unwrap().push(FakeEvent::OpenAdmission);
                }
                SubagentEvent::Spawn(req) => {
                    log_task.lock().unwrap().push(FakeEvent::Spawn { latched });
                    let error = if latched {
                        "parent session is stopped"
                    } else {
                        "planner failed"
                    };
                    let result = SubagentResult {
                        success: false,
                        error: Some(error.into()),
                        cancelled: latched,
                        subagent_id: req.id.clone(),
                        child_session_id: req.id.clone(),
                        ..Default::default()
                    };
                    let _ = req.result_tx.send(result);
                }
                _ => {}
            }
        }
    });
    (tx, log)
}

#[tokio::test(flavor = "current_thread")]
async fn stop_then_slash_goal_resume_reopens_spawn_admission_before_planner_retry() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, log) = spawn_latching_planner_coordinator();
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            *actor.agent.borrow_mut() = test_agent_with_goal_tool().await;
            create_test_goal(&actor);
            let _ = actor
                .auto_pause_goal_if_active_with_message(
                    crate::session::goal_tracker::GoalPauseReason::User,
                    planner_failure_pause_message(),
                )
                .await;
            {
                let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
                assert!(snap.status.is_paused(), "got {:?}", snap.status);
                assert!(snap.plan_file.is_none());
            }

            *actor
                .current_prompt_id
                .lock()
                .expect("current_prompt_id mutex poisoned") = Some("running".to_string());
            {
                let mut state = actor.state.lock().await;
                state.running_task = Some(running_task_stub("running"));
                state.pending_inputs.push_back(user_item("running", "test"));
            }
            let _ = actor
                .cancel_running_task(crate::session::CancelOptions {
                    cancel_subagents: true,
                    trigger: Some(crate::session::CancelTrigger::CtrlC),
                    user_initiated: true,
                    ..Default::default()
                })
                .await;
            drain_gateway_turns().await;
            assert_eq!(
                *log.lock().unwrap(),
                vec![FakeEvent::CancelParentSession],
                "user Stop must latch spawn admission before the resume turn",
            );

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                actor.handle_turn_input(TurnInputRequest {
                    prompt_id: "goal-resume".into(),
                    input_origin: InputOrigin::new(PromptOrigin::User),
                    prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                        "/goal resume",
                    ))],
                    prompt_mode: PromptMode::Agent,
                    trace_gcs_config: None,
                    artifact_tracker: None,
                    client_identifier: None,
                    screen_mode: None,
                    verbatim: true,
                    send_now: false,
                    json_schema: None,
                    persist_ack: None,
                    parsed_prompt_tx: None,
                    traceparent: None,
                    start_gate: None,
                }),
            )
            .await
            .expect("turn must finish");
            assert!(
                result.is_ok(),
                "re-paused resume must end the host turn cleanly: {result:?}"
            );

            // The turn reopens admission, and the planner reopens it again before it spawns.
            assert_eq!(
                *log.lock().unwrap(),
                vec![
                    FakeEvent::CancelParentSession,
                    FakeEvent::OpenAdmission,
                    FakeEvent::OpenAdmission,
                    FakeEvent::Spawn { latched: false },
                ],
                "the resume turn must reopen spawn admission before the planner retry",
            );
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.status.is_paused(), "got {:?}", snap.status);
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_failure_pause_message().as_str()),
            );
        })
        .await;
}

/// End-to-end gate (enabled side): when the planner is on and writes a
/// plan, `setup_goal`'s reminder folds in the plan-aware block carrying
/// the actual `plan_path()` pointer, the "already on your list" statement
/// (NOT a manual seed directive), the `## Deviations` instruction, and the
/// legacy discipline intact.
#[tokio::test(flavor = "current_thread")]
async fn setup_goal_reminder_is_plan_aware_when_planner_enabled() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) = spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone {
                body: b"# Plan\n\n1. do it\n",
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            let plan_path = actor.goal_tracker.lock().plan_path();

            let GoalSetupOutcome::Inference { reminder } =
                actor.setup_goal("ship it", None, GoalMode::Full).await
            else {
                panic!("a published plan must flow through to inference");
            };
            // Printed so `cargo test -- --nocapture` captures the exact reminder
            // text for review: the assertions below are the gate, this is the
            // artifact a reader (or the goal's verification) reads.
            println!("=== plan-aware setup_goal reminder ===\n{reminder}\n=== end ===\n");

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert_eq!(snap.plan_file.as_deref(), Some(plan_path.as_path()));
            let expected = format!("\nPlan: {}\n", plan_path.display());
            assert!(
                reminder.contains(&expected),
                "reminder must carry the plan pointer line `{expected}`:\n{reminder}"
            );
            assert!(
                !reminder.contains(PLAN_SEED_TODOS_PHRASE),
                "the manual seed-todos directive must be gone — the harness \
                 populates the list itself:\n{reminder}"
            );
            assert!(
                reminder.contains(PLAN_TODOS_ALREADY_SEEDED_PHRASE),
                "the reminder must say the plan's steps are already on the list:\n{reminder}"
            );
        })
        .await;
}

/// End-to-end gate (disabled side, the default today): with the planner off, `setup_goal` writes no plan and the reminder renders the no-plan block.
/// There is no dangling `Plan:` pointer and no plan-aware phrasing, while the discipline and slim TRACKING/TEST sections remain.
#[tokio::test(flavor = "current_thread")]
async fn setup_goal_reminder_is_no_plan_when_planner_disabled() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, false).await;

            let GoalSetupOutcome::Inference { reminder } =
                actor.setup_goal("ship it", None, GoalMode::Full).await
            else {
                panic!("a disabled planner must flow through to inference");
            };
            println!("=== no-plan setup_goal reminder ===\n{reminder}\n=== end ===\n");

            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none(), "planner off writes no plan");
            assert!(
                !reminder.contains("\nPlan: "),
                "no-plan reminder must not carry a `Plan:` pointer:\n{reminder}"
            );
        })
        .await;
}

/// `/goal resume` on a planner-enabled goal with a plan must build a plan-aware reminder (carrying the real `plan_path()` pointer).
/// Guards against the resume site regressing to `None` while setup_goal stays correct (a prior regression: the sibling branch was untested).
/// The reminder is returned as the `Inference` turn content (resume flows through to inference now).
#[tokio::test(flavor = "current_thread")]
async fn goal_resume_reminder_is_plan_aware_when_planner_enabled() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, _c) =
                spawn_planner_coordinator(SpawnBehaviour::WritePlanThenDone { body: b"x" });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            let plan_path = actor.goal_tracker.lock().plan_path();
            {
                let mut tracker = actor.goal_tracker.lock();
                tracker.snapshot_mut().unwrap().plan_file = Some(plan_path.clone());
            }
            let _ = actor
                .auto_pause_goal_if_active_with_message(
                    crate::session::goal_tracker::GoalPauseReason::User,
                    "user pause".into(),
                )
                .await;

            let GoalResumeOutcome::Inference { reminder, .. } = actor.resume_goal().await else {
                panic!("resumed paused goal must flow through to inference");
            };
            println!("=== plan-aware /goal resume reminder ===\n{reminder}\n=== end ===\n");

            assert!(
                reminder.contains("Continue working now."),
                "resume reminder must close with the continuation directive:\n{reminder}"
            );
            let expected = format!("\nPlan: {}\n", plan_path.display());
            assert!(
                reminder.contains(&expected),
                "resume reminder must carry the plan pointer `{expected}`:\n{reminder}"
            );
            assert!(
                !reminder.contains(PLAN_SEED_TODOS_PHRASE),
                "resume reminder must not re-issue the manual seed directive:\n{reminder}"
            );
            assert!(
                reminder.contains(PLAN_TODOS_ALREADY_SEEDED_PHRASE),
                "resume reminder must say the plan's steps are already on the list:\n{reminder}"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn setup_goal_returns_message_when_planner_pauses() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "planner crashed".into(),
                cancelled: false,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;

            let GoalSetupOutcome::Message(msg) =
                actor.setup_goal("ship it", None, GoalMode::Full).await
            else {
                panic!("a planner pause must end the turn, not seed inference");
            };

            assert_eq!(
                msg,
                format!("Goal paused. {}", planner_failure_pause_message())
            );
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::InfraPaused)
            );
        })
        .await;
}

/// A `/goal pause` landing mid-plan stores no `pause_message`; the short-circuit must still produce a complete sentence.
#[tokio::test(flavor = "current_thread")]
async fn setup_goal_message_is_total_when_pause_message_missing() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            let tracker = Arc::clone(&actor.goal_tracker);
            tokio::task::spawn_local(async move {
                while let Some(ev) = rx.recv().await {
                    if let SubagentEvent::Spawn(req) = ev {
                        tracker
                            .lock()
                            .pause(crate::session::goal_tracker::GoalPauseReason::User);
                        let result = plan_written(&req, None, b"");
                        let _ = req.result_tx.send(result);
                    }
                }
            });

            let GoalSetupOutcome::Message(msg) =
                actor.setup_goal("ship it", None, GoalMode::Full).await
            else {
                panic!("a goal paused mid-plan must end the turn");
            };

            assert_eq!(msg, format!("Goal paused. {GOAL_RESUME_HINT}"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_after_planner_failure_refires_planner_and_reports_failure_again() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "planner crashed".into(),
                cancelled: false,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            actor.maybe_run_goal_planner("do X").await;
            assert!(actor.goal_tracker.lock().status().unwrap().is_paused());

            let GoalResumeOutcome::Message(msg) = actor.resume_goal().await else {
                panic!("a re-paused resume must end the turn");
            };

            assert_eq!(
                msg,
                format!(
                    "Planning failed again; goal paused. {}",
                    planner_failure_pause_message()
                )
            );
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 2);
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::InfraPaused)
            );
        })
        .await;
}

/// A planner pause is `InfraPaused`, but a plan published on the resume supersedes it: no "prior turn failed" recap.
#[tokio::test(flavor = "current_thread")]
async fn resume_after_planner_failure_succeeds_without_infra_recap() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::RuntimeThenWritePlan {
                    message: "planner crashed".into(),
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);
            actor.maybe_run_goal_planner("do X").await;
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(crate::session::goal_tracker::GoalStatus::InfraPaused)
            );

            let GoalResumeOutcome::Inference { reminder, .. } = actor.resume_goal().await else {
                panic!("a published plan must flow through to inference");
            };

            assert!(
                !reminder.contains("Previous state: Paused (infrastructure error)."),
                "{reminder}"
            );
            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 2);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_some());
            assert_eq!(
                snap.status,
                crate::session::goal_tracker::GoalStatus::Active
            );
        })
        .await;
}

/// With the planner disabled `plan_file` is always `None`, so the recap guard must key on a plan actually published, not on the retry attempt.
#[tokio::test(flavor = "current_thread")]
async fn resume_with_planner_disabled_keeps_infra_recap() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (actor, _tmp) = make_planner_actor(None, false).await;
            create_test_goal(&actor);
            let _ = actor
                .auto_pause_goal_if_active_with_message(
                    crate::session::goal_tracker::GoalPauseReason::Infra,
                    "Turn failed: rate limit".into(),
                )
                .await;

            let GoalResumeOutcome::Inference { reminder, .. } = actor.resume_goal().await else {
                panic!("an infra resume must flow through to inference");
            };

            assert!(
                reminder.contains("Previous state: Paused (infrastructure error)."),
                "{reminder}"
            );
            assert!(
                reminder.contains("Previous error: Turn failed: rate limit"),
                "{reminder}"
            );
        })
        .await;
}

/// The [X]/Stop shape: the plan writer is cancelled mid-stream with nothing
/// steering the replan. Spawning another planner is doing the opposite of what
/// was asked, and every one after the first is dead on arrival anyway — the
/// same Stop latched the session's spawns shut.
#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_planner_pauses_instead_of_respawning() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
            let objectives = StdArc::new(std::sync::Mutex::new(Vec::new()));
            let (tx, spawn_count) =
                spawn_planner_coordinator(SpawnBehaviour::WaitForCancelsThenWrite {
                    cancels: 1,
                    started: started_tx,
                    objectives: StdArc::clone(&objectives),
                    body: b"# Plan\n",
                });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            let planner = {
                let actor = StdArc::clone(&actor);
                tokio::task::spawn_local(async move { actor.maybe_run_goal_planner("do X").await })
            };

            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), started_rx.recv())
                    .await
                    .expect("planner spawn"),
                Some(1),
            );
            // A cancel with no steering behind it — what a Stop leaves.
            actor
                .goal_tracker
                .lock()
                .take_planner_run()
                .expect("planner run registered")
                .cancel
                .cancel();

            tokio::time::timeout(std::time::Duration::from_secs(5), planner)
                .await
                .expect("planner completion")
                .unwrap();

            assert_eq!(
                spawn_count.load(SeqOrd::SeqCst),
                1,
                "a cancel is terminal: no second planner",
            );
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.plan_file.is_none(), "no plan written");
            assert!(
                snap.status.is_paused(),
                "no plan means the goal cannot run; got {:?}",
                snap.status,
            );
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_cancelled_pause_message().as_str()),
                "a plan the user stopped is not a planner that failed",
            );
        })
        .await;
}

/// The same distinction one layer down: the subagent itself came back
/// cancelled, which `Aborted` already records — the pause has to say so too.
#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_subagent_pauses_as_cancelled_not_failed() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (tx, spawn_count) = spawn_planner_coordinator(SpawnBehaviour::Runtime {
                message: "Subagent was cancelled".into(),
                cancelled: true,
            });
            let (actor, _tmp) = make_planner_actor(Some(tx), true).await;
            create_test_goal(&actor);

            actor.maybe_run_goal_planner("do X").await;

            assert_eq!(spawn_count.load(SeqOrd::SeqCst), 1);
            let snap = actor.goal_tracker.lock().snapshot().cloned().unwrap();
            assert!(snap.status.is_paused());
            assert_eq!(
                snap.pause_message.as_deref(),
                Some(planner_cancelled_pause_message().as_str()),
            );
        })
        .await;
}
