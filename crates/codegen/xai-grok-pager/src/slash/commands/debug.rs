//! `/debug <what is wrong>` — a self-debugging skill: hand the model this
//! process's execution context (binary, config, log, model) and turn it loose
//! on the user's question.
//!
//! `/debug why was the context size defaulted to 256k?` injects the question
//! together with the answers the model would otherwise have to guess at:
//!
//! - The debug-log file the firehose writes for this session. `/debug` turns
//!   the firehose on first (`debug_log::enable_firehose`), in this process and,
//!   through `ENABLE_FIREHOSE_META` on the prompt block, in the agent process.
//!   The file is `<grok_home>/debug/<session_id>.txt`, or the one file that
//!   `GROK_LOG_FILE` / `GROK_DEBUG_LOG=<path>` names. It is created if it does
//!   not exist.
//! - Whether the firehose ran since launch or only since this `/debug`.
//! - The rest of the execution context — running binary vs installed binary
//!   (staleness), version and commit, config layers, model id, context window,
//!   effort, `GROK_*`/`XAI_*` environment — assembled by
//!   [`super::debug_context::DebugContext`].
//!
//! Delivery is [`CommandResult::InjectSkill`], the same path skills and `/loop`
//! use, so the injected prompt reaches the model as the next turn's content.
//!
//! Args that are not one of the reserved overlay keywords are the user's
//! question, verbatim. The overlay toggles keep their keywords:
//! - `/debug` bare / `/debug on` — inject the context with no question; the
//!   model debugs whatever the user says next.
//! - `/debug scroll` — the scroll-diagnostics HUD; same
//!   [`Action::ToggleScrollDebugHud`] as `/scroll-debug`, which stays
//!   registered as the hidden long-form alias.
//! - `/debug fps` — the release-safe FPS HUD
//!   ([`crate::views::fps_hud`]).
//! - `/debug log` — the scroll flight recorder
//!   ([`crate::input::scroll_log`]), runtime-constructed to a fresh
//!   timestamped path.

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use xai_grok_telemetry::debug_log::{
    ENABLE_FIREHOSE_META, FirehoseStatus, enable_firehose, session_log_path,
};

use super::debug_context::{DebugContext, ModelFacts};
use crate::app::actions::Action;
use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand, slash_meta,
};

/// The per-session firehose file: `<grok_home>/debug/<session_id>.txt`.
pub fn debug_log_path(grok_home: &Path, session_id: &str) -> PathBuf {
    session_log_path(&grok_home.join("debug"), session_id)
}

/// The file the firehose writes for `session_id`, given where it is routed.
///
/// `Unavailable` still names the per-session file under `default_dir`. An agent
/// in a separate leader process routes there once the prompt wakes its firehose.
pub fn log_target(status: &FirehoseStatus, default_dir: &Path, session_id: &str) -> PathBuf {
    match status {
        FirehoseStatus::PerSession { dir, .. } => session_log_path(dir, session_id),
        FirehoseStatus::SingleFile { path } => path.clone(),
        FirehoseStatus::Unavailable => session_log_path(default_dir, session_id),
    }
}

/// One line for the `Session log` row: is the firehose on, and since when.
pub fn log_summary(status: &FirehoseStatus) -> String {
    match status {
        FirehoseStatus::PerSession {
            enabled_at_runtime: true,
            ..
        } => "firehose ON since this /debug, per-session routing. Events from before \
              this /debug were not recorded"
            .to_string(),
        FirehoseStatus::PerSession {
            enabled_at_runtime: false,
            ..
        } => "firehose ON since launch (GROK_DEBUG_LOG, per-session routing)".to_string(),
        FirehoseStatus::SingleFile { .. } => {
            "firehose ON since launch (GROK_LOG_FILE / GROK_DEBUG_LOG=<path>, single-file \
             routing)"
                .to_string()
        }
        FirehoseStatus::Unavailable => "firehose UNAVAILABLE in the TUI process (no firehose \
                                        layer installed). The agent process turns its own on \
                                        when it receives this prompt"
            .to_string(),
    }
}

/// Build the scrollback display text for an injecting `/debug`.
pub fn debug_display_text(path: &std::path::Path, request: &str) -> String {
    let request = request.trim();
    if request.is_empty() {
        format!("/debug: injected debug context; log: {}", path.display())
    } else {
        format!("/debug {request}")
    }
}

