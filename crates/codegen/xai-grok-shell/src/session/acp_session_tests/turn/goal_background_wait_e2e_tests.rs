//! Runs the real turn loop against a scripted backend, with an active goal and a background subagent.
use super::support::*;
use super::*;
use crate::session::goal_tracker::{GoalMode, GoalStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use xai_grok_test_support::sse::chat_completion_script_exact;
use xai_grok_test_support::{MockInferenceServer, ScriptedResponse};
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentEvent, SubagentInspection, SubagentSnapshot, SubagentSnapshotStatus,
};

const MODEL_PATH: &str = "/v1/chat/completions";
const WAIT_MESSAGE: &str = "The build runs in the background. I wait for its result.";

/// `SessionActor` turn futures overflow the default test thread stack.
fn block_on_session(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(f)
        .expect("spawn large-stack test thread")
        .join()
        .expect("test thread");
}

fn current_thread_local<F>(f: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    tokio::task::LocalSet::new().block_on(&rt, f);
}

fn reply(text: &str) -> ScriptedResponse {
    ScriptedResponse::sse(chat_completion_script_exact(text, "test"))
}

fn running_inspection(id: &str) -> SubagentInspection {
    SubagentInspection {
        snapshot: SubagentSnapshot {
            subagent_id: id.to_string(),
            description: "build".to_string(),
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

/// An actor on the mock server with an active lite goal. Its coordinator reports `running` live subagents.
async fn goal_actor(server: &MockInferenceServer, running: Arc<AtomicUsize>) -> Arc<SessionActor> {
    let sampling_cfg = xai_grok_sampler::SamplerConfig {
        api_key: Some("test-key".to_string()),
        base_url: server.url(),
        model: "test".to_string(),
        api_backend: xai_grok_sampling_types::ApiBackend::ChatCompletions,
        context_window: 256_000,
        max_retries: Some(0),
        idle_timeout_secs: Some(30),
        ..Default::default()
    };
    let (sampler_event_tx, mut sampler_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_grok_sampler::SamplingEvent>();
    let sampler_handle = xai_grok_sampler::SamplerActor::spawn(
        sampling_cfg,
        xai_grok_sampler::RetryPolicy {
            max_retries: 0,
            ..Default::default()
        },
        sampler_event_tx,
    );
    let (gateway_tx, gateway_rx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    drain_gateway(gateway_rx);
    let (persistence_tx, persistence_rx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    drain_persistence(persistence_rx);
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.sampler_handle = sampler_handle;
    let mut cfg = actor
        .chat_state_handle
        .get_sampling_config()
        .await
        .expect("test actor has sampling config");
    cfg.base_url = server.url();
    cfg.api_backend = xai_grok_sampling_types::ApiBackend::ChatCompletions;
    cfg.model = "test".to_string();
    actor.chat_state_handle.update_sampling_config(cfg);
    let mut creds = actor.chat_state_handle.get_credentials().await;
    creds.api_key = Some("test-key".to_string());
    actor.chat_state_handle.update_credentials(creds);
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

    set_goal_harness_for_tests(&actor);
    {
        let mut tracker = actor.goal_tracker.lock();
        tracker.create_goal(
            "goal".into(),
            "make the build pass".into(),
            None,
            0,
            "2026-01-01T00:00:00Z".into(),
            None,
        );
        tracker.snapshot_mut().expect("the goal was created").mode = GoalMode::Lite;
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    actor.tool_context.subagent_event_tx = Some(event_tx);
    xai_spawn::spawn_local(async move {
        while let Some(event) = event_rx.recv().await {
            if let SubagentEvent::ListRunning(request) = event {
                let list = (0..running.load(Ordering::Relaxed))
                    .map(|i| running_inspection(&format!("sa-{i}")))
                    .collect();
                let _ = request.respond_to.send(list);
            }
        }
    });

    let actor = Arc::new(actor);
    {
        let drainer = actor.clone();
        xai_spawn::spawn_local(async move {
            while let Some(event) = sampler_event_rx.recv().await {
                drainer.handle_sampling_event(event).await;
            }
        });
    }
    actor
}

async fn run_turn(actor: &Arc<SessionActor>, prompt_id: &str, text: &str) -> PromptTurnResult {
    *actor
        .current_prompt_id
        .lock()
        .expect("current_prompt_id mutex poisoned") = Some(prompt_id.to_string());
    tokio::time::timeout(
        Duration::from_secs(60),
        actor.handle_prompt(
            prompt_id,
            vec![acp::ContentBlock::Text(acp::TextContent::new(
                text.to_string(),
            ))],
            PromptMode::Agent,
            None,
            None,
            None,
            None,
            true,
            false,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("turn must finish within timeout")
}

fn queued_ids(state: &State) -> Vec<String> {
    state
        .pending_inputs
        .iter()
        .map(|item| item.prompt_id.clone())
        .collect()
}

/// The model ends its turn to wait for a background subagent. The turn ends with its message and no goal check.
/// The completion wake then runs a turn, and the goal check runs at the end of that turn.
#[test]
fn a_goal_turn_waits_on_background_work_with_no_goal_check_until_the_wake_turn_ends() {
    block_on_session(|| {
        current_thread_local(async {
            let server = MockInferenceServer::start()
                .await
                .expect("mock inference server");
            let running = Arc::new(AtomicUsize::new(1));
            let actor = goal_actor(&server, Arc::clone(&running)).await;

            server.enqueue_response(MODEL_PATH, reply(WAIT_MESSAGE));
            let result = run_turn(&actor, "user-1", "start the build and wait for it").await;
            assert!(
                matches!(
                    result,
                    Ok(crate::session::commands::PromptTurnOk {
                        stop_reason: acp::StopReason::EndTurn,
                        ..
                    })
                ),
                "the waiting turn must end normally: {result:?}"
            );
            assert_eq!(
                server.request_count_for(MODEL_PATH),
                1,
                "one model request: no goal check runs while the subagent is live"
            );
            assert_eq!(actor.goal_tracker.lock().status(), Some(GoalStatus::Active));
            let conversation = actor.chat_state_handle.get_conversation().await;
            assert!(
                conversation
                    .iter()
                    .any(|item| item.text_content().contains(WAIT_MESSAGE)),
                "the model's message of the waiting turn must be in the conversation"
            );
            assert!(
                !conversation
                    .iter()
                    .any(|item| item.text_content().contains("Goal NOT complete")),
                "no continuation directive may follow the waiting turn"
            );

            actor.handle_turn_end(true, false).await;
            assert!(
                queued_ids(&*actor.state.lock().await).is_empty(),
                "the session idles: no goal continuation while the subagent runs"
            );

            running.store(0, Ordering::Relaxed);
            server.enqueue_response(MODEL_PATH, reply("The build passed."));
            server.enqueue_response(
                MODEL_PATH,
                reply(
                    r#"{"decision":"candidate_complete","evidence":"the build passed","next_step":"none","blocker_key":""}"#,
                ),
            );
            let result = run_turn(
                &actor,
                "subagent-completed-sa-0",
                "<system-reminder>\nSubagent sa-0 completed: the build passed.\n</system-reminder>",
            )
            .await;
            assert!(
                result.is_ok(),
                "the wake turn must end normally: {result:?}"
            );
            let paths: Vec<String> = server
                .request_bodies()
                .iter()
                .map(|body| {
                    let last = body
                        .get("messages")
                        .and_then(|m| m.as_array())
                        .and_then(|m| m.last())
                        .map(|m| m.to_string())
                        .unwrap_or_default();
                    last.chars().take(160).collect()
                })
                .collect();
            let paths = (
                paths,
                format!("{result:?}"),
                actor.goal_tracker.lock().status(),
            );
            let pause = actor
                .goal_tracker
                .lock()
                .snapshot()
                .and_then(|goal| goal.pause_message.clone());
            assert_eq!(
                server.request_count_for(MODEL_PATH),
                3,
                "the wake turn samples the model, and then the goal check runs once: \
                 requests {paths:?}, goal pause {pause:?}"
            );
            assert_eq!(
                actor.goal_tracker.lock().status(),
                Some(GoalStatus::Complete),
                "the goal check ran at the end of the wake turn: goal pause {pause:?}"
            );
        });
    });
}
