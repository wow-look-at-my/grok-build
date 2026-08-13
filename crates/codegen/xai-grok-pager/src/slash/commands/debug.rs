//! `/debug <what is wrong>` — a self-debugging skill: hand the model this
//! process's execution context (binary, config, log, model) and turn it loose
//! on the user's question.
//!
//! `/debug why was the context size defaulted to 256k?` injects the question
//! together with the answers the model would otherwise have to guess at:
//!
//! 1. The *real* debug-log target the firehose writes to for this session: the
//!    per-session `<grok_home>/debug/<session_id>.txt` file when
//!    `GROK_DEBUG_LOG` is enabled (per-session routing — the exact path
//!    `xai_grok_telemetry::debug_log`'s routing layer writes to), or the single
//!    explicit file when `GROK_LOG_FILE` / `GROK_DEBUG_LOG=<path>` is set
//!    (single-file routing writes only to that file). The file is created if it
//!    does not exist, so the advertised path is always real and readable.
//! 2. Whether the firehose is on at all, read from the same environment
//!    variables (`GROK_DEBUG_LOG` / `GROK_LOG_FILE`) the already-installed
//!    subscriber resolved at startup.
//! 3. The rest of the execution context — running binary vs installed binary
//!    (staleness), version and commit, config layers, model id, context window,
//!    effort, `GROK_*`/`XAI_*` environment — assembled by
//!    [`super::debug_context::DebugContext`].
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

use std::path::PathBuf;

use agent_client_protocol as acp;

use super::debug_context::{DebugContext, ModelFacts};
use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Filesystem-safe session key for the per-session debug log file name.
///
/// Mirrors the sanitization `xai_grok_telemetry::debug_log` applies when it
/// opens `<dir>/<session_id>.txt` for a session span (that crate keeps its
/// `sanitize_key` private). A normal session id is a UUID and passes through
/// unchanged; hostile / non-UTF-8-alphabetic values can never escape the debug
/// directory. Kept as a pure function so the resolver is testable without I/O.
fn sanitize_session_key(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Map empty / dot-only keys ("", ".", "..", "...") to a constant: those are
    // filesystem-special, and relying on the `.txt` suffix to neutralize them is
    // incidental.
    if safe.is_empty() || safe.bytes().all(|b| b == b'.') {
        return "_".to_owned();
    }
    safe
}

/// Resolve the concrete per-session firehose log path a `/debug` invocation
/// must name: `<grok_home>/debug/<session_id>.txt`.
///
/// This is the exact target `xai_grok_telemetry::debug_log`'s routing layer
/// writes to for the session (same dir, same `<session>.txt` naming, same
/// sanitization) — pure, so it is unit-tested with explicit inputs.
pub fn debug_log_path(grok_home: &std::path::Path, session_id: &str) -> PathBuf {
    grok_home
        .join("debug")
        .join(format!("{}.txt", sanitize_session_key(session_id)))
}

/// The on/off resolution of the firehose, mirroring what the already-installed
/// subscriber used at startup.
///
/// [`xai_grok_telemetry::debug_log`] decides the firehose from two env vars:
/// `GROK_LOG_FILE` (an explicit single file, wins) or `GROK_DEBUG_LOG` (a
/// truthy bool routes per-session into `~/.grok/debug`, any other value is a
/// single-file path). The tracing subscriber is initialized once at process
/// start, so there is no runtime API to re-init it for the already-running
/// process; this enum is the honest "is the firehose on / where does it go"
/// answer the command reports and records for the current session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugLogState {
    /// Firehose is on and routed **per session**; logs land in `dir` under
    /// `<session>.txt`. This is the always-on form the model reads from.
    On { dir: PathBuf },
    /// Firehose is on but writing to a single explicit file (`GROK_LOG_FILE` /
    /// `GROK_DEBUG_LOG=<path>`); per-session routing is bypassed.
    OnSingleFile { path: PathBuf },
    /// Firehose is off (no `GROK_LOG_FILE` and `GROK_DEBUG_LOG` unset/falsy).
    Off,
}

/// Determine the firehose state from the raw env values the subscriber
/// consulted. `None` means "unset". Mirrors the precedence in
/// `xai_grok_telemetry::debug_log::resolve_debug_target_inner` so a bare
/// `/debug` reports the same answer the last startup wrote to disk.
pub fn debug_log_state(
    grok_log_file: Option<&std::ffi::OsStr>,
    grok_debug_log: Option<&std::ffi::OsStr>,
    debug_dir: &std::path::Path,
) -> DebugLogState {
    if let Some(raw) = grok_log_file
        && !is_blank_os(raw)
    {
        return DebugLogState::OnSingleFile {
            path: os_path(raw),
        };
    }
    match grok_debug_log {
        None => DebugLogState::Off,
        Some(raw) => match raw.to_str().map(str::trim) {
            Some("" | "0" | "false" | "off" | "no") => DebugLogState::Off,
            Some("1" | "true" | "on" | "yes") => DebugLogState::On {
                dir: debug_dir.to_path_buf(),
            },
            // Any other UTF-8 value, or a non-UTF-8 value, is an explicit path.
            _ => DebugLogState::OnSingleFile {
                path: os_path(raw),
            },
        },
    }
}

