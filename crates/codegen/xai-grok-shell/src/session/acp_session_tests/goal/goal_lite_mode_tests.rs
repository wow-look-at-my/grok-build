//! Lite `/goal` runs on a real session actor.
use super::support::*;
use super::*;
use crate::session::goal_tracker::{GoalMode, GoalStatus};
use xai_grok_test_support::sse::responses_api_script_exact;
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse};

fn drain_gateway(mut rx: tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>) {
    xai_spawn::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let xai_acp_lib::AcpClientMessage::SessionNotification(args) = msg {
                let _ = args.response_tx.send(Ok(()));
            }
        }
    });
}

fn drain_persistence(mut rx: tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>) {
    xai_spawn::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let PersistenceMsg::FlushAndAck { respond_to } = msg {
                let _ = respond_to.send(Ok(()));
            }
        }
    });
}

fn verdict(decision: &str, evidence: &str) -> ScriptedResponse {
    let json = format!(
        r#"{{"decision":"{decision}","evidence":"{evidence}","next_step":"run the suite","blocker_key":""}}"#
    );
    ScriptedResponse::sse(responses_api_script_exact(&json, "test"))
}

/// An actor on the workflow-engine driver whose session model is `server`, with the real todo tool.
async fn goal_actor(server: &MockInferenceServer) -> SessionActor {
    let mut actor = goal_actor_without_todo_tool(server).await;
    arm_todo_writes(&mut actor).await;
    actor
}

use super::support::{LITE_GOAL_CAPTURED_STEP as CAPTURED_STEP, lite_goal_capture as lite_capture};

/// Every todo on the session's list, as `(id, content, status)`.
async fn todo_list(actor: &SessionActor) -> Vec<(String, String, crate::tools::todo::TodoStatus)> {
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

/// Mark every item done, so the round end reaches the evaluator.
async fn finish_all_todos(actor: &SessionActor) {
    let todos: Vec<_> = todo_list(actor)
        .await
        .into_iter()
        .map(|(id, content, _)| serde_json::json!({"id": id, "content": content, "status": "completed"}))
        .collect();
    write_todos(actor, serde_json::Value::Array(todos)).await;
}

async fn goal_actor_without_todo_tool(server: &MockInferenceServer) -> SessionActor {
    let (gateway_tx, gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    drain_gateway(gateway_rx);
    let (persistence_tx, persistence_rx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    drain_persistence(persistence_rx);

    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.background_workflows_enabled = true;
    set_goal_harness_for_tests(&actor);
    point_model_at(&actor, server).await;
    actor
}

#[tokio::test(flavor = "current_thread")]
async fn a_lite_goal_is_sent_back_with_the_reason_then_ends_on_the_evaluator_alone() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response("/v1/responses", lite_capture());
            server.enqueue_response("/v1/responses", verdict("continue", "two tests still fail"));
            server.enqueue_response(
                "/v1/responses",
                verdict("candidate_complete", "the suite passed on the last run"),
            );
            let actor = goal_actor(&server).await;

            let GoalSetupOutcome::Inference { reminder } = actor
                .setup_goal("make the tests pass", None, GoalMode::Lite)
                .await
            else {
                panic!("a lite goal must flow through to inference");
            };
            finish_all_todos(&actor).await;
            assert!(
                reminder.contains("When that check finds the work complete, the goal ends."),
                "lite rules must say the check ends the goal: {reminder}"
            );
            assert!(
                !reminder.contains("verification panel")
                    && !reminder.contains("{COMPLETION_CHECK}"),
                "lite rules must not promise a panel: {reminder}"
            );
            assert_eq!(actor.goal_tracker.lock().mode(), GoalMode::Lite);

            match actor.run_goal_round_end().await {
                GoalRoundDecision::Continue(directive) => assert!(
                    directive.contains(
                        "Next step:\nThe goal check found the goal not met yet.\nWhy: two tests still fail"
                    ),
                    "the model must be told why it was sent back: {directive}"
                ),
                GoalRoundDecision::EndTurn => panic!("a `continue` verdict must continue"),
            }
            assert_eq!(actor.goal_tracker.lock().status(), Some(GoalStatus::Active));

            assert!(matches!(
                actor.run_goal_round_end().await,
                GoalRoundDecision::EndTurn
            ));
            // The test actor has the verifier off. A full goal would pause here
            // for want of a panel, so Complete proves no panel was asked for.
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::Complete)
            );
        })
        .await;
}

