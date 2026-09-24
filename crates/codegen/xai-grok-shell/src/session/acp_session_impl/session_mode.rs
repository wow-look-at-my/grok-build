//! Session/plan-mode concern for `SessionActor` (`handle_session_mode`, plan-mode reminders and persistence, active-template detection).
use super::*;
pub(super) fn prompt_mode_from_session_mode_id(session_mode_id: &acp::SessionModeId) -> PromptMode {
    use xai_grok_tools::types::SessionMode;
    match SessionMode::from_id(session_mode_id.0.as_ref()) {
        SessionMode::Plan => PromptMode::Plan,
        SessionMode::Ask => PromptMode::Ask,
        SessionMode::Default => PromptMode::Agent,
    }
}
/// Inverse of [`prompt_mode_from_session_mode_id`]: the mode id a client displays for a prompt mode.
/// Needed wherever a transition the client did not drive has to be reported back to it.
pub(super) fn session_mode_id_from_prompt_mode(prompt_mode: PromptMode) -> acp::SessionModeId {
    use xai_grok_tools::types::SessionMode;
    let mode = match prompt_mode {
        PromptMode::Plan => SessionMode::Plan,
        PromptMode::Ask => SessionMode::Ask,
        PromptMode::Agent => SessionMode::Default,
    };
    acp::SessionModeId::new(mode.as_id())
}
/// The agent-identity half of the session mode.
///
/// A session mode is either a permission mode (`default`, `plan`, `ask`) or an
/// agent name. An agent name swaps the whole agent, and with it the system
/// prompt and the tool registry.
#[derive(Debug, Default)]
pub(crate) struct ModeAgentState {
    /// The agent that ran before a Shift+Tab ring identity replaced it.
    ring_base: Option<String>,
    /// A swap that arrived while a turn ran. The run loop applies it at turn
    /// end. It is never dropped, because a dropped swap leaves the model under
    /// the prompt of a mode the user already left.
    pending: Option<AgentDefinition>,
}
fn is_shift_tab_ring_agent(name: &str) -> bool {
    xai_grok_agent::config::BuiltinAgentName::shift_tab_variants()
        .iter()
        .any(|v| AsRef::<str>::as_ref(v) == name)
}
/// The agent that a mode change must run, or `None` to keep the current agent.
///
/// `active` is the agent the session will run once any pending swap lands.
/// A permission mode that arrives while a ring identity is active means the
/// client left the ring. The base agent comes back then, whatever the client
/// remembers about the ring. A ring identity that the session started with
/// has no base, so it stays.
pub(super) fn mode_agent_target(
    state: &mut ModeAgentState,
    mode_id: &str,
    active: Option<&str>,
) -> Option<String> {
    let active_is_ring = active.is_some_and(is_shift_tab_ring_agent);
    if mode_id
        .parse::<xai_grok_tools::types::SessionMode>()
        .is_ok()
    {
        return if active_is_ring {
            state.ring_base.take()
        } else {
            None
        };
    }
    if !is_shift_tab_ring_agent(mode_id) {
        state.ring_base = None;
    } else if !active_is_ring {
        state.ring_base = Some(active.unwrap_or("grok-build").to_string());
    }
    Some(mode_id.to_string())
}
/// Pass-through twin: no toolset in this build carries a plan-gated tool.
pub(super) fn filter_cursor_tools_by_plan_mode(
    defs: Vec<ToolDefinition>,
    _plan_active: bool,
) -> Vec<ToolDefinition> {
    defs
}
impl SessionActor {
    pub(super) fn apply_prompt_modes_to_snapshot(&self, snapshot: &mut TurnDeltaSnapshot) {
        snapshot.start_prompt_mode = Some(self.turn_start_prompt_mode.lock().to_string());
        snapshot.end_prompt_mode = Some(self.turn_prompt_mode.lock().to_string());
    }
    /// `false` twin: this agent type is not compiled into this build, so no session runs it.
    /// Keeps ungated call sites compiling in both configurations, like [`Self::is_cursor_harness`].
    pub(super) fn is_cursor_agent(&self) -> bool {
        false
    }
    /// `false` twin: this template integration is not compiled into this build, so no session runs it.
    /// Keeps ungated call sites compiling in both configurations.
    pub(super) fn is_cursor_harness(&self) -> bool {
        false
    }
    pub(super) async fn handle_session_mode(&self, session_mode_id: acp::SessionModeId) {
        use xai_grok_tools::types::SessionMode;
        let prompt_mode = prompt_mode_from_session_mode_id(&session_mode_id);
        *self.current_prompt_mode.lock() = prompt_mode;
        let mode = SessionMode::from_id(session_mode_id.0.as_ref());
        let agent_target = {
            let active = self.active_agent_type.lock().clone();
            let mut state = self.mode_agent.lock();
            let active = state.pending.as_ref().map(|d| d.name.clone()).or(active);
            mode_agent_target(&mut state, session_mode_id.0.as_ref(), active.as_deref())
        };
        if mode.is_plan() {
            if let Some(name) = agent_target {
                self.switch_mode_agent(&name).await;
            }
            let entered = self.plan_mode.lock().enter_pending();
            if entered {
                self.persist_plan_mode_state();
                self.enqueue_current_mode_update(acp::SessionModeId::new(
                    SessionMode::Plan.as_id(),
                ));
            }
            tracing::info!(
                session_id = %self.session_info.id.0,
                entered,
                "Plan mode toggled ON (Pending)"
            );
            let turn_in_flight = self.state.lock().await.running_task.is_some();
            if entered && turn_in_flight {
                self.activate_plan_mode_mid_turn().await;
            }
            xai_grok_telemetry::session_ctx::log_event(
                xai_grok_telemetry::events::PlanModeToggled {
                    enabled: true,
                    trigger: xai_grok_telemetry::events::PlanModeTrigger::User,
                    turn_in_flight,
                    was_previously_active: !entered,
                    from_mode: Some(if entered {
                        if self.permissions.is_yolo_mode() {
                            "bypass_permissions"
                        } else {
                            "default"
                        }
                        .to_owned()
                    } else {
                        "plan".to_owned()
                    }),
                },
            );
            if entered {
                xai_grok_telemetry::event_span!(
                    "session.permission_mode_changed",
                    from_mode =
                        super::telemetry::permission_mode_label(self.permissions.is_yolo_mode()),
                    to_mode = "plan",
                    trigger = "user",
                    enabled = true,
                );
            }
            return;
        }
        let was_plan = {
            let tracker = self.plan_mode.lock();
            tracker.state() != crate::session::plan_mode::PlanModeState::Inactive
        };
        if was_plan {
            let turn_in_flight = self.state.lock().await.running_task.is_some();
            self.plan_mode.lock().user_exit(turn_in_flight);
            self.persist_plan_mode_state();
            self.enqueue_current_mode_update(session_mode_id.clone());
            tracing::info!(
                session_id = %self.session_info.id.0,
                new_mode = %session_mode_id.0,
                turn_in_flight,
                "Plan mode toggled OFF"
            );
            xai_grok_telemetry::session_ctx::log_event(
                xai_grok_telemetry::events::PlanModeToggled {
                    enabled: false,
                    trigger: xai_grok_telemetry::events::PlanModeTrigger::User,
                    turn_in_flight,
                    was_previously_active: true,
                    from_mode: Some("plan".into()),
                },
            );
            xai_grok_telemetry::event_span!(
                "session.permission_mode_changed",
                from_mode = "plan",
                to_mode = %session_mode_id.0,
                trigger = "user",
                enabled = false,
            );
        }
        if let Some(name) = agent_target {
            self.switch_mode_agent(&name).await;
        }
    }
    /// Run the named agent for the session mode.
    ///
    /// A full rebuild: the system prompt and the tool registry change
    /// together. A prompt swap alone shows one agent's prompt beside another
    /// agent's tools. While a turn runs, the swap waits for the turn to end.
    async fn switch_mode_agent(&self, name: &str) {
        let def = match name {
            "browser_use" => Some(AgentDefinition::browser_use()),
            name => {
                xai_grok_agent::discovery::by_name_in_cwd(name, self.tool_context.cwd.as_path())
            }
        };
        let Some(def) = def else {
            return;
        };
        let active = self.active_agent_type.lock().clone();
        if active.as_deref() == Some(def.name.as_str()) {
            self.mode_agent.lock().pending = None;
            return;
        }
        if self.state.lock().await.running_task.is_some() {
            tracing::info!(
                session_id = %self.session_info.id.0,
                agent_name = %def.name,
                "session mode names another agent while a turn runs: swapping at turn end"
            );
            self.mode_agent.lock().pending = Some(def);
            return;
        }
        self.mode_agent.lock().pending = None;
        self.rebuild_mode_agent(def).await;
    }
    /// Apply a mode-agent swap that arrived while the last turn ran.
    pub(super) async fn apply_pending_mode_agent(&self) {
        if self.mode_agent.lock().pending.is_none()
            || self.state.lock().await.running_task.is_some()
        {
            return;
        }
        let Some(def) = self.mode_agent.lock().pending.take() else {
            return;
        };
        self.rebuild_mode_agent(def).await;
    }
    async fn rebuild_mode_agent(&self, def: AgentDefinition) {
        tracing::info!(
            session_id = %self.session_info.id.0,
            agent_name = %def.name,
            agent_scope = %def.scope,
            prompt_mode = ?def.prompt_mode,
            tool_configs = def.tool_config.tools.len(),
            "session mode: rebuilding agent"
        );
        let name = def.name.clone();
        // A mode switch keeps the model, so it keeps the model's label.
        let label = self
            .agent
            .borrow()
            .prompt_context()
            .system_prompt_label
            .clone();
        // `zero_turn: false`: this is a live switch, so the turn prefix
        // surgery must not run.
        if let Err(e) = self
            .handle_rebuild_agent_for_definition(def, false, label)
            .await
        {
            tracing::error!(
                session_id = %self.session_info.id.0,
                agent_name = %name,
                error = ?e,
                "session mode: agent rebuild failed; the session keeps the previous agent's prompt and tools"
            );
        }
    }
    /// Only a real user turn declares a mode.
    /// Reconciling one would end plan mode just by waking the session.
    /// Returns the resolved mode rather than echoing the argument, so a synthetic turn is also *recorded* under the mode it really ran in.
    pub(super) fn resolve_turn_prompt_mode(
        &self,
        origin: &crate::session::PromptOrigin,
        declared: PromptMode,
    ) -> PromptMode {
        if !origin.is_synthetic() {
            self.reconcile_plan_mode_with_prompt(declared);
        }
        *self.current_prompt_mode.lock()
    }
    /// Mirrors `handle_session_mode` but driven from `_meta.mode` on the prompt, the only signal the client sends.
    /// Without it a client that carries its mode on the prompt could enter or leave plan mode with no signal.
    /// The same line is what lands in `updates.jsonl`, so a later replay could not recover the mode either.
    pub(super) fn reconcile_plan_mode_with_prompt(&self, prompt_mode: PromptMode) {
        use crate::session::plan_mode::PlanModeState;
        *self.current_prompt_mode.lock() = prompt_mode;
        match prompt_mode {
            PromptMode::Plan => {
                let entered = self.plan_mode.lock().enter_pending();
                if entered {
                    self.persist_plan_mode_state();
                    self.enqueue_current_mode_update(session_mode_id_from_prompt_mode(prompt_mode));
                }
            }
            PromptMode::Agent | PromptMode::Ask => {
                let was_plan = {
                    let tracker = self.plan_mode.lock();
                    tracker.state() != PlanModeState::Inactive
                };
                if was_plan {
                    self.plan_mode.lock().user_exit(false);
                    self.persist_plan_mode_state();
                    self.enqueue_current_mode_update(session_mode_id_from_prompt_mode(prompt_mode));
                }
            }
        }
    }
    /// Called once per turn from `handle_prompt()`, before the user's actual message is pushed.
    /// All reminders are pushed as `<system-reminder>`-wrapped user messages so the model sees them in the same turn as the user's prompt.
    /// Tool names are resolved at render time via `TemplateRenderer`.
    pub(super) async fn inject_plan_mode_reminders(&self) {
        use crate::session::plan_mode::{
            PlanModeState, plan_mode_exit_reminder_template, plan_mode_reminder_full_template,
            plan_mode_reminder_sparse_template,
        };
        let use_cursor_reminders = self.is_cursor_harness();
        let push_reminder = |this: &Self, content: &str| {
            this.push_system_reminder_with_tag(content, this.reminder_wrapper_tag());
        };
        let mut injected_this_turn = false;
        let activation = {
            let tracker = self.plan_mode.lock();
            (tracker.state() == PlanModeState::Pending)
                .then(|| (tracker.is_reentry(), tracker.plan_file_path().to_path_buf()))
        };
        if let Some((is_reentry, plan_path)) = activation {
            self.plan_mode.lock().activate();
            self.persist_plan_mode_state();
            let plan_has_content =
                crate::session::plan_mode::plan_file_has_content(&plan_path).await;
            let template = self.plan_activation_template(is_reentry);
            if let Some(rendered) = self
                .render_plan_template(template, &plan_path, plan_has_content)
                .await
            {
                push_reminder(self, &rendered);
                injected_this_turn = true;
                self.plan_mode.lock().record_reminder_injected();
                self.persist_plan_mode_state();
                tracing::info!(
                    session_id = %self.session_info.id.0,
                    is_reentry,
                    uses_template_reminders = use_cursor_reminders,
                    "Plan mode activated: injected system-reminder"
                );
            }
        }
        if !injected_this_turn {
            let per_turn = {
                let tracker = self.plan_mode.lock();
                tracker.is_active().then(|| {
                    (
                        tracker.should_use_full_reminder(),
                        tracker.plan_file_path().to_path_buf(),
                    )
                })
            };
            if let Some((use_full, plan_path)) = per_turn {
                let plan_has_content =
                    crate::session::plan_mode::plan_file_has_content(&plan_path).await;
                let template = if use_full {
                    plan_mode_reminder_full_template()
                } else {
                    plan_mode_reminder_sparse_template()
                };
                if let Some(rendered) = self
                    .render_plan_template(template, &plan_path, plan_has_content)
                    .await
                {
                    push_reminder(self, &rendered);
                    self.plan_mode.lock().record_reminder_injected();
                    self.persist_plan_mode_state();
                }
            }
        }
        if self.plan_mode.lock().has_pending_exit_reminder() {
            let plan_path = self.plan_mode.lock().plan_file_path().to_path_buf();
            let template = plan_mode_exit_reminder_template();
            if let Some(rendered) = self.render_plan_template(template, &plan_path, false).await {
                push_reminder(self, &rendered);
            }
            self.plan_mode.lock().clear_pending_exit_reminder();
            self.persist_plan_mode_state();
        }
    }
    /// Mid-turn counterpart of `inject_plan_mode_reminders` case 1. The user toggled plan mode ON (Shift+Tab) while the model was thinking, so the tracker sits in `Pending`.
    /// The running turn would otherwise proceed without any plan-mode instruction.
    /// No-op unless the tracker is `Pending`.
    pub(super) async fn activate_plan_mode_mid_turn(&self) {
        use crate::session::plan_mode::PlanModeState;
        let activation = {
            let tracker = self.plan_mode.lock();
            (tracker.state() == PlanModeState::Pending)
                .then(|| (tracker.is_reentry(), tracker.plan_file_path().to_path_buf()))
        };
        let Some((is_reentry, plan_path)) = activation else {
            return;
        };
        let plan_has_content = crate::session::plan_mode::plan_file_has_content(&plan_path).await;
        let template = self.plan_activation_template(is_reentry);
        let rendered = self
            .render_plan_template(template, &plan_path, plan_has_content)
            .await;
        let tag = self.reminder_wrapper_tag();
        let buffered = rendered.is_some();
        let activated = match rendered {
            Some(rendered) => self
                .plan_mode
                .lock()
                .activate_mid_turn(format!("<{tag}>\n{rendered}\n</{tag}>")),
            None => {
                tracing::warn!(
                    session_id = %self.session_info.id.0,
                    "Mid-turn plan activation: reminder render failed; \
                     activating without a buffered reminder"
                );
                self.plan_mode.lock().activate()
            }
        };
        if !activated {
            return;
        }
        self.persist_plan_mode_state();
        tracing::info!(
            session_id = %self.session_info.id.0,
            is_reentry,
            buffered,
            "Plan mode activated mid-turn"
        );
    }
    /// The activation reminder template for the active template (no first-entry/reentry distinction), or grok's reentry/full variant.
    /// Shared by turn-start injection (`inject_plan_mode_reminders` case 1) and the mid-turn toggle (`activate_plan_mode_mid_turn`).
    fn plan_activation_template(&self, is_reentry: bool) -> &'static str {
        use crate::session::plan_mode::{
            plan_mode_reentry_reminder_template, plan_mode_reminder_full_template,
        };
        if is_reentry {
            plan_mode_reentry_reminder_template()
        } else {
            plan_mode_reminder_full_template()
        }
    }
    /// Render a plan mode template via the tool bridge's `TemplateRenderer`.
    ///
    /// Passes `plan_path` and `plan_has_content` as extra context alongside the registry's `tools.by_kind.*` mappings.
    pub(super) async fn render_plan_template(
        &self,
        template: &str,
        plan_path: &std::path::Path,
        plan_has_content: bool,
    ) -> Option<String> {
        let extra = serde_json::json!({
            "plan_path": plan_path.display().to_string(),
            "plan_has_content": plan_has_content,
            "goal_contract": !self.startup_hints.is_subagent && self.goal_harness_enabled(),
        });
        self.agent
            .borrow()
            .tool_bridge()
            .render_prompt(template, &extra)
            .await
    }
    /// Persist the current plan mode state to disk.
    ///
    /// Called after every state transition so plan mode survives session reload/resume/reconnect.
    pub(super) fn persist_plan_mode_state(&self) {
        let snapshot = self.plan_mode.lock().snapshot();
        let _ = self
            .notifications
            .persistence_tx
            .send(PersistenceMsg::PlanModeState(snapshot));
    }
}
#[cfg(test)]
mod mode_agent_target_tests {
    use super::{ModeAgentState, mode_agent_target};