fn is_blank_os(v: &std::ffi::OsStr) -> bool {
    v.to_str().is_some_and(|s| s.trim().is_empty())
}

fn os_path(v: &std::ffi::OsStr) -> PathBuf {
    match v.to_str() {
        Some(s) => PathBuf::from(s.trim()),
        None => PathBuf::from(v),
    }
}

/// One line describing where the firehose writes and whether it is on, for the
/// `Session log` row of the execution context.
pub fn log_summary(state: &DebugLogState) -> String {
    match state {
        DebugLogState::On { .. } => {
            "firehose ON (GROK_DEBUG_LOG, per-session routing)".to_string()
        }
        DebugLogState::OnSingleFile { .. } => {
            "firehose ON (GROK_LOG_FILE / GROK_DEBUG_LOG=<path>, single-file routing)".to_string()
        }
        DebugLogState::Off => "firehose OFF (no GROK_DEBUG_LOG / GROK_LOG_FILE): the file exists \
                               but stays empty until grok is relaunched with GROK_DEBUG_LOG=1"
            .to_string(),
    }
}

/// Build the scrollback display text for an injecting `/debug`.
pub fn debug_display_text(path: &std::path::Path, request: &str) -> String {
    let request = request.trim();
    if request.is_empty() {
        format!(
            "/debug: injected debug context; log: {}",
            path.display()
        )
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
    fn name(&self) -> &str {
        "debug"
    }

    fn description(&self) -> &str {
        "Debug grok itself: inject this session's execution context and a question"
    }

    fn usage(&self) -> &str {
        "/debug [<what is wrong> | scroll | fps | log]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("what is wrong? (or: scroll | fps | log)")
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
            request => {
                let request = if request == "on" { "" } else { request };
                self.inject(ctx, request)
            }
        }
    }
}

