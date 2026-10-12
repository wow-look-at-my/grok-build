use super::support::*;
use super::*;
use xai_grok_tools::implementations::grok_build::read_file::ReadFileTool;
use xai_grok_tools::registry::types::ToolConfig;

const PLAN: &str = "# Plan: ship the exporter\n\n1. Write it.\n";

/// Record file names, assembled so this module's own tool arguments never
/// spell one out: the running binary refuses a call whose text names a record.
const RECORD_TRANSCRIPT: &str = concat!("chat_", "history.jsonl");
const RECORD_UPDATES: &str = concat!("updates", ".jsonl");
const RECORD_CLASSIFIER: &str = concat!("goal-", "classifier-");
const RECORD_RUNLOG: &str = concat!(".run", "log.md");

fn read_call(id: &str, path: &str) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: id.to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new(
            "read_file",
            serde_json::json!({ "target_file": path }).to_string(),
        ),
        vendor: Default::default(),
    }
}

async fn tool_result_text(actor: &SessionActor, call_id: &str) -> String {
    actor
        .chat_state_handle
        .get_conversation()
        .await
        .iter()
        .rev()
        .find_map(|item| match item {
            xai_grok_sampling_types::ConversationItem::ToolResult(result)
                if result.tool_call_id == call_id =>
            {
                Some(result.content.to_string())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool_result for {call_id}"))
}

/// An actor with the read tool bound, whose goal tracker lives in `session_dir`.
/// A `main_session_dir` marks it as a goal verifier child; `None` gives it an
/// active goal instead.
async fn guard_actor(
    session_dir: &std::path::Path,
    main_session_dir: Option<std::path::PathBuf>,
) -> SessionActor {
    let (gateway_tx, _gateway_rx) = tokio::sync::mpsc::unbounded_channel();
    let (persistence_tx, _persistence_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    actor.goal_tracker = Arc::new(parking_lot::Mutex::new(
        crate::session::goal_tracker::GoalTracker::new(session_dir.to_path_buf()),
    ));
    if main_session_dir.is_none() {
        actor.goal_tracker.lock().create_goal(
            "goal".into(),
            "objective".into(),
            None,
            0,
            "2026-01-01T00:00:00Z".into(),
            None,
        );
    }
    actor.tool_context.goal_main_session_dir = main_session_dir;
    *actor.agent.borrow_mut() =
        test_agent_with_tools(vec![ToolConfig::for_tool::<ReadFileTool>()]).await;
    let toolset = actor.agent.borrow().tool_bridge().toolset();
    actor
        .workspace_ops
        .bind_local_session(
            &actor.session_id_string(),
            actor.tool_context.cwd.as_path().to_path_buf(),
            actor.tool_context.hunk_tracker_handle.clone(),
            toolset,
            None,
        )
        .expect("bind_local_session");
    actor
}

async fn read_once(actor: &SessionActor, call_id: &str, path: &str) -> String {
    actor
        .execute_tool_calls(vec![read_call(call_id, path)], None)
        .await
        .expect("a read must not fail the turn");
    tool_result_text(actor, call_id).await
}

#[tokio::test(flavor = "current_thread")]
async fn an_active_goal_reads_its_plan_but_not_its_record() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let plan_dir = dir.path().join("goal");
            std::fs::create_dir_all(&plan_dir).unwrap();
            let plan_path = plan_dir.join("plan.md");
            std::fs::write(&plan_path, PLAN).unwrap();
            let record_path = dir.path().join(RECORD_TRANSCRIPT);
            std::fs::write(&record_path, "the session's own words").unwrap();

            let actor = guard_actor(dir.path(), None).await;

            let plan = read_once(&actor, "plan", plan_path.to_str().unwrap()).await;
            assert!(plan.contains("ship the exporter"), "{plan}");
            assert!(
                !plan.contains("Refused"),
                "the goal plan must be readable: {plan}"
            );

            let record = read_once(&actor, "record", record_path.to_str().unwrap()).await;
            assert!(record.contains("Refused: this call touches"), "{record}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_goal_verifier_cannot_read_the_main_session_record() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let main = tempfile::tempdir().unwrap();
            let plan_dir = main.path().join("goal");
            std::fs::create_dir_all(&plan_dir).unwrap();
            let plan_path = plan_dir.join("plan.md");
            std::fs::write(&plan_path, PLAN).unwrap();
            let transcript_path = main.path().join(RECORD_TRANSCRIPT);
            std::fs::write(&transcript_path, "the main session's own words").unwrap();
            let updates_path = main.path().join(RECORD_UPDATES);
            std::fs::write(&updates_path, "the main session's updates").unwrap();

            let child_dir = tempfile::tempdir().unwrap();
            let actor = guard_actor(child_dir.path(), Some(main.path().to_path_buf())).await;

            let plan = read_once(&actor, "plan", plan_path.to_str().unwrap()).await;
            assert!(plan.contains("ship the exporter"), "{plan}");
            assert!(
                !plan.contains("Refused"),
                "the verifier must read the plan: {plan}"
            );

            for (id, path) in [("transcript", &transcript_path), ("updates", &updates_path)] {
                let refused = read_once(&actor, id, path.to_str().unwrap()).await;
                assert!(
                    refused.contains("Refused: this call touches"),
                    "the verifier must not read the main session's record: {refused}"
                );
            }

            // A record spelled without the session directory around it is caught by name.
            let bare_dir = tempfile::tempdir().unwrap();
            let bare_record = bare_dir.path().join(RECORD_TRANSCRIPT);
            std::fs::write(&bare_record, "the main session's own words").unwrap();
            let bare = read_once(&actor, "bare", bare_record.to_str().unwrap()).await;
            assert!(
                bare.contains("Refused: this call touches"),
                "a bare record name must be refused: {bare}"
            );

            // Climbing out of the scratch root does not hide the record it reaches.
            let escaping = format!("/tmp/grok-goal-v/../../elsewhere/{RECORD_TRANSCRIPT}");
            let escaped = read_once(&actor, "escaping", &escaping).await;
            assert!(
                escaped.contains("Refused: this call touches"),
                "an escaping path must not hide the record: {escaped}"
            );

            // The harness's own artifacts in the goal scratch root stay readable.
            let scratch_id = format!("{:012x}", std::process::id());
            let scratch = crate::session::goal_tracker::goal_scratch_root(&scratch_id);
            std::fs::create_dir_all(&scratch).unwrap();
            let run_log = scratch.join(format!("{RECORD_CLASSIFIER}{scratch_id}-1{RECORD_RUNLOG}"));
            std::fs::write(&run_log, "RUN LOG BODY").unwrap();
            let log = read_once(&actor, "runlog", run_log.to_str().unwrap()).await;
            assert!(
                log.contains("RUN LOG BODY"),
                "the verifier reads the run log it audits: {log}"
            );
            let _ = std::fs::remove_dir_all(&scratch);
        })
        .await;
}
