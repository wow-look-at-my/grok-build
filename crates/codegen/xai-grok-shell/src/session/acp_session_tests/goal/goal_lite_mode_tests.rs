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