/// `/goal --lite <objective>` runs `/todo <objective>` before the goal exists.
#[tokio::test(flavor = "current_thread")]
async fn a_lite_goal_starts_with_the_objective_captured_on_the_todo_list() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.set_keep_requests(true);
            server.enqueue_response("/v1/responses", lite_capture());
            let actor = goal_actor(&server).await;

            let GoalSetupOutcome::Inference { .. } = actor
                .setup_goal("make the tests pass", None, GoalMode::Lite)
                .await
            else {
                panic!("a lite goal must flow through to inference");
            };
            let bodies = evaluator_bodies(&server);
            assert_eq!(bodies.len(), 1, "one capture call, nothing else");
            assert!(
                bodies[0].contains("todo-capture agent")
                    && bodies[0].contains("make the tests pass"),
                "the capture gets the objective as its request: {}",
                bodies[0]
            );
            let todos = todo_list(&actor).await;
            assert_eq!(todos.len(), 1, "{todos:?}");
            assert_eq!(todos[0].1, CAPTURED_STEP);
            assert_eq!(todos[0].2, crate::tools::todo::TodoStatus::Pending);
            assert_eq!(actor.goal_tracker.lock().status(), Some(GoalStatus::Active));
        })
        .await;
}

/// A capture that adds nothing leaves no goal behind: the goal never starts with an empty list.
#[tokio::test(flavor = "current_thread")]
async fn a_lite_goal_does_not_start_when_the_capture_fails() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let actor = goal_actor_without_todo_tool(&server).await;

            let GoalSetupOutcome::Message(message) = actor
                .setup_goal("make the tests pass", None, GoalMode::Lite)
                .await
            else {
                panic!("a failed capture must not start the goal");
            };
            assert!(
                message.starts_with(
                    "Goal not started: putting the objective on the todo list failed:"
                ),
                "{message}"
            );
            assert_eq!(
                actor.goal_tracker.lock().status(),
                None,
                "no goal was created"
            );
            assert_eq!(server.request_count(), 0);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_full_goal_still_hands_a_candidate_to_the_panel() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response(
                "/v1/responses",
                verdict("candidate_complete", "the suite passed on the last run"),
            );
            let actor = goal_actor(&server).await;

            let GoalSetupOutcome::Inference { reminder } = actor
                .setup_goal("make the tests pass", None, GoalMode::Full)
                .await
            else {
                panic!("a full goal with the planner off must flow through to inference");
            };
            assert!(reminder.contains("adversarial verification panel"));

            assert!(matches!(
                actor.run_goal_round_end().await,
                GoalRoundDecision::EndTurn
            ));
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::InfraPaused),
                "with the verifier off, a full goal pauses instead of completing"
            );
        })
        .await;
}

async fn write_todos(actor: &SessionActor, todos: serde_json::Value) {
    actor
        .workspace_ops
        .call_tool(
            "todo_write",
            serde_json::json!({ "todos": todos }),
            "test-seed",
            Some(&actor.session_info.id.0),
        )
        .await
        .expect("todo_write must succeed");
}

fn mixed_todos() -> serde_json::Value {
    serde_json::json!([
        {"id": "t1", "content": "write the parser", "status": "completed"},
        {"id": "t2", "content": "wire the parser in", "status": "in_progress"},
        {"id": "t3", "content": "cover it with a test", "status": "pending"},
        {"id": "t4", "content": "drop the old path", "status": "cancelled"},
    ])
}