/// Args that are NOT the user's question: the overlay toggles plus the `on`
/// alias for a bare invocation. Anything else is free text.
const SUBCOMMANDS: &[(&str, &str)] = &[
    ("on", "Inject the debug context with no question attached"),
    ("scroll", "Toggle the scroll-diagnostics HUD"),
    ("fps", "Toggle the FPS overlay"),
    ("log", "Toggle the scroll flight recorder (JSONL)"),
];

/// Self-debugging skill + the overlay toggles it fronts.
pub struct DebugCommand;

impl SlashCommand for DebugCommand {
    slash_meta! {
        name: "debug",
        description: "Debug grok itself: inject this session's execution context and a question",
        usage: "/debug [<what is wrong> | scroll | fps | log]",
        takes_args: true,
        // The injection needs a session id to resolve the log path, so the
        // session-less dashboard input does not offer it.
        session_scoped: true,
        arg_placeholder: "what is wrong? (or: scroll | fps | log)",
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(
            SUBCOMMANDS
                .iter()
                .map(|&(name, desc)| ArgItem {
                    display: name.to_string(),
                    match_text: name.to_string(),
                    insert_text: name.to_string(),
                    description: desc.to_string(),
                    loaded_in_vram: None,
                })
                .collect(),
        )
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        match args.trim() {
            "scroll" => CommandResult::Action(Action::ToggleScrollDebugHud),
            "fps" => CommandResult::Action(Action::ToggleFpsHud),
            "log" => CommandResult::Action(Action::ToggleScrollLog),
            // Everything else is the user's question — `on` and a bare `/debug`
            // are the same invocation with no question attached.
            request => inject(ctx, if request == "on" { "" } else { request }),
        }
    }
}

/// Resolve the firehose target, provision it, and inject the execution context
/// plus the user's question.
fn inject(ctx: &mut CommandExecCtx, request: &str) -> CommandResult {
    let Some(session_id) = ctx.session_id else {
        return CommandResult::Error(
            "/debug needs an active session so it can resolve the per-session \
                 debug log path"
                .to_string(),
        );
    };
    // Turn the firehose on in this process.
    let status = enable_firehose();
    let path = log_target(
        &status,
        &xai_grok_config::grok_home().join("debug"),
        session_id.0.as_ref(),
    );
    // Ensure the log file exists so the advertised path is real and readable
    // by the model's tools, even before the firehose writes to it.
    // Best-effort: if the dir can't be created the injection still proceeds.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);

    let display_text = debug_display_text(&path, request);
    let context = DebugContext::gather(
        session_id.0.as_ref(),
        path,
        log_summary(&status),
        model_facts(ctx),
    );
    let mut meta = acp::Meta::new();
    meta.insert(ENABLE_FIREHOSE_META.into(), serde_json::Value::Bool(true));
    CommandResult::InjectSkill {
        display_text,
        prompt_blocks: vec![acp::ContentBlock::Text(
            acp::TextContent::new(context.render(request)).meta(Some(meta)),
        )],
        display_as_skill: true,
        scheduled_task_preview: None,
    }
}

