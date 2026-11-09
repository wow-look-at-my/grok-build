//! A prompt that starts spawns a model-family subagent at once.

use super::*;
use pretty_assertions::assert_eq;

fn assert_spawn(effects: &[Effect], family: &str, effort: Option<&str>, prompt: &str) {
    assert!(
        matches!(
            effects,
            [Effect::SpawnModelAgent { family: f, effort: e, prompt: p, .. }]
                if f == family && e.as_deref() == effort && p == prompt
        ),
        "expected one @{family} spawn ({effort:?}, {prompt:?}), got {effects:?}"
    );
}

#[test]
fn each_family_mention_spawns_its_subagent() {
    for family in ["sonnet", "haiku", "opus"] {
        let mut app = test_app_with_agent();
        let effects = dispatch(
            Action::SendPrompt(format!("@{family} high fix the flaky test")),
            &mut app,
        );
        assert_spawn(&effects, family, Some("high"), "fix the flaky test");
        assert_eq!("", test_agent(&app, AgentId(0)).prompt.text());
    }
}

#[test]
fn the_level_is_optional() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::SendPrompt("@opus review the diff".to_owned()),
        &mut app,
    );
    assert_spawn(&effects, "opus", None, "review the diff");
}

#[test]
fn a_mention_while_a_turn_runs_bypasses_the_queue() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().session.state = AgentState::TurnRunning;

    let effects = dispatch(
        Action::SendPrompt("@haiku low list the crates".to_owned()),
        &mut app,
    );

    assert_spawn(&effects, "haiku", Some("low"), "list the crates");
    assert!(test_agent(&app, id).session.pending_prompts.is_empty());
}

#[test]
fn history_keeps_the_typed_mention() {
    let mut app = test_app_with_agent();
    dispatch(
        Action::SendPrompt("@sonnet max write the docs".to_owned()),
        &mut app,
    );
    assert_eq!(
        vec!["@sonnet max write the docs".to_owned()],
        test_agent(&app, AgentId(0)).session.prompt_history
    );
}

#[test]
fn a_file_mention_is_still_a_prompt() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::SendPrompt("@sonnet-notes.md summarize this".to_owned()),
        &mut app,
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::SpawnModelAgent { .. })),
        "{effects:?}"
    );
}

#[test]
fn the_shell_answer_is_reported_in_the_transcript() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    crate::app::dispatch::notes::handle_model_agent_spawned(
        &mut app,
        id,
        "sonnet",
        Ok("claude-sonnet-5-5".to_owned()),
    );
    crate::app::dispatch::notes::handle_model_agent_spawned(
        &mut app,
        id,
        "opus",
        Err("No opus model is available.".to_owned()),
    );
    let text: String = app.agents[&id]
        .scrollback
        .iter_entries()
        .filter_map(|(_, entry)| match &entry.block {
            RenderBlock::System(b) => Some(format!("{}\n", b.text)),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("Started a background @sonnet subagent on claude-sonnet-5-5."),
        "{text}"
    );
    assert!(
        text.contains("Couldn't start the @opus subagent: No opus model is available."),
        "{text}"
    );
}