fn assert_lists_open_todos(text: &str) {
    assert!(
        text.starts_with(
            "You have incomplete todos, complete them before trying to submit your work \
             to the goal validator."
        ),
        "the bounce must lead with the instruction: {text}"
    );
    assert!(
        text.contains("[in_progress] `t2`: wire the parser in")
            && text.contains("[pending] `t3`: cover it with a test"),
        "the bounce must list every open todo: {text}"
    );
    assert!(
        !text.contains("write the parser") && !text.contains("drop the old path"),
        "a finished todo is not listed: {text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_round_with_open_todos_is_bounced_without_an_evaluator_call() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response("/v1/responses", lite_capture());
            server.enqueue_response(
                "/v1/responses",
                verdict("candidate_complete", "the suite passed on the last run"),
            );
            let actor = goal_actor(&server).await;
            let GoalSetupOutcome::Inference { .. } = actor
                .setup_goal("make the tests pass", None, GoalMode::Lite)
                .await
            else {
                panic!("a lite goal must flow through to inference");
            };
            write_todos(&actor, mixed_todos()).await;

            match actor.run_goal_round_end().await {
                GoalRoundDecision::Continue(directive) => assert_lists_open_todos(&directive),
                GoalRoundDecision::EndTurn => panic!("open todos must send the round back"),
            }
            assert_eq!(
                server.request_count_for("/v1/responses"),
                1,
                "only the capture: no evaluator request while todos are open"
            );
            assert_eq!(actor.goal_tracker.lock().status(), Some(GoalStatus::Active));

            finish_all_todos(&actor).await;
            assert!(matches!(
                actor.run_goal_round_end().await,
                GoalRoundDecision::EndTurn
            ));
            assert_eq!(server.request_count_for("/v1/responses"), 2);
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::Complete),
                "with every todo finished, the evaluator runs again"
            );
        })
        .await;
}

const UNTRIED_STEP: &str = "Grep src/ViewBuffer.cpp for every lock of the recursive mutex";

fn claim_verdict(decision: &str, steps: &[&str], user_action: &str) -> ScriptedResponse {
    let json = serde_json::json!({
        "decision": decision,
        "evidence": "the agent read the bundle but not the mutex holders",
        "untried_steps": steps,
        "user_action": user_action,
    })
    .to_string();
    ScriptedResponse::sse(responses_api_script_exact(&json, "test"))
}

/// The agent ends the round with a give-up claim.
fn claim_round(actor: &SessionActor) {
    actor
        .chat_state_handle
        .push_user_message(crate::sampling::ConversationItem::user("continue the goal"));
    actor
        .chat_state_handle
        .push_assistant_response(crate::sampling::ConversationItem::assistant(format!(
            "I read every log in the bundle.\n\n{}\nThe stall needs a car-side trace.",
            crate::session::goal_evaluator::BLOCKED_CLAIM_HEADING
        )));
}

fn evaluator_bodies(server: &MockInferenceServer) -> Vec<String> {
    server
        .request_bodies()
        .iter()
        .map(|body| body.to_string())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_give_up_claim_is_sent_back_with_steps_then_upheld_on_the_same_path_for_lite_and_full() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            for mode in [GoalMode::Lite, GoalMode::Full] {
                let server = MockInferenceServer::start()
                    .await
                    .expect("mock inference server");
                server.set_keep_requests(true);
                if mode == GoalMode::Lite {
                    server.enqueue_response("/v1/responses", lite_capture());
                }
                server.enqueue_response(
                    "/v1/responses",
                    claim_verdict("rejected", &[UNTRIED_STEP], ""),
                );
                server.enqueue_response(
                    "/v1/responses",
                    claim_verdict("upheld", &[], "Capture a Tracy trace of updateColor()"),
                );
                let actor = goal_actor(&server).await;
                let GoalSetupOutcome::Inference { reminder } =
                    actor.setup_goal("find the stall", None, mode).await
                else {
                    panic!("{mode:?}: the goal must flow through to inference");
                };
                assert!(
                    reminder.contains("## Cannot complete"),
                    "{mode:?}: the rules must teach the claim: {reminder}"
                );

                claim_round(&actor);
                match actor.run_goal_round_end().await {
                    GoalRoundDecision::Continue(directive) => assert!(
                        directive.starts_with(
                            "Your claim that you cannot finish the goal was REJECTED."
                        ) && directive.contains(&format!("1. {UNTRIED_STEP}")),
                        "{mode:?}: a rejection must carry the untried steps: {directive}"
                    ),
                    GoalRoundDecision::EndTurn => {
                        panic!("{mode:?}: a rejected claim must send the agent back")
                    }
                }
                assert_eq!(
                    actor.goal_tracker.lock().status(),
                    Some(GoalStatus::Active),
                    "{mode:?}: a rejected claim must not run the panel"
                );

                claim_round(&actor);
                assert!(matches!(
                    actor.run_goal_round_end().await,
                    GoalRoundDecision::EndTurn
                ));
                let bodies = evaluator_bodies(&server);
                let capture_calls = usize::from(mode == GoalMode::Lite);
                assert_eq!(
                    bodies.len(),
                    capture_calls + 2,
                    "{mode:?}: one validator call per claim"
                );
                let second_review = &bodies[capture_calls + 1];
                assert!(
                    second_review.contains("prior_rejected_steps")
                        && second_review.contains(UNTRIED_STEP),
                    "{mode:?}: the second review must check the steps the first one gave"
                );
                // The test actor has the verifier off, so InfraPaused proves
                // the upheld claim went on to the panel in both modes.
                assert_eq!(
                    actor.goal_tracker.lock().status(),
                    Some(GoalStatus::InfraPaused),
                    "{mode:?}: an upheld claim goes to the panel"
                );
            }
        })
        .await;
}