    #[test]
    fn a_permission_mode_after_explore_restores_the_base_agent() {
        let mut state = ModeAgentState::default();
        assert_eq!(
            mode_agent_target(&mut state, "grok-build-orchestrator", Some("grok-build")).as_deref(),
            Some("grok-build-orchestrator")
        );
        assert_eq!(
            mode_agent_target(&mut state, "explore", Some("grok-build-orchestrator")).as_deref(),
            Some("explore")
        );
        // The client lost its ring state and sends a bare permission mode.
        assert_eq!(
            mode_agent_target(&mut state, "plan", Some("explore")).as_deref(),
            Some("grok-build")
        );
        // Nothing is left to restore after that.
        assert_eq!(
            mode_agent_target(&mut state, "default", Some("grok-build")),
            None
        );
    }

    #[test]
    fn every_permission_mode_leaves_the_ring() {
        for mode in ["default", "plan", "ask"] {
            let mut state = ModeAgentState::default();
            mode_agent_target(&mut state, "explore", Some("my-agent"));
            assert_eq!(
                mode_agent_target(&mut state, mode, Some("explore")).as_deref(),
                Some("my-agent"),
                "{mode} must restore the base agent"
            );
        }
    }

    #[test]
    fn the_ring_close_names_the_base_and_the_plan_mode_adds_nothing() {
        let mut state = ModeAgentState::default();
        mode_agent_target(&mut state, "explore", Some("grok-build"));
        assert_eq!(
            mode_agent_target(&mut state, "grok-build", Some("explore")).as_deref(),
            Some("grok-build")
        );
        assert_eq!(
            mode_agent_target(&mut state, "plan", Some("grok-build")),
            None
        );
    }

    #[test]
    fn a_session_that_started_as_explore_stays_explore() {
        let mut state = ModeAgentState::default();
        assert_eq!(mode_agent_target(&mut state, "plan", Some("explore")), None);
        assert_eq!(
            mode_agent_target(&mut state, "default", Some("explore")),
            None
        );
    }

    #[test]
    fn a_permission_mode_outside_the_ring_keeps_the_agent() {
        let mut state = ModeAgentState::default();
        assert_eq!(
            mode_agent_target(&mut state, "plan", Some("grok-build")),
            None
        );
        assert_eq!(mode_agent_target(&mut state, "default", None), None);
    }
}
