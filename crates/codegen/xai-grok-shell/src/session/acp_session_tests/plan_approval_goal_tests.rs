use super::support::*;
use super::*;

const PLAN: &str = "# Plan: Add JWT auth\n\n## Acceptance criteria\n1. A login returns a token.\n";

struct Harness {
	actor: std::sync::Arc<SessionActor>,
	responder: tokio::task::JoinHandle<()>,
	_session_dir: tempfile::TempDir,
	_plan_dir: tempfile::TempDir,
}

/// An actor in active plan mode with `PLAN` on disk, whose client approves
/// every plan. `goal_harness` switches the goal harness on.
async fn approving_actor(goal_harness: bool) -> Harness {
	let session_dir = tempfile::tempdir().unwrap();
	let (gateway_tx, mut gateway_rx) = tokio::sync::mpsc::unbounded_channel();
	let (persistence_tx, _persistence_rx) = tokio::sync::mpsc::unbounded_channel();
	let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
	actor.events = crate::session::events::EventTracker::new(session_dir.path());
	if goal_harness {
		actor.goal_enabled = true;
		set_goal_harness_for_tests(&actor);
	}
	actor.goal_tracker = Arc::new(parking_lot::Mutex::new(
		crate::session::goal_tracker::GoalTracker::new(session_dir.path().to_path_buf()),
	));
	*actor.agent.borrow_mut() = test_agent_with_plan_tools().await;
	let plan_dir = tempfile::tempdir().unwrap();
	std::fs::write(plan_dir.path().join("plan.md"), PLAN).unwrap();
	{
		let mut tracker = actor.plan_mode.lock();
		*tracker = crate::session::plan_mode::PlanModeTracker::new(plan_dir.path().to_path_buf());
		tracker.activate_from_tool();
	}
	let responder = tokio::task::spawn_local(async move {
		while let Some(msg) = gateway_rx.recv().await {
			match msg {
				xai_acp_lib::AcpClientMessage::ExtMethod(args) => {
					let approved =
						serde_json::value::to_raw_value(&serde_json::json!({ "outcome": "approved" }))
							.unwrap();
					let _ = args
						.response_tx
						.send(Ok(acp::ExtResponse::new(approved.into())));
				}
				xai_acp_lib::AcpClientMessage::SessionNotification(args) => {
					let _ = args.response_tx.send(Ok(()));
				}
				_ => {}
			}
		}
	});
	Harness {
		actor: std::sync::Arc::new(actor),
		responder,
		_session_dir: session_dir,
		_plan_dir: plan_dir,
	}
}

async fn approve_plan(actor: &SessionActor) -> Vec<ConversationItem> {
	let call = crate::sampling::types::ToolCallResponse {
		id: "call-exit".to_string(),
		kind: "function".to_string(),
		function: crate::sampling::types::ToolCallFunction::new("exit_plan_mode", "{}"),
		vendor: Default::default(),
	};
	let mut deferred = Vec::new();
	let outcome = actor
		.prepare_tool_call(call, &mut deferred)
		.await
		.expect("prepare_tool_call should not error");
	assert!(outcome.is_ok(), "an approved plan must still run exit_plan_mode");
	deferred
}

fn text_of(item: &ConversationItem) -> String {
	match item {
		ConversationItem::User(user) => user
			.content
			.iter()
			.filter_map(|part| match part {
				xai_grok_sampling_types::ContentPart::Text { text } => Some(text.to_string()),
				_ => None,
			})
			.collect(),
		other => panic!("expected a user message, got {other:?}"),
	}
}

#[tokio::test(flavor = "current_thread")]
async fn approving_a_plan_makes_a_goal_whose_plan_is_the_approved_plan() {
	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let h = approving_actor(true).await;
			let deferred = approve_plan(&h.actor).await;

			let (status, objective, plan_file, baseline_file) = {
				let tracker = h.actor.goal_tracker.lock();
				let goal = tracker.snapshot().expect("approval must create a goal");
				(
					goal.status,
					goal.objective.clone(),
					goal.plan_file.clone(),
					goal.plan_baseline_file.clone(),
				)
			};
			assert_eq!(status, crate::session::goal_tracker::GoalStatus::Active);
			assert_eq!(objective, "Implement the approved plan: Add JWT auth");
			let plan_file = plan_file.expect("the goal must carry the approved plan");
			assert_eq!(std::fs::read_to_string(&plan_file).unwrap(), PLAN);
			let baseline_file = baseline_file.expect("the approved plan is the goal's baseline");
			assert_eq!(std::fs::read_to_string(&baseline_file).unwrap(), PLAN);

			assert_eq!(deferred.len(), 1, "one goal-start reminder follows the tool result");
			let reminder = text_of(&deferred[0]);
			assert!(reminder.starts_with("<system-reminder>"), "{reminder}");
			assert!(reminder.contains(&objective), "{reminder}");
			assert!(
				reminder.contains(&plan_file.display().to_string()),
				"the reminder must name the goal's plan: {reminder}"
			);
			h.responder.abort();
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
async fn approving_a_plan_without_the_goal_harness_makes_no_goal() {
	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let h = approving_actor(false).await;
			let deferred = approve_plan(&h.actor).await;
			assert!(h.actor.goal_tracker.lock().snapshot().is_none());
			assert!(deferred.is_empty(), "no goal, so no goal reminder");
			h.responder.abort();
		})
		.await;
}

#[tokio::test(flavor = "current_thread")]
async fn approving_a_plan_never_replaces_an_active_goal() {
	let local = tokio::task::LocalSet::new();
	local
		.run_until(async {
			let h = approving_actor(true).await;
			let running = h
				.actor
				.create_goal_orchestration("ship the exporter", None)
				.await;
			let deferred = approve_plan(&h.actor).await;
			{
				let tracker = h.actor.goal_tracker.lock();
				let goal = tracker.snapshot().unwrap();
				assert_eq!(goal.goal_id, running);
				assert_eq!(goal.objective, "ship the exporter");
				assert!(goal.plan_file.is_none());
			}
			assert!(deferred.is_empty());
			h.responder.abort();
		})
		.await;
}
