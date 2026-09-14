//! A slash command submitted while the agent cannot run it immediately — a turn
//! is running, or rows are already queued — must still execute as a command.
//!
//! Every test here drives the shipped dispatch entry points from the state the
//! user actually reaches (`Action::SendPrompt` on a running turn, the bare-Enter
//! interrupt, the send-now chord, the queue-pane send-now and the local drain)
//! and asserts on the effects those entries produce.

use super::*;
use crate::app::agent::{QueueEntryKind, QueuedPrompt};
use crate::app::agent_view::test_fixtures::test_pasted_image;
use crate::app::app_view::InputOutcome;
use crate::app::prompt_queue::QueueEntryWire;
use crate::views::queue_pane::wire_row_is_steering_text;

/// Advertise `name` the way a session's registry sync does for a SHELL-owned
/// command: no skill `_meta`, so the pager resolves it to `PassThrough` and the
/// shell executes it when the prompt's own turn starts.
fn register_shell_command(app: &mut AppView, id: AgentId, name: &str) {
    let agent = app.agents.get_mut(&id).unwrap();
    let models = agent.session.models.clone();
    agent.prompt.sync_acp_commands(
        &[acp::AvailableCommand::new(
            name.to_string(),
            format!("{name} command"),
        )],
        None,
        &models,
    );
}

fn server_row(id: &str, text: &str, position: usize) -> QueueEntryWire {
    QueueEntryWire {
        id: id.into(),
        version: 0,
        owner: None,
        last_editor: None,
        kind: "prompt".into(),
        text: text.into(),
        combined_texts: None,
        position,
    }
}

fn running_agent_with_a_queued_message(app: &mut AppView, id: AgentId) {
    let agent = app.agents.get_mut(&id).unwrap();
    agent.session.state = AgentState::TurnRunning;
    // The report's precondition: something is already queued, so the command
    // cannot be sent immediately.
    agent.shared_queue = vec![server_row("srv-1", "an earlier message", 0)];
}

/// Every text payload a dispatched effect would hand to the model (or to the
/// shell as a prompt). The literal `/cmd args` must never appear here.
fn model_bound_payloads(effects: &[Effect]) -> Vec<String> {
    let mut out = Vec::new();
    for effect in effects {
        match effect {
            Effect::SendPrompt { text, .. }
            | Effect::SendBashCommand { command: text, .. }
            | Effect::SetModeThenPrompt { text, .. }
            | Effect::SendInterject { text, .. } => out.push(text.clone()),
            Effect::SendPromptBlocks { blocks, .. } | Effect::SendPromptNow { blocks, .. } => {
                out.extend(blocks.iter().filter_map(|block| match block {
                    acp::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                }));
            }
            _ => {}
        }
    }
    out
}

fn local_rows(app: &AppView, id: AgentId) -> Vec<(QueueEntryKind, String)> {
    app.agents[&id]
        .session
        .pending_prompts
        .iter()
        .map(|p| (p.kind, p.text.clone()))
        .collect()
}

fn local_texts(app: &AppView, id: AgentId) -> Vec<String> {
    local_rows(app, id).into_iter().map(|(_, t)| t).collect()
}

fn text_block(text: &str) -> acp::ContentBlock {
    acp::ContentBlock::Text(acp::TextContent::new(text.to_string()))
}

// ── the classification ────────────────────────────────────────────────────

/// The predicate every delivery path consults reads a line the same way the
/// submit path does: a leading `/name` is a command; a bare `/`, a lone `/ `,
/// or a `/` mid-sentence is ordinary text.
#[test]
fn slash_invocation_classification_matches_the_submit_path() {
    for text in [
        "/plan",
        "/plan implement the auth flow",
        "/compact keep the auth notes",
        "  /model grok-4  ",
        "/TODO jump the queue",
    ] {
        assert!(
            crate::slash::is_slash_invocation(text),
            "{text:?} is a command line and the submit path treats it as one"
        );
    }
    for text in [
        "",
        "   ",
        "/",
        "/ ",
        "/\t",
        "hello",
        "hello /plan implement it",
        "!ls -la",
        "https://example.com/x",
    ] {
        assert!(
            !crate::slash::is_slash_invocation(text),
            "{text:?} is ordinary text, so the submit path would send it as a prompt"
        );
    }
}