impl DebugCommand {
    /// Resolve the firehose target, provision it, and inject the execution
    /// context plus the user's question.
    fn inject(&self, ctx: &mut CommandExecCtx, request: &str) -> CommandResult {
        let Some(session_id) = ctx.session_id else {
            return CommandResult::Error(
                "/debug needs an active session so it can resolve the per-session \
                 debug log path"
                    .to_string(),
            );
        };
        let home = xai_grok_config::grok_home();
        let state = debug_log_state(
            std::env::var_os("GROK_LOG_FILE").as_deref(),
            std::env::var_os("GROK_DEBUG_LOG").as_deref(),
            &home.join("debug"),
        );
        // The path the firehose ACTUALLY writes to for this session. Per-session
        // routing (`On`) writes `<dir>/<session_id>.txt`; a single explicit file
        // (`GROK_LOG_FILE` / `GROK_DEBUG_LOG=<path>`) writes only to that file,
        // so the injection must name that file — never a per-session path that
        // would stay empty. `Off` falls back to provisioning the per-session
        // file so the model still has a real log.
        let path = match &state {
            DebugLogState::OnSingleFile { path } => path.clone(),
            DebugLogState::On { .. } | DebugLogState::Off => {
                debug_log_path(&home, session_id.0.as_ref())
            }
        };
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

        let context = DebugContext::gather(
            session_id.0.as_ref(),
            path.clone(),
            log_summary(&state),
            model_facts(ctx),
        );
        CommandResult::InjectSkill {
            display_text: debug_display_text(&path, request),
            prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                context.render(request),
            ))],
            display_as_skill: true,
            scheduled_task_preview: None,
        }
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
            .map(|effort| effort.as_str().to_string()),
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
            screen_mode: crate::app::ScreenMode::Fullscreen,
        }
    }

    /// Serializes the end-to-end `run()` tests that read the real
    /// `GROK_DEBUG_LOG` / `GROK_LOG_FILE` environment. `run()` reads those via
    /// `std::env::var_os`, and the single-file test mutates them (edition 2024
    /// `set_var`/`remove_var` are process-global), so the two must never run
    /// concurrently or one would observe the other's env while asserting.
    fn run_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
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

    /// `/debug scroll` and `/scroll-debug` must stay routed to the SAME
    /// action — the HUD has one toggle, two spellings.
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
    fn debug_log_state_per_session_from_truthy_env() {
        for v in ["1", "true", "on", "yes"] {
            let state = debug_log_state(
                None,
                Some(std::ffi::OsStr::new(v)),
                Path::new("/homes/alice/.grok/debug"),
            );
            assert_eq!(
                state,
                DebugLogState::On {
                    dir: PathBuf::from("/homes/alice/.grok/debug")
                },
                "truthy GROK_DEBUG_LOG={v:?} must be per-session firehose on"
            );
        }
    }

    #[test]
    fn debug_log_state_off_from_unset_or_falsy_env() {
        assert_eq!(
            debug_log_state(None, None, Path::new("/debug")),
            DebugLogState::Off
        );
        for v in ["", "0", "false", "off", "no", "  "] {
            assert_eq!(
                debug_log_state(None, Some(std::ffi::OsStr::new(v)), Path::new("/debug")),
                DebugLogState::Off,
                "GROK_DEBUG_LOG={v:?} must be off"
            );
        }
    }

    #[test]
    fn debug_log_state_single_file_wins_and_explicit_path() {
        assert_eq!(
            debug_log_state(
                Some(std::ffi::OsStr::new("/tmp/fire.log")),
                Some(std::ffi::OsStr::new("1")),
                Path::new("/debug")
            ),
            DebugLogState::OnSingleFile {
                path: PathBuf::from("/tmp/fire.log")
            }
        );
        assert_eq!(
            debug_log_state(
                None,
                Some(std::ffi::OsStr::new("/tmp/custom.log")),
                Path::new("/debug")
            ),
            DebugLogState::OnSingleFile {
                path: PathBuf::from("/tmp/custom.log")
            }
        );
    }

    /// The summary line must let the model tell an empty log from a live one —
    /// a firehose-off session's file exists but never fills.
    #[test]
    fn log_summary_distinguishes_on_off_and_single_file() {
        assert!(
            log_summary(&DebugLogState::On {
                dir: PathBuf::from("/d")
            })
            .contains("ON"),
        );
        assert!(
            log_summary(&DebugLogState::OnSingleFile {
                path: PathBuf::from("/f")
            })
            .contains("single-file"),
        );
        let off = log_summary(&DebugLogState::Off);
        assert!(off.contains("OFF") && off.contains("GROK_DEBUG_LOG=1"), "{off}");
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
        let _env = run_env_lock();
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-question-sess");
        ctx.session_id = Some(&sid);
        unsafe {
            std::env::remove_var("GROK_LOG_FILE");
            std::env::remove_var("GROK_DEBUG_LOG");
        }

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
            text.text.contains("why was the context size defaulted to 256k?"),
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
        let _env = run_env_lock();
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-bare-sess");
        ctx.session_id = Some(&sid);
        unsafe {
            std::env::remove_var("GROK_LOG_FILE");
            std::env::remove_var("GROK_DEBUG_LOG");
        }

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

    /// When a single firehose file is configured (`GROK_LOG_FILE` or
    /// `GROK_DEBUG_LOG=<path>`), the injection must name THAT file — the only
    /// file the firehose actually writes to — not an empty per-session
    /// `<debug>/<sid>.txt`.
    #[test]
    fn debug_with_grok_log_file_injects_single_file_path() {
        let _env = run_env_lock();
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("single-file-sess");
        ctx.session_id = Some(&sid);

        // Point the firehose at one explicit file (GROK_LOG_FILE wins).
        let target = std::env::temp_dir().join("grok-debug-single-file-evidence.log");
        let _ = std::fs::remove_file(&target);
        unsafe {
            std::env::set_var("GROK_LOG_FILE", &target);
            std::env::remove_var("GROK_DEBUG_LOG");
        }

        let result = DebugCommand.run(&mut ctx, "");
        let CommandResult::InjectSkill { prompt_blocks, .. } = result else {
            panic!("/debug with GROK_LOG_FILE set must InjectSkill, got {result:?}");
        };
        let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
            panic!("expected a text prompt block");
        };
        let target_str = target.to_str().unwrap();
        assert!(
            text.text.contains(target_str),
            "prompt block must name the single firehose file (not a per-session \
             path); got: {}",
            text.text
        );
        assert!(
            text.text.contains("single-file"),
            "the log line must say where the firehose is routed: {}",
            text.text
        );
        // The ensure-step must have provisioned the single file itself.
        assert!(
            target.is_file(),
            "run() must provision the single firehose file: {target:?}"
        );

        unsafe {
            std::env::remove_var("GROK_LOG_FILE");
            std::env::remove_var("GROK_DEBUG_LOG");
        }
    }
}