fn tracker_view(actor: &SessionActor) -> (Option<GoalStatus>, String) {
    let tracker = actor.goal_tracker.lock();
    (
        tracker.status(),
        tracker
            .snapshot()
            .and_then(|o| o.pause_message.clone())
            .unwrap_or_default(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn open_todos_do_not_bounce_a_give_up_claim_unheard() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            server.enqueue_response("/v1/responses", lite_capture());
            server.enqueue_response(
                "/v1/responses",
                claim_verdict("upheld", &[], "Send the qtcar logs from before 08:46"),
            );
            let actor = goal_actor(&server).await;
            let GoalSetupOutcome::Inference { .. } = actor
                .setup_goal("find the stall", None, GoalMode::Lite)
                .await
            else {
                panic!("a lite goal must flow through to inference");
            };
            write_todos(&actor, mixed_todos()).await;
            claim_round(&actor);
            assert!(matches!(
                actor.run_goal_round_end().await,
                GoalRoundDecision::EndTurn
            ));
            assert_eq!(
                server.request_count_for("/v1/responses"),
                2,
                "the capture, then the claim review"
            );
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::InfraPaused),
                "the upheld claim went on to the panel, which this actor has off"
            );
        })
        .await;
}

/// A subagent coordinator for the full panel. Each skeptic writes `verdict`
/// to the verdict file its prompt names. Any other spawn is the summarizer.
/// The log keeps every spawn prompt.
fn panel_coordinator(
    verdict: serde_json::Value,
) -> (
    tokio::sync::mpsc::UnboundedSender<
        xai_grok_tools::implementations::grok_build::task::types::SubagentEvent,
    >,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    use xai_grok_tools::implementations::grok_build::task::types::{SubagentEvent, SubagentResult};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&prompts);
    xai_spawn::spawn_local(async move {
        while let Some(event) = rx.recv().await {
            let SubagentEvent::Spawn(req) = event else {
                continue;
            };
            log.lock().unwrap().push(req.prompt.clone());
            let output = match crate::session::goal_classifier::parse_verdict_path_from_prompt(
                &req.prompt,
            ) {
                Some(path) => {
                    std::fs::write(&path, verdict.to_string()).expect("write the verdict");
                    verdict.to_string()
                }
                None => "The stall was traced to the mutex holder.".to_string(),
            };
            let id = req.id.clone();
            let _ = req.result_tx.send(SubagentResult {
                success: true,
                output: std::sync::Arc::from(output.as_str()),
                subagent_id: id.clone(),
                child_session_id: id,
                ..Default::default()
            });
        }
    });
    (tx, prompts)
}