/// Which rows own their turn, and which may travel as steering text.
///
/// `wire_blocks` excluded because a client-expanded payload has already
/// replaced the command text with what the model must see; images make no
/// difference, because a `/gboom stats` row can carry a pasted image and is
/// still a command; `own_turn` covers the `/plan <description>` description.
#[test]
fn only_payload_free_slash_rows_own_their_turn() {
    let slash = QueuedPrompt::plain(1, "/plan implement it", QueueEntryKind::Prompt);
    assert!(slash.is_slash_command());
    assert!(slash.owns_its_turn());
    assert!(!slash.is_steering_text());

    let plain = QueuedPrompt::plain(2, "look at this instead", QueueEntryKind::Prompt);
    assert!(!plain.is_slash_command());
    assert!(plain.is_steering_text());

    let expanded = QueuedPrompt {
        wire_blocks: Some(vec![text_block("<skill_information/>")]),
        ..QueuedPrompt::plain(3, "/imagine a cat", QueueEntryKind::Prompt)
    };
    assert!(!expanded.is_slash_command(), "the payload replaced the text");
    assert!(!expanded.is_steering_text(), "client-expanded payload");

    let with_image = QueuedPrompt {
        images: vec![test_pasted_image()],
        ..QueuedPrompt::plain(4, "/gboom stats", QueueEntryKind::Prompt)
    };
    assert!(with_image.is_slash_command(), "an attachment is not a reply");

    let own = QueuedPrompt {
        own_turn: true,
        ..QueuedPrompt::plain(5, "implement the auth flow", QueueEntryKind::Prompt)
    };
    assert!(!own.is_slash_command());
    assert!(own.owns_its_turn());
    assert!(!own.is_steering_text());

    let command = QueuedPrompt::plain(6, "/compact", QueueEntryKind::Command);
    assert!(!command.is_steering_text(), "a command row owns its turn");
}

/// A server row is judged the same way: the shell's own harvest leaves a slash
/// invocation alone, so an interrupt must not count it as deliverable.
#[test]
fn server_rows_carry_the_same_rule() {
    assert!(wire_row_is_steering_text(&server_row("s", "read this", 0)));
    assert!(!wire_row_is_steering_text(&server_row(
        "s",
        "/pr-cleanup fix the branch",
        0
    )));
    let mut bash = server_row("s", "ls -la", 0);
    bash.kind = "bash".into();
    assert!(!wire_row_is_steering_text(&bash));
}

// ── the delivery routes ───────────────────────────────────────────────────

/// Gating (1): a shell command submitted while a turn is running and a message
/// is already queued stays in the LOCAL queue. Handing it to the shell as a
/// plain prompt is what put the literal `/pr-cleanup fix the branch` in front of
/// the model: the running turn harvests queued rows into itself as text, and the
/// shell resolves a command only when the prompt's own turn starts.
#[test]
fn a_queued_slash_command_is_not_hoisted_to_the_shell_as_plain_text() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    register_shell_command(&mut app, id, "pr-cleanup");
    running_agent_with_a_queued_message(&mut app, id);

    let submitted = dispatch(Action::SendPrompt("/pr-cleanup fix the branch".into()), &mut app);
    assert_eq!(
        model_bound_payloads(&submitted),
        Vec::<String>::new(),
        "submitting a command sends nothing to the model: {submitted:?}"
    );
    assert_eq!(
        local_texts(&app, id),
        vec!["/pr-cleanup fix the branch".to_string()],
        "the command waits in the local queue for its own turn"
    );

    // Any later submit (an inbound update does the same) runs the migration
    // that hands leading plain rows to the shell.
    let later = dispatch(Action::SendPrompt("and one more".into()), &mut app);
    assert!(
        model_bound_payloads(&later).is_empty(),
        "the command row blocks the migration instead of travelling as text: {later:?}"
    );
    assert_eq!(
        local_texts(&app, id),
        vec![
            "/pr-cleanup fix the branch".to_string(),
            "and one more".to_string()
        ],
        "both rows stay local"
    );

    // Once the turn ends the command runs as its own turn's prompt, where the
    // shell resolves it — alone. Merging it into a neighbour's turn would put
    // the command line mid-body, where `resolve` never looks (`combine` merely
    // joins the texts), so the row behind it must be untouched too.
    let agent = app.agents.get_mut(&id).unwrap();
    agent.session.state = AgentState::Idle;
    agent.session.current_prompt_id = None;
    agent.shared_queue.clear();
    let drained = dispatch(Action::DrainQueue, &mut app);
    assert!(
        matches!(
            drained.as_slice(),
            [Effect::SendPrompt { text, .. }] if text == "/pr-cleanup fix the branch"
        ),
        "the command drains as its own turn, unmerged: {drained:?}"
    );
    assert_eq!(
        local_texts(&app, id),
        vec!["and one more".to_string()],
        "the following plain row keeps its own turn"
    );
}