/// The model rows of the execution context, read from the pager's own state so
/// they match what this session is acting on rather than the catalog default.
fn model_facts(ctx: &CommandExecCtx) -> ModelFacts {
    ModelFacts {
        name: ctx.models.current_model_name(),
        id: ctx.models.current_model_id_str().map(str::to_string),
        context_window: ctx.models.get_context_window(),
        reasoning_effort: ctx
            .models
            .reasoning_effort
            .map(|effort| effort.as_ref().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::slash::commands::scroll_debug::ScrollDebugCommand;
    use crate::slash::commands::tests::make_ctx;

    fn app_ctx(models: &ModelState) -> AppCtx<'_> {
        AppCtx {
            models,
            cwd: std::path::Path::new("."),
            has_session_announcements: false,
            billing_surface_visible: true,
            usage_command_visible: true,
            workflows_available: true,
            saved_workflows: &[],
            workflow_runs: &[],
            current_title: None,
        }
    }

    /// The whole point of the command is being typeable: it has to be listed on
    /// every binary, release included, not just where `debug_assertions` is on.
    #[test]
    fn debug_is_listed_on_every_binary() {
        let models = ModelState::default();
        assert!(
            DebugCommand.visible(&app_ctx(&models)),
            "/debug must be offered in the composer regardless of build profile"
        );
    }

    /// `/debug scroll` and `/scroll-debug` must stay routed to the same action: the HUD has one toggle, two spellings.
    #[test]
    fn debug_scroll_routes_to_same_action_as_scroll_debug() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        assert!(matches!(
            DebugCommand.run(&mut ctx, "scroll"),
            CommandResult::Action(Action::ToggleScrollDebugHud)
        ));
        assert!(matches!(
            ScrollDebugCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::ToggleScrollDebugHud)
        ));
    }

    #[test]
    fn debug_fps_and_log_route_to_their_toggles() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        assert!(matches!(
            DebugCommand.run(&mut ctx, " fps "),
            CommandResult::Action(Action::ToggleFpsHud)
        ));
        assert!(matches!(
            DebugCommand.run(&mut ctx, "log"),
            CommandResult::Action(Action::ToggleScrollLog)
        ));
    }

    #[test]
    fn debug_requires_session_for_injection() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        // make_ctx has session_id: None — an injecting /debug must refuse cleanly.
        for args in ["", "   ", "on", "why is the context window 256k?"] {
            assert!(
                matches!(DebugCommand.run(&mut ctx, args), CommandResult::Error(_)),
                "/debug without a session must error, args={args:?}"
            );
        }
    }

    #[test]
    fn debug_suggest_args_lists_subcommands() {
        let models = ModelState::default();
        let items = DebugCommand
            .suggest_args(&app_ctx(&models), "")
            .expect("suggestions");
        let names: Vec<&str> = items.iter().map(|i| i.insert_text.as_str()).collect();
        assert_eq!(names, vec!["on", "scroll", "fps", "log"]);
    }

    // ── Pure resolver / builder tests ────────────────────────────────────

    #[test]
    fn debug_log_path_is_under_grok_home_debug_with_session_name() {
        let path = debug_log_path(Path::new("/homes/alice/.grok"), "0192-abc-EF");
        assert_eq!(
            path,
            PathBuf::from("/homes/alice/.grok/debug/0192-abc-EF.txt")
        );
    }

    #[test]
    fn debug_log_path_sanitizes_hostile_session_ids() {
        // Path separators / dot-only ids must never escape the debug dir.
        assert_eq!(
            debug_log_path(Path::new("/gh"), "../escape"),
            PathBuf::from("/gh/debug/.._escape.txt")
        );
        assert_eq!(
            debug_log_path(Path::new("/gh"), "a/b\\c"),
            PathBuf::from("/gh/debug/a_b_c.txt")
        );
        for dotty in ["", ".", ".."] {
            assert_eq!(
                debug_log_path(Path::new("/gh"), dotty),
                PathBuf::from("/gh/debug/_.txt")
            );
        }
    }

    #[test]
    fn log_target_names_the_file_the_firehose_writes() {
        let default_dir = Path::new("/h/.grok/debug");
        for enabled_at_runtime in [false, true] {
            assert_eq!(
                log_target(
                    &FirehoseStatus::PerSession {
                        dir: PathBuf::from("/other/debug"),
                        enabled_at_runtime,
                    },
                    default_dir,
                    "sid"
                ),
                PathBuf::from("/other/debug/sid.txt")
            );
        }
        assert_eq!(
            log_target(
                &FirehoseStatus::SingleFile {
                    path: PathBuf::from("/tmp/fire.log")
                },
                default_dir,
                "sid"
            ),
            PathBuf::from("/tmp/fire.log"),
            "single-file routing writes only that file"
        );
        assert_eq!(
            log_target(&FirehoseStatus::Unavailable, default_dir, "sid"),
            PathBuf::from("/h/.grok/debug/sid.txt")
        );
    }

    /// The model must know whether the log covers the time before `/debug`.
    #[test]
    fn log_summary_says_since_when_the_firehose_ran() {
        let runtime = log_summary(&FirehoseStatus::PerSession {
            dir: PathBuf::from("/d"),
            enabled_at_runtime: true,
        });
        assert!(
            runtime.contains("ON since this /debug") && runtime.contains("not recorded"),
            "{runtime}"
        );
        let launch = log_summary(&FirehoseStatus::PerSession {
            dir: PathBuf::from("/d"),
            enabled_at_runtime: false,
        });
        assert!(launch.contains("ON since launch"), "{launch}");
        assert!(
            log_summary(&FirehoseStatus::SingleFile {
                path: PathBuf::from("/f")
            })
            .contains("single-file"),
        );
        assert!(log_summary(&FirehoseStatus::Unavailable).contains("UNAVAILABLE"));
    }

    #[test]
    fn display_text_shows_the_question_and_falls_back_to_the_log_path() {
        assert_eq!(
            debug_display_text(Path::new("/h/.grok/debug/s.txt"), "  why 256k?  "),
            "/debug why 256k?"
        );
        assert!(
            debug_display_text(Path::new("/h/.grok/debug/s.txt"), "")
                .contains("/h/.grok/debug/s.txt")
        );
    }

    /// The user's headline case: `/debug <free text>` must reach the model as a
    /// skill injection carrying the question AND this process's real context —
    /// not an "unknown option" error.
    #[test]
    fn debug_with_a_question_injects_it_with_the_execution_context() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-question-sess");
        ctx.session_id = Some(&sid);

        let result = DebugCommand.run(
            &mut ctx,
            "why was the context size defaulted to 256k? this model is 1m",
        );
        let CommandResult::InjectSkill {
            display_text,
            prompt_blocks,
            display_as_skill,
            scheduled_task_preview,
        } = result
        else {
            panic!("/debug <question> must InjectSkill, got {result:?}");
        };
        assert!(display_as_skill, "it renders as the skill invocation it is");
        assert!(scheduled_task_preview.is_none());
        assert_eq!(
            display_text,
            "/debug why was the context size defaulted to 256k? this model is 1m"
        );
        let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
            panic!("expected a text prompt block");
        };
        let home = xai_grok_config::grok_home();
        let expected_log = debug_log_path(&home, "debug-question-sess");
        assert!(
            text.text
                .contains("why was the context size defaulted to 256k?"),
            "the question must reach the model: {}",
            text.text
        );
        for expected in [
            expected_log.to_str().unwrap(),
            home.join("config.toml").to_str().unwrap(),
            "Running binary",
            "PID",
            "DEBUG mode",
        ] {
            assert!(
                text.text.contains(expected),
                "execution context missing {expected:?} in: {}",
                text.text
            );
        }
        // The ensure-step must have provisioned a real file at that path.
        assert!(
            expected_log.is_file(),
            "/debug must ensure the session log file exists: {expected_log:?}"
        );
    }

    /// A bare `/debug` (and its `on` alias) still injects — with no question,
    /// so the model debugs whatever the user says next.
    #[test]
    fn debug_bare_and_on_inject_without_a_question() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-bare-sess");
        ctx.session_id = Some(&sid);

        for args in ["", "on"] {
            let result = DebugCommand.run(&mut ctx, args);
            let CommandResult::InjectSkill {
                display_text,
                prompt_blocks,
                ..
            } = result
            else {
                panic!("/debug {args:?} must InjectSkill, got {result:?}");
            };
            let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                panic!("expected a text prompt block");
            };
            assert!(
                text.text.contains("debug what the user says next"),
                "a question-less /debug must still be actionable: {}",
                text.text
            );
            assert!(
                display_text.contains("debug-bare-sess.txt"),
                "scrollback must reference the session log: {display_text}"
            );
        }
    }

    /// The agent can run in a separate leader process.
    #[test]
    fn debug_prompt_block_asks_the_agent_process_for_the_firehose() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-meta-sess");
        ctx.session_id = Some(&sid);

        let result = DebugCommand.run(&mut ctx, "why?");
        let CommandResult::InjectSkill { prompt_blocks, .. } = result else {
            panic!("/debug must InjectSkill, got {result:?}");
        };
        let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
            panic!("expected a text prompt block");
        };
        assert_eq!(
            text.meta
                .as_ref()
                .and_then(|meta| meta.get(ENABLE_FIREHOSE_META)),
            Some(&serde_json::Value::Bool(true)),
            "the /debug prompt block must carry {ENABLE_FIREHOSE_META}=true: {:?}",
            text.meta
        );
    }
}