#[tokio::test(flavor = "current_thread")]
async fn lite_and_full_goals_run_the_panel_on_an_upheld_claim() {
    let blocked = serde_json::json!({
        "refuted": true,
        "confidence": "high",
        "blocking": "unverifiable",
        "evidence": "no car-side trace is reachable from this workspace",
        "findings": [{"kind": "gap", "location": "updateColor()", "detail": "needs a car-side Tracy trace"}],
    });
    let fixable = serde_json::json!({
        "refuted": true,
        "confidence": "high",
        "blocking": "none",
        "evidence": "src/ViewBuffer.cpp:146",
        "findings": [{"kind": "gap", "location": "src/ViewBuffer.cpp:146", "detail": "Grep src/ for every lock of the ViewBuffer recursive mutex"}],
    });
    let cleared = serde_json::json!({
        "refuted": false,
        "confidence": "high",
        "blocking": "none",
        "evidence": "the report names the mutex holder",
    });
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let cases = [
                (blocked, GoalStatus::Blocked),
                (fixable, GoalStatus::Active),
                (cleared, GoalStatus::Complete),
            ];
            for (mode, (verdict, expect)) in [GoalMode::Lite, GoalMode::Full]
                .into_iter()
                .flat_map(|mode| cases.clone().map(|case| (mode, case)))
            {
                let server = MockInferenceServer::start()
                    .await
                    .expect("mock inference server");
                if mode == GoalMode::Lite {
                    server.enqueue_response("/v1/responses", lite_capture());
                }
                server.enqueue_response(
                    "/v1/responses",
                    claim_verdict("upheld", &[], "Capture a Tracy trace of updateColor()"),
                );
                let (coordinator, prompts) = panel_coordinator(verdict);
                let mut actor = goal_actor(&server).await;
                actor.goal_classifier_enabled = true;
                actor.goal_summary_enabled = true;
                // Skeptics are configured. A lite goal still runs one.
                actor.goal_verifier_skeptic_count = match mode {
                    GoalMode::Lite => 3,
                    GoalMode::Full => 1,
                };
                actor.tool_context.subagent_event_tx = Some(coordinator);
                let GoalSetupOutcome::Inference { .. } =
                    actor.setup_goal("find the stall", None, mode).await
                else {
                    panic!("{mode:?}: the goal must flow through to inference");
                };

                claim_round(&actor);
                let decision = actor.run_goal_round_end().await;
                assert_eq!(actor.goal_tracker.lock().status(), Some(expect), "{mode:?}");

                let prompts = prompts.lock().unwrap().clone();
                let skeptic = &prompts[0];
                assert!(
                    skeptic.contains("The stall needs a car-side trace.")
                        && skeptic.contains("the implementer claims it cannot finish"),
                    "{mode:?} {expect:?}: the skeptic must see the claim and the rule for it"
                );
                if mode == GoalMode::Lite {
                    assert_eq!(
                        prompts.len(),
                        1,
                        "{expect:?}: a lite goal runs one validator and no summarizer"
                    );
                }
                match expect {
                    GoalStatus::Blocked => {
                        assert!(matches!(decision, GoalRoundDecision::EndTurn));
                        let (_, pause) = tracker_view(&actor);
                        assert!(
                            pause.contains("needs a car-side Tracy trace"),
                            "the pause names what only the user can supply: {pause}"
                        );
                    }
                    GoalStatus::Active => match decision {
                        GoalRoundDecision::Continue(directive) => assert!(
                            directive.starts_with(
                                "Your claim that you cannot finish the goal was REJECTED by the \
                                 verification panel."
                            ) && directive.contains(
                                "Grep src/ for every lock of the ViewBuffer recursive mutex"
                            ),
                            "the panel's concrete step reaches the agent: {directive}"
                        ),
                        GoalRoundDecision::EndTurn => {
                            panic!("a fixable panel gap must send the agent back")
                        }
                    },
                    _ => {
                        assert!(matches!(decision, GoalRoundDecision::EndTurn));
                        if mode == GoalMode::Full {
                            assert_eq!(
                                prompts.len(),
                                2,
                                "a cleared full panel runs the summarizer after the skeptic"
                            );
                        }
                    }
                }
            }
        })
        .await;
}