/// Gating (2), the objective's literal case: `/plan <description>` submitted
/// mid-turn enters plan mode AND keeps the description for the following turn.
/// Neither the description nor `/plan <description>` may be folded into the
/// running turn as steering text — the plan mode this submit switched on
/// belongs to the NEXT turn.
#[test]
fn plan_description_submitted_mid_turn_waits_for_the_next_turn() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    running_agent_with_a_queued_message(&mut app, id);

    let effects = dispatch(
        Action::SendPrompt("/plan implement the auth flow".into()),
        &mut app,
    );

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetSessionMode { mode_id, .. }] if &*mode_id.0 == "plan"
        ),
        "plan mode is entered now: {effects:?}"
    );
    assert_eq!(app.agents[&id].plan_mode_pending, Some(true));
    assert_eq!(model_bound_payloads(&effects), Vec::<String>::new());
    assert_eq!(
        local_texts(&app, id),
        vec!["implement the auth flow".to_string()],
        "the description is queued, not dropped"
    );

    // Nothing may hoist it into the running turn: neither the migration a later
    // submit triggers nor the interrupt-with-queue gesture.
    let migrated = dispatch(Action::SendPrompt("and one more".into()), &mut app);
    assert_eq!(
        model_bound_payloads(&migrated),
        Vec::<String>::new(),
        "the description must not be handed to the shell mid-turn: {migrated:?}"
    );
    let interrupted = dispatch(Action::InterruptWithQueuedPrompts, &mut app);
    for payload in model_bound_payloads(&interrupted) {
        assert!(
            !payload.contains("implement the auth flow"),
            "the description must not be interjected into the running turn: {interrupted:?}"
        );
    }
    assert_eq!(
        local_texts(&app, id),
        vec!["implement the auth flow".to_string()],
        "the description is the one row that waits; the plain message was delivered"
    );

    // The turn ends: the description runs as the following (plan-mode) turn.
    let agent = app.agents.get_mut(&id).unwrap();
    agent.session.state = AgentState::Idle;
    agent.session.current_prompt_id = None;
    agent.shared_queue.clear();
    let drained = dispatch(Action::DrainQueue, &mut app);
    assert!(
        matches!(
            drained.as_slice(),
            [Effect::SendPrompt { text, .. }] if text == "implement the auth flow"
        ),
        "the following turn's prompt is the description: {drained:?}"
    );
}

/// Gating (3a), send-now chord: `/plan <description>` typed mid-turn and
/// force-sent must RUN the command. Handing the text to the shell as a `sendNow`
/// prompt does not: the shell knows no `plan` command and would give the model
/// the literal `/plan <description>`.
#[test]
fn send_now_on_a_pager_command_runs_the_command() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    running_agent_with_a_queued_message(&mut app, id);

    let effects = dispatch(
        Action::SendPromptNow {
            text: "/plan implement the auth flow".into(),
            images: vec![],
            wire_blocks: None,
        },
        &mut app,
    );

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetSessionMode { mode_id, .. }] if &*mode_id.0 == "plan"
        ),
        "the command ran instead of becoming a prompt: {effects:?}"
    );
    assert_eq!(app.agents[&id].plan_mode_pending, Some(true));
    assert_eq!(
        local_texts(&app, id),
        vec!["implement the auth flow".to_string()]
    );
}

