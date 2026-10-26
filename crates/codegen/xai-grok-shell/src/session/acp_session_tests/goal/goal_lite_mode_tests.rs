//! Lite `/goal` runs on a real session actor.
use super::support::*;
use super::*;
use crate::session::goal_tracker::{GoalMode, GoalStatus};
use xai_grok_test_support::sse::responses_api_script_exact;
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse};

fn drain_gateway(mut rx: tokio::sync::mpsc::UnboundedReceiver<xai_acp_lib::AcpClientMessage>) {
    tokio::task::spawn_local(async move {
        while let Some(msg) = rx.recv().await {
            if let xai_acp_lib::AcpClientMessage::SessionNotification(args) = msg {
                let _ = args.response_tx.send(Ok(()));
            }
        }
    });
}

fn drain_persistence(mut rx: tokio::sync::mpsc::UnboundedReceiver<PersistenceMsg>) {
    tokio::task::spawn_local(async move {
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

/// An actor on the workflow-engine driver whose session model is `server`.
async fn goal_actor(server: &MockInferenceServer) -> SessionActor {
    let (gateway_tx, gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    drain_gateway(gateway_rx);
    let (persistence_tx, persistence_rx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    drain_persistence(persistence_rx);

    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.background_workflows_enabled = true;
    set_goal_harness_for_tests(&actor);

    let mut cfg = actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .expect("test actor has sampling config");
    cfg.base_url = server.url();
    cfg.api_backend = xai_grok_sampling_types::ApiBackend::Responses;
    cfg.model = "test".to_string();
    actor.chat_state_handle.update_sampling_config(cfg);
    let mut creds = actor.chat_state_handle.get_credentials().await;
    creds.api_key = Some("test-key".to_string());
    actor.chat_state_handle.update_credentials(creds);
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
                    directive.contains("Why the goal is not met yet: two tests still fail"),
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

/// Bind the real `todo_write` tool to the actor's live todo list.
async fn arm_todo_writes(actor: &mut SessionActor) {
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
            server.enqueue_response(
                "/v1/responses",
                verdict("candidate_complete", "the suite passed on the last run"),
            );
            let mut actor = goal_actor(&server).await;
            arm_todo_writes(&mut actor).await;
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
                0,
                "no evaluator request while todos are open"
            );
            assert_eq!(actor.goal_tracker.lock().status(), Some(GoalStatus::Active));

            write_todos(
                &actor,
                serde_json::json!([
                    {"id": "t2", "content": "wire the parser in", "status": "completed"},
                    {"id": "t3", "content": "cover it with a test", "status": "completed"},
                ]),
            )
            .await;
            assert!(matches!(
                actor.run_goal_round_end().await,
                GoalRoundDecision::EndTurn
            ));
            assert_eq!(server.request_count_for("/v1/responses"), 1);
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::Complete),
                "with every todo finished, the evaluator runs again"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn update_goal_completed_with_open_todos_is_rejected_without_a_validator() {
    use xai_grok_tools::implementations::grok_build::update_goal::{
        RejectReason, UpdateGoalAck, UpdateGoalInput,
    };
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let mut actor = goal_actor(&server).await;
            arm_todo_writes(&mut actor).await;
            for mode in [GoalMode::Lite, GoalMode::Full] {
                let GoalSetupOutcome::Inference { .. } =
                    actor.setup_goal("make the tests pass", None, mode).await
                else {
                    panic!("the goal must flow through to inference");
                };
                write_todos(&actor, mixed_todos()).await;
                let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
                actor
                    .drain_goal_updates_with_extra(
                        0,
                        DrainPurpose::MidTurn,
                        vec![(
                            UpdateGoalInput {
                                completed: Some(true),
                                message: None,
                                blocked_reason: None,
                            },
                            ack_tx,
                        )],
                    )
                    .await;
                match ack_rx.await.expect("the drain must answer") {
                    UpdateGoalAck::Rejected {
                        reason: RejectReason::UnfinishedTodos,
                        detail,
                    } => assert_lists_open_todos(&detail),
                    other => panic!("{mode:?}: open todos must reject the claim: {other:?}"),
                }
                assert_eq!(
                    actor.goal_tracker.lock().status(),
                    Some(GoalStatus::Active),
                    "{mode:?}"
                );
                assert!(
                    actor.pending_classifier_completions.lock().is_empty(),
                    "{mode:?}: the claim must not wait for a turn-end validator"
                );
                actor.goal_tracker.lock().complete();
            }
            assert_eq!(server.request_count(), 0, "no validator request was made");
        })
        .await;
}
