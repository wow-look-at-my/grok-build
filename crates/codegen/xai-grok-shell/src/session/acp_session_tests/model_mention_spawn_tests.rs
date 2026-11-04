//! `@sonnet`, `@haiku` and `@opus` reach the subagent coordinator as a background spawn on the
//! newest model of the family, with the typed effort.

use super::support::*;
use super::*;
use xai_grok_tools::implementations::grok_build::task::types::{SubagentEvent, SubagentRequest};

fn catalog_entry(model_id: &str) -> crate::agent::config::ModelEntry {
    crate::agent::config::ModelEntry {
        info: crate::agent::config::ModelInfo {
            user_selectable: true,
            supported_in_api: true,
            model: model_id.to_string(),
            context_window: std::num::NonZeroU64::new(128_000).unwrap(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            ..Default::default()
        },
        mtls_cert_dir: None,
        api_key: Some("test-key".to_string()),
        env_key: None,
        auth_provider: None,
        api_base_url: None,
    }
}

async fn actor_with_catalog(ids: &[&str], tmp: &tempfile::TempDir) -> SessionActor {
    let (gateway_tx, _grx) =
        tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
    let (persistence_tx, _prx) = tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
    let mut actor = create_test_actor(0, 256_000, 85, gateway_tx, persistence_tx).await;
    let mut catalog = indexmap::IndexMap::new();
    for id in ids {
        catalog.insert((*id).to_string(), catalog_entry(id));
    }
    actor.models_manager = crate::agent::remote_config::ModelsManager::new(
        None,
        catalog,
        agent_client_protocol::ModelId::new("claude-opus-5-5"),
        std::sync::Arc::new(xai_grok_login::AuthManager::new(
            tmp.path(),
            Default::default(),
        )),
        crate::agent::config::Config::default(),
    );
    actor
}

/// A coordinator that records each spawn and reports it registered. It holds the result
/// sender, as a running child would.
fn recording_coordinator(
    actor: &mut SessionActor,
) -> std::rc::Rc<std::cell::RefCell<Vec<SubagentRequest>>> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SubagentEvent>();
    actor.tool_context.subagent_event_tx = Some(tx);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = seen.clone();
    xai_spawn::spawn_local(async move {
        let mut running = Vec::new();
        while let Some(event) = rx.recv().await {
            if let SubagentEvent::Spawn(mut spawn) = event {
                spawn.notify_registered();
                let SubagentSpawnRequestParts { request, result_tx } = split(spawn);
                sink.borrow_mut().push(request);
                running.push(result_tx);
            }
        }
    });
    seen
}

struct SubagentSpawnRequestParts {
    request: SubagentRequest,
    result_tx: tokio::sync::oneshot::Sender<
        xai_grok_tools::implementations::grok_build::task::types::SubagentResult,
    >,
}

fn split(
    spawn: xai_grok_tools::implementations::grok_build::task::types::SubagentSpawnRequest,
) -> SubagentSpawnRequestParts {
    SubagentSpawnRequestParts {
        request: *spawn.request,
        result_tx: spawn.result_tx,
    }
}

const CATALOG: &[&str] = &[
    "claude-sonnet-4-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
    "claude-opus-5-5",
    "grok-4",
];

#[tokio::test(flavor = "current_thread")]
async fn each_family_spawns_a_background_child_on_its_newest_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut actor = actor_with_catalog(CATALOG, &tmp).await;
            let seen = recording_coordinator(&mut actor);

            for (family, effort, model) in [
                ("sonnet", Some("high"), "claude-sonnet-5-5"),
                ("haiku", None, "claude-haiku-5-5"),
                ("opus", Some("max"), "claude-opus-5-5"),
            ] {
                let spawned = actor
                    .handle_spawn_model_agent(family, effort, "fix the flaky test")
                    .await
                    .unwrap_or_else(|e| panic!("@{family} must spawn: {e}"));
                assert_eq!(spawned.model_id, model);

                let seen = seen.borrow();
                let request = seen.last().expect("the coordinator got the spawn");
                assert_eq!(request.id, spawned.subagent_id);
                assert_eq!(request.prompt, "fix the flaky test");
                assert_eq!(request.runtime_overrides.model.as_deref(), Some(model));
                assert_eq!(
                    request.runtime_overrides.reasoning_effort.as_deref(),
                    effort
                );
                assert!(request.run_in_background);
                assert!(request.surface_completion);
                assert_eq!(request.parent_prompt_id, None);
                assert_eq!(request.subagent_type, "general-purpose");
            }
            assert_eq!(seen.borrow().len(), 3);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_family_with_no_model_fails_without_a_spawn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut actor = actor_with_catalog(&["claude-sonnet-5-5", "grok-4"], &tmp).await;
            let seen = recording_coordinator(&mut actor);

            let err = actor
                .handle_spawn_model_agent("opus", None, "review it")
                .await
                .expect_err("no opus model is in the catalog");
            assert!(err.contains("No opus model is available"), "{err}");
            assert!(err.contains("claude-sonnet-5-5"), "{err}");
            assert!(seen.borrow().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_bad_level_or_empty_task_fails_without_a_spawn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut actor = actor_with_catalog(CATALOG, &tmp).await;
            let seen = recording_coordinator(&mut actor);

            let err = actor
                .handle_spawn_model_agent("sonnet", Some("turbo"), "go")
                .await
                .expect_err("turbo is not an effort level");
            assert!(err.contains("turbo"), "{err}");
            let err = actor
                .handle_spawn_model_agent("sonnet", None, "   ")
                .await
                .expect_err("a mention needs a task");
            assert!(err.contains("needs a task"), "{err}");
            assert!(seen.borrow().is_empty());
        })
        .await;
}
