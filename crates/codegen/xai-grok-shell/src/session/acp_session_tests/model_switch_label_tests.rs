use xai_grok_sampling_types::conversation::ConversationItem;

use super::support::{build_actor, running_task_stub};
use crate::session::compaction_config::AsyncCompactionCache;

const SWITCH_TARGET_LABEL: &str = "Aurora";

fn head_text(conv: &[ConversationItem]) -> String {
    let Some(ConversationItem::System(sys)) = conv.first() else {
        panic!(
            "conversation must start with a System item, got {:?}",
            conv.first()
        );
    };
    sys.content.as_ref().to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn model_switch_relabels_live_agent_and_system_head() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _gateway_rx) = build_actor().await;
            actor.chat_state_handle.replace_conversation(vec![
                ConversationItem::system("spawn-time prompt"),
                ConversationItem::user("hi"),
            ]);

            actor
                .handle_set_session_model(crate::session::SessionModelSwitch {
                    sampling_config: xai_grok_sampler::SamplerConfig {
                        model: "switch-target".to_owned(),
                        context_window: 256_000,
                        ..xai_grok_sampler::SamplerConfig::default()
                    },
                    use_concise: false,
                    is_family_switch: false,
                    apply_prompt_override: true,
                    skip_prompt_rewrite: false,
                    auto_compact_threshold_percent: 85,
                    system_prompt_label: SWITCH_TARGET_LABEL.to_owned(),
                })
                .await
                .expect("model switch succeeds");

            let conv = actor.chat_state_handle.get_conversation().await;
            let head = head_text(&conv);
            assert!(
                head.contains(SWITCH_TARGET_LABEL),
                "system head must render the new model's label, got: {head:.120}"
            );
            let agent = actor.agent.borrow();
            assert_eq!(
                SWITCH_TARGET_LABEL,
                agent.prompt_context().system_prompt_label
            );
            assert_eq!(agent.system_prompt(), head);
            assert_eq!(2, conv.len(), "the switch swaps only the head");
        })
        .await;
}

/// A switch while a turn runs must reach the turn's next request. Each tool
/// round builds its request from chat state and pushes
/// `reconstruct_full_config` to the sampler, so both must name the new model.
#[tokio::test(flavor = "current_thread")]
async fn model_switch_mid_turn_reaches_the_next_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _gateway_rx) = build_actor().await;
            actor.state.lock().await.running_task = Some(running_task_stub("running"));
            let before = actor.reconstruct_full_config().await.model;
            assert_ne!(before, "switch-target");

            actor
                .handle_set_session_model(crate::session::SessionModelSwitch {
                    sampling_config: xai_grok_sampler::SamplerConfig {
                        model: "switch-target".to_owned(),
                        context_window: 256_000,
                        ..xai_grok_sampler::SamplerConfig::default()
                    },
                    use_concise: false,
                    is_family_switch: false,
                    apply_prompt_override: true,
                    skip_prompt_rewrite: false,
                    auto_compact_threshold_percent: 85,
                    system_prompt_label: SWITCH_TARGET_LABEL.to_owned(),
                })
                .await
                .expect("model switch succeeds while a turn runs");

            assert_eq!(
                actor.reconstruct_full_config().await.model,
                "switch-target",
                "the sampler config of the next round must name the new model"
            );
            let request = actor
                .chat_state_handle
                .build_request(vec![], None, false, None, "s".to_owned(), "r".to_owned())
                .await
                .expect("chat state actor alive");
            assert_eq!(
                request.model.as_deref(),
                Some("switch-target"),
                "the next request of the running turn must name the new model"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn relabel_skipped_mid_turn_keeps_prefire_cache() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _gateway_rx) = build_actor().await;
            actor.compaction.prefire.store(AsyncCompactionCache {
                note1: "NOTE1".to_owned(),
                prefix_len: 1,
                fingerprint: 42,
                model_slug: "switch-target".to_owned(),
                pass1_latency_ms: 5,
            });
            actor.state.lock().await.running_task = Some(running_task_stub("running"));

            actor
                .relabel_agent_system_prompt(SWITCH_TARGET_LABEL.to_owned())
                .await;

            assert!(
                actor.compaction.prefire.has_cache(),
                "a skipped relabel must not drop a prefire built on the still-live prompt"
            );
            assert_eq!(
                xai_grok_agent::DEFAULT_SYSTEM_PROMPT_LABEL,
                actor.agent.borrow().prompt_context().system_prompt_label
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn zero_turn_rebuild_renders_switch_target_label() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _gateway_rx) = build_actor().await;

            actor
                .handle_rebuild_agent_for_definition(
                    xai_grok_agent::AgentDefinition::default_grok_build(),
                    true,
                    SWITCH_TARGET_LABEL.to_owned(),
                )
                .await
                .expect("zero-turn rebuild succeeds");

            let conv = actor.chat_state_handle.get_conversation().await;
            let head = head_text(&conv);
            assert!(
                head.contains(SWITCH_TARGET_LABEL),
                "rebuilt head must render the new model's label, got: {head:.120}"
            );
            let agent = actor.agent.borrow();
            assert_eq!(
                SWITCH_TARGET_LABEL,
                agent.prompt_context().system_prompt_label
            );
            assert_eq!(agent.system_prompt(), head);
        })
        .await;
}