/// Gating (3a, with an attachment): the chord's producer drains the pasted
/// image along with the text, so the command is force-sent carrying one. The
/// command must still run — the literal `/plan <description>` never becomes a
/// prompt — and the attachment must land somewhere real: on the row the
/// command queued, exactly as pressing Enter with that image does.
#[test]
fn send_now_on_a_pager_command_with_an_image_runs_the_command() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    running_agent_with_a_queued_message(&mut app, id);

    let effects = dispatch(
        Action::SendPromptNow {
            text: "/plan implement the auth flow".into(),
            images: vec![test_pasted_image()],
            wire_blocks: None,
        },
        &mut app,
    );

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetSessionMode { mode_id, .. }] if &*mode_id.0 == "plan"
        ),
        "the command ran instead of becoming a prompt: {effects:?}"
    );
    assert_eq!(app.agents[&id].plan_mode_pending, Some(true));
    assert_eq!(
        local_texts(&app, id),
        vec!["implement the auth flow".to_string()],
        "the description is the following turn's prompt"
    );
    assert_eq!(
        app.agents[&id].session.pending_prompts[0].images.len(),
        1,
        "the pasted image rides with the description instead of being dropped"
    );
    assert!(
        app.agents[&id].toast.is_none(),
        "nothing was dropped, so nothing is reported as dropped"
    );
}

/// Gating (3b), queue-pane send-now of a command row: the row leaves the queue
/// and the command runs. Its text never becomes a `sendNow` prompt, so a
/// pager-owned command cannot reach the model as `/plan <description>`.
#[test]
fn send_now_of_a_queued_pager_command_runs_the_command() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    running_agent_with_a_queued_message(&mut app, id);
    // A row can hold command text this client did not resolve: queued before the
    // registry synced, edited into a command, or left by an older client. The
    // delivery rule must hold whatever the provenance.
    enqueue_local(&mut app, id, "/plan implement the auth flow");
    let row = app.agents[&id].session.pending_prompts[0].id;

    let outcome = app
        .agents
        .get_mut(&id)
        .unwrap()
        .force_interject_queue_row(row);
    let InputOutcome::Action(action) = outcome else {
        panic!("send-now of a prompt-kind row is an action, got {outcome:?}");
    };
    let effects = dispatch(action, &mut app);

    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetSessionMode { mode_id, .. }] if &*mode_id.0 == "plan"
        ),
        "the queued command ran instead of being sent as text: {effects:?}"
    );
    assert_eq!(app.agents[&id].plan_mode_pending, Some(true));
    assert_eq!(
        local_texts(&app, id),
        vec!["implement the auth flow".to_string()],
        "the row was consumed and the description queued"
    );
}

/// Gating (3c), bare Enter on the empty composer with a command queued: the
/// interrupt delivers the rows it can fold into the turn and leaves the command
/// queued. Sending the command as an interjection is what put `/pr-cleanup …`
/// in the model's context as user text.
#[test]
fn bare_enter_leaves_a_queued_command_to_its_own_turn() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    register_shell_command(&mut app, id, "pr-cleanup");
    running_agent_with_a_queued_message(&mut app, id);
    dispatch(Action::SendPrompt("/pr-cleanup fix the branch".into()), &mut app);

    let outcome = app
        .agents
        .get_mut(&id)
        .unwrap()
        .try_interrupt_with_queued_from_prompt()
        .expect("a queued row makes Enter a send gesture");
    let InputOutcome::Action(action) = outcome else {
        panic!("expected an interrupt action, got {outcome:?}");
    };
    let effects = dispatch(action, &mut app);

    for payload in model_bound_payloads(&effects) {
        assert!(
            !payload.contains("/pr-cleanup"),
            "the command is not steering text: {effects:?}"
        );
    }
    assert!(
        local_texts(&app, id).contains(&"/pr-cleanup fix the branch".to_string()),
        "the command is still queued for its own turn: {:?}",
        local_texts(&app, id)
    );
}

