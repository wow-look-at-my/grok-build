//! Voice mode enable, toggle, and stop dispatchers.

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView, VoiceState, VoiceTarget};

/// Promote live interim into the bound prompt, then hard-reset (no trailing final).
/// Returns the promoted fragment and its caret for callers that captured text earlier.
pub(super) fn voice_stop_on_submit(app: &mut AppView) -> Option<crate::voice::VoiceInterimCommit> {
    let interim = crate::voice::commit_interim_into_prompt(app);
    app.voice_reset();
    interim
}

/// Merge interim into a payload captured before [`voice_stop_on_submit`].
/// The fragment lands at the caret it was committed at, matching where a final would insert.
pub(super) fn merge_prompt_with_voice_interim(
    existing: String,
    interim: Option<crate::voice::VoiceInterimCommit>,
) -> String {
    match interim {
        Some(commit) => {
            crate::voice::merge_voice_fragment(&existing, commit.replace, &commit.fragment)
        }
        None => existing,
    }
}

/// The prompt box dictation should target for the current view.
/// That is a top-level row's peek reply when one is open, the new-agent dispatch input otherwise, or the active agent's prompt.
/// A non-top-level peek (subagent / roster, which can't accept a reply) maps to the dispatch box.
fn voice_target_for_view(app: &AppView) -> Option<VoiceTarget> {
    use crate::views::dashboard::DashboardRowId;
    match app.active_view {
        ActiveView::Agent(id) => Some(VoiceTarget::Agent(id)),
        ActiveView::AgentDashboard => {
            let dashboard = app.dashboard.as_ref();
            // The attached-agent popup hides the dispatch/peek inputs; don't bind dictation to a box the user can't see (no overlay, finals lost)
            if dashboard.is_some_and(|d| d.attached_agent.is_some()) {
                return None;
            }
            Some(
                match dashboard.and_then(|d| d.peek.as_ref()).map(|p| &p.row) {
                    Some(DashboardRowId::TopLevel(id)) => VoiceTarget::DashboardPeekReply(*id),
                    _ => VoiceTarget::DashboardDispatch,
                },
            )
        }
        _ => None,
    }
}

/// That keybinding bypasses the slash registry (`/voice` is instead hidden and upsold via the deny list).
/// Elsewhere (e.g. the welcome screen, which has no agent to host a modal) it is a silent no-op.
/// Never starts voice; always returns no effects.
fn open_voice_tier_upsell(app: &mut AppView) -> Vec<Effect> {
    let login_method = app.login_method_id.as_ref().map(|id| id.0.to_string());
    match app.active_view {
        ActiveView::Agent(id) => {
            if let Some(agent) = app.agents.get_mut(&id) {
                super::billing::open_restricted_command_upsell(agent, login_method);
            }
        }
        ActiveView::AgentDashboard => {
            if let Some(d) = app.dashboard.as_mut() {
                d.set_error_toast(&format!(
                    "/voice requires SuperGrok: upgrade at {}",
                    super::billing::UPSELL_URL_UPGRADE
                ));
            }
        }
        _ => {}
    }
    vec![]
}

/// When the flag is off this is a **silent no-op** with no toast; users who don't have the feature see nothing.
/// A build without audio capture (only the Bazel test build; every shipped binary compiles `audio` in)
/// The matching Ctrl+Space release (see [`dispatch_voice_stop`]) then ends *this* session and only this one.
pub(super) fn dispatch_enable_voice_mode(app: &mut AppView, from_hold: bool) -> Vec<Effect> {
    if !app.voice_mode_enabled {
        return vec![];
    }
    // Tier gate: free / X Basic personal users can't use voice (the server
    // zero-limits these tiers).
    if app.is_voice_tier_restricted() {
        return open_voice_tier_upsell(app);
    }
    // Leave home after the flag / tier gates so a disabled or restricted press stays a no-op.
    let effects = super::session::lifecycle::leave_welcome_for_session(app);
    if !xai_grok_voice::AUDIO_SUPPORTED {
        return effects;
    }

    // Bind the dictation target at press time (the cold-start path defers
    // capture to the event loop, where the view could have changed).
    let Some(target) = voice_target_for_view(app) else {
        return effects;
    };

    app.voice_ui_active = true;
    if app.voice_cmd_tx.is_some() {
        // Pipeline already up. Start a new recording now; if one is already
        // live leave it (and its hold-ownership) untouched.
        if !app.voice_listening() {
            app.voice_begin_recording(target, from_hold);
        }
    } else if !app.voice_state.pending_cold_start() {
        // Pipeline still spawning. Queue a cold-start, but only if one isn't
        // already pending.
        app.voice_state = VoiceState::ColdStart {
            hold: from_hold,
            target,
        };
    }
    effects
}

/// Toggle mic capture: `Ctrl+Space`, Esc (while listening), the recording-row
/// `[stop]`, and `Ctrl+Space` on terminals without key-release events.
pub(super) fn dispatch_voice_toggle(app: &mut AppView) -> Vec<Effect> {
    if app.voice_listening() {
        // Stop always succeeds, even if the remote flag or `/voice` mode flipped mid-recording
        app.voice_stop_keeping_final();
        return vec![];
    }
    // Not recording: start.
    dispatch_enable_voice_mode(app, /* from_hold */ false)
}

/// Ctrl+Space hold-to-talk key release: end the session a Ctrl+Space hold
/// started.
pub(super) fn dispatch_voice_stop(app: &mut AppView) -> Vec<Effect> {
    app.voice_hold_release();
    vec![]
}