/// Gating (3d): a SHELL-advertised command keeps the send-now route. The shell
/// resolves it when the prompt's own turn starts, so force-sending it must stay
/// immediate rather than being turned into a queued command — and the payload is
/// a prompt the shell consumes, never text the model reads.
#[test]
fn send_now_on_a_shell_command_keeps_the_immediate_route() {    let mut app = test_app_with_agent();
    let id = AgentId(0);
    register_shell_command(&mut app, id, "pr-cleanup");
    running_agent_with_a_queued_message(&mut app, id);

    let effects = dispatch(
        Action::SendPromptNow {
            text: "/pr-cleanup fix the branch".into(),
            images: vec![],
            wire_blocks: None,
        },
        &mut app,
    );

    assert!(
        matches!(effects.as_slice(), [Effect::SendPromptNow { .. }]),
        "a shell-resolved command still cancels-and-sends: {effects:?}"
    );
    assert_eq!(
        local_texts(&app, id),
        Vec::<String>::new(),
        "it is not queued: the shell runs it now"
    );
}

/// Regression (4): the unaffected paths keep their behavior. `/plan <desc>` on
/// an idle session bundles the mode switch with the prompt; `/compact` queues as
/// a command row and is never interjected; a plain prompt sends immediately; a
/// plain prompt typed mid-turn is still delivered to the running turn.
#[test]
fn unaffected_paths_keep_their_routing() {
    // Idle `/plan <desc>`: mode switch + prompt in one ordered effect.
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::SendPrompt("/plan add auth to the app".into()),
        &mut app,
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SetModeThenPrompt { mode_id, text, .. }]
                if &*mode_id.0 == "plan" && text == "add auth to the app"
        ),
        "{effects:?}"
    );

    // Idle plain prompt: immediate send, nothing queued.
    let mut app = test_app_with_agent();
    let effects = dispatch(Action::SendPrompt("go now".into()), &mut app);
    assert!(matches!(
        effects.as_slice(),
        [Effect::SendPrompt { text, .. }] if text == "go now"
    ));

    // `/compact` mid-turn: a Command row, honored when the turn ends.
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    running_agent_with_a_queued_message(&mut app, id);
    let effects = dispatch(
        Action::SendPrompt("/compact keep the auth notes".into()),
        &mut app,
    );
    assert!(effects.is_empty(), "a command is not sent now: {effects:?}");
    assert_eq!(
        local_rows(&app, id),
        vec![(
            QueueEntryKind::Command,
            "/compact keep the auth notes".to_string()
        )]
    );
    let interrupted = dispatch(Action::InterruptWithQueuedPrompts, &mut app);
    assert!(
        !model_bound_payloads(&interrupted)
            .iter()
            .any(|text| text.contains("/compact")),
        "a command row is never folded into a turn: {interrupted:?}"
    );
    let agent = app.agents.get_mut(&id).unwrap();
    agent.session.state = AgentState::Idle;
    agent.session.current_prompt_id = None;
    agent.shared_queue.clear();
    let drained = dispatch(Action::DrainQueue, &mut app);
    assert!(
        matches!(drained.as_slice(), [Effect::Compact { .. }]),
        "the queued command runs: {drained:?}"
    );

    // Plain prompt mid-turn: still delivered to the running turn ASAP.
    let mut app = test_app_with_agent();
    dispatch(Action::SendPrompt("first".into()), &mut app);
    let effects = dispatch(Action::SendPrompt("read this next".into()), &mut app);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::SendPrompt { text, .. }] if text == "read this next"
        ),
        "a plain mid-turn prompt still reaches the shell queue: {effects:?}"
    );
}
