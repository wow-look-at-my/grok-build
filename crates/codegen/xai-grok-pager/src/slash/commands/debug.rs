//! `/debug` — Claude-Code-style debug self-help: ensure the firehose is on,
//! hand the model the concrete per-session log path, and direct it to debug
//! what the user said.
//!
//! Reworked from a set of debug-overlay toggles. The toggles still live on
//! behind their subcommands ([`SUBCOMMANDS`]: `scroll` / `fps` / `log`, the
//! same actions as `/scroll-debug` and friends) — untouched — but a **bare**
//! `/debug` no longer just prints an overlay status line. It now:
//!
//! 1. Resolves the *real* debug-log target the firehose writes to for this
//!    session: the per-session `<grok_home>/debug/<session_id>.txt` file when
//!    `GROK_DEBUG_LOG` is enabled (per-session routing — the exact path
//!    `xai_grok_telemetry::debug_log`'s routing layer writes to), or the single
//!    explicit file when `GROK_LOG_FILE` / `GROK_DEBUG_LOG=<path>` is set
//!    (single-file routing writes only to that file).
//! 2. *Confirms / records* whether the firehose ("debug logging") is on for
//!    this session by reading the same environment variables
//!    (`GROK_DEBUG_LOG` / `GROK_LOG_FILE`) the already-installed firehose
//!    resolved at startup, and *ensures* the log file it names exists on disk
//!    so the advertised path is real and readable by the model's tools.
//! 3. Injects model-facing instructions through the real
//!    [`CommandResult::InjectSkill`] path (the same delivery used by `/loop`
//!    and skills) telling the model it is in debug mode, naming the log file
//!    path, and directing it to debug whatever the user just said.
//!
//! Registration/visibility split: the command is registered on EVERY binary
//! and fully functional in release — like the hidden diagnostics it fronts
//! (`/scroll-debug`, `/gboom`) — but it is LISTED (dropdown, completion,
//! recognized-token highlight via `visible()`) only on debug binaries
//! (`cfg(debug_assertions)`). Discoverable where developers live, out of
//! sight for users, yet still typeable in the field when support asks.
//!
//! Subcommands (args-based; a popup menu can come later):
//! - `/debug` bare — the Claude-Code-style injection: confirm/provision the
//!   firehose and inject the debug instructions + log path to the model.
//! - `/debug on` — alias for the bare behavior (explicit "make debug on").
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

/// The model-facing instruction block delivered on a bare `/debug`.
///
/// Names the concrete log path so the model can read it through its own file
/// tools, and directs it to debug whatever the user just said. Pure, so tests
/// assert on the actual text.
pub fn debug_instruction_text(path: &std::path::Path, on: bool) -> String {
    let state = if on {
        "Debug / firehose logging is confirmed ON for this session."
    } else {
        "Debug / firehose logging is currently OFF for this session (no \
         GROK_DEBUG_LOG / GROK_LOG_FILE). The per-session log file below is \
         provisioned and ready; relaunch grok with GROK_DEBUG_LOG=1 to have the \
         firehose populate it."
    };
    format!(
        "You are in DEBUG mode for this grok session.\n\
         {state}\n\
         The debug log file for this session is:\n\
         {}\n\
         Read it (and any supporting harness state) with your file tools, then \
         diagnose and debug what the user just said. Explain the root cause and \
         fix anything that is broken.",
        path.display()
    )
}

/// Build the scrollback display text for a bare `/debug`.
pub fn debug_display_text(path: &std::path::Path) -> String {
    format!(
        "/debug: injected debug instructions; log: {}",
        path.display()
    )
}

/// Whether `/debug` is listed on completion surfaces. `visible()` returns
/// this constant, so release invisibility is pinned by the constant's shape
/// (`cfg!(debug_assertions)`) rather than a runtime check — tests always
/// compile with `debug_assertions`, so the release half is untestable by
/// assertion and locked by construction instead.
pub const LISTED_IN_COMPLETIONS: bool = cfg!(debug_assertions);

/// Subcommand name/description pairs (single source for run + suggestions).
/// `on` and the bare invocation share the Claude-Code-style injection; the
/// overlay toggles stay as-is.
const SUBCOMMANDS: &[(&str, &str)] = &[
    ("on", "Ensure debug logging is on and inject debug instructions + log path"),
    ("scroll", "Toggle the scroll-diagnostics HUD"),
    ("fps", "Toggle the FPS overlay"),
    ("log", "Toggle the scroll flight recorder (JSONL)"),
];

/// Debug self-help + overlay toggles; listed only on debug binaries.
pub struct DebugCommand;

impl SlashCommand for DebugCommand {
    fn name(&self) -> &str {
        "debug"
    }

    fn description(&self) -> &str {
        "Debug the session: hand the model the debug log path and instructions"
    }

    fn usage(&self) -> &str {
        "/debug [on|scroll|fps|log]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("on | scroll | fps | log")
    }

    /// Debug binaries only; release keeps it registered but unlisted.
    fn visible(&self, _ctx: &AppCtx) -> bool {
        LISTED_IN_COMPLETIONS
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
            // Bare (and explicit `on`): the Claude-Code-style debug self-help.
            // Resolve the firehose state + per-session log path, ensure the log
            // file exists, then hand the model the path + instructions.
            "" | "on" => match ctx.session_id {
                Some(session_id) => {
                    let home = xai_grok_config::grok_home();
                    let state = debug_log_state(
                        std::env::var_os("GROK_LOG_FILE").as_deref(),
                        std::env::var_os("GROK_DEBUG_LOG").as_deref(),
                        &home.join("debug"),
                    );
                    // The path the firehose ACTUALLY writes to for this session.
                    // Per-session routing (`On`) writes `<dir>/<session_id>.txt`;
                    // a single explicit file (`GROK_LOG_FILE` /
                    // `GROK_DEBUG_LOG=<path>`) writes only to that file, so the
                    // injection must name that file — never a per-session path
                    // that would stay empty. `Off` falls back to provisioning
                    // the per-session file so the model still has a real log.
                    let path = match &state {
                        DebugLogState::OnSingleFile { path } => path.clone(),
                        DebugLogState::On { .. } | DebugLogState::Off => {
                            debug_log_path(&home, session_id.0.as_ref())
                        }
                    };
                    let on = state != DebugLogState::Off;
                    // Ensure the log file exists so the advertised path is real
                    // and readable by the model's tools, even before the firehose
                    // writes to it. Best-effort: if the dir can't be created the
                    // injection still proceeds.
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path);
                    CommandResult::InjectSkill {
                        display_text: debug_display_text(&path),
                        prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                            debug_instruction_text(&path, on),
                        ))],
                        display_as_skill: false,
                        scheduled_task_preview: None,
                    }
                }
                None => CommandResult::Error(
                    "/debug needs an active session so it can resolve the per-session \
                     debug log path".to_string(),
                ),
            },
            "scroll" => CommandResult::Action(Action::ToggleScrollDebugHud),
            "fps" => CommandResult::Action(Action::ToggleFpsHud),
            "log" => CommandResult::Action(Action::ToggleScrollLog),
            other => CommandResult::Error(format!(
                "Unknown /debug option '{other}'. Usage: /debug [on|scroll|fps|log]"
            )),
        }
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

    /// Tests compile with `debug_assertions`, so this asserts the
    /// debug-binary half live: `/debug` must be visible here. The release
    /// half (invisible) is untestable from a debug test build and pinned by
    /// mechanism instead — `visible()` returns `LISTED_IN_COMPLETIONS =
    /// cfg!(debug_assertions)`, which a release compile evaluates to
    /// `false` by construction; the `assert_eq!` locks `visible()` to that
    /// constant under whichever profile compiles the test.
    #[test]
    fn debug_listed_on_debug_binaries_only() {
        let models = ModelState::default();
        let listed = DebugCommand.visible(&app_ctx(&models));
        assert_eq!(
            listed,
            cfg!(debug_assertions),
            "visible() must track the binary profile"
        );
        assert_eq!(listed, LISTED_IN_COMPLETIONS);
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
        // make_ctx has session_id: None — a bare /debug must refuse cleanly.
        for args in ["", "   ", "on"] {
            assert!(
                matches!(DebugCommand.run(&mut ctx, args), CommandResult::Error(_)),
                "bare /debug without a session must error, args={args:?}"
            );
        }
    }

    #[test]
    fn debug_junk_subcommand_errors_helpfully() {
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        match DebugCommand.run(&mut ctx, "wat") {
            CommandResult::Error(msg) => {
                assert!(msg.contains("wat"), "must echo the bad option: {msg}");
                assert!(
                    msg.contains("scroll") && msg.contains("fps") && msg.contains("log"),
                    "must list the valid options: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn debug_suggest_args_lists_subcommands() {
        let models = ModelState::default();
        let items = DebugCommand
            .suggest_args(&app_ctx(&models), "")
            .expect("suggestions");
        let names: Vec<&str> = items.iter().map(|i| i.insert_text.as_str()).collect();
        assert_eq!(
            names,
            vec!["on", "scroll", "fps", "log"],
            "completion must surface the new `on` subcommand"
        );
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
            debug_log_state(None, Some(std::ffi::OsStr::new("/tmp/custom.log")), Path::new("/debug")),
            DebugLogState::OnSingleFile {
                path: PathBuf::from("/tmp/custom.log")
            }
        );
    }

    #[test]
    fn debug_instruction_text_names_the_path_and_debug_directive() {
        let text = debug_instruction_text(Path::new("/h/.grok/debug/sid.txt"), true);
        assert!(
            text.contains("/h/.grok/debug/sid.txt"),
            "must name the concrete log path: {text}"
        );
        assert!(text.contains("DEBUG mode"), "must declare debug mode: {text}");
        assert!(
            text.contains("Read it") && text.contains("debug what the user just said"),
            "must direct the model to read the log and debug the user's request: {text}"
        );
        assert!(
            text.contains("confirmed ON"),
            "must report firehose on: {text}"
        );
    }

    #[test]
    fn debug_instruction_text_off_reports_provisioned_and_relaunch() {
        let text = debug_instruction_text(Path::new("/h/.grok/debug/sid.txt"), false);
        assert!(
            text.contains("currently OFF"),
            "must state firehose off: {text}"
        );
        assert!(
            text.contains("GROK_DEBUG_LOG=1"),
            "must tell how to enable on relaunch: {text}"
        );
    }

    /// Drive the real bare invocation: it must return an `InjectSkill` whose
    /// prompt block embeds the real per-session path under the grok debug home
    /// and instructs the model to debug. This exercises the full `run()` glue
    /// (routing + resolver + builder) with the shipped code.
    #[test]
    fn debug_bare_invocation_injects_path_and_instructions() {
        let _env = run_env_lock();
        let models = ModelState::default();
        let mut ctx = make_ctx(&models);
        let sid = acp::SessionId::new("debug-bare-sess");
        ctx.session_id = Some(&sid);
        // This test asserts on the ambient (test) environment being firehose-off,
        // so it must not inherit GROK_LOG_FILE/GROK_DEBUG_LOG from the host.
        unsafe {
            std::env::remove_var("GROK_LOG_FILE");
            std::env::remove_var("GROK_DEBUG_LOG");
        }

        let result = DebugCommand.run(&mut ctx, "");
        let CommandResult::InjectSkill {
            display_text,
            prompt_blocks,
            display_as_skill,
            scheduled_task_preview,
        } = result
        else {
            panic!("bare /debug must InjectSkill, got {result:?}");
        };
        assert_eq!(display_as_skill, false);
        assert!(scheduled_task_preview.is_none());
        let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
            panic!("expected a text prompt block");
        };
        let home = xai_grok_config::grok_home();
        let expected = debug_log_path(&home, "debug-bare-sess");
        assert!(
            text.text.contains(expected.to_str().unwrap()),
            "prompt block must name the session's real debug log path; \
             got: {}",
            text.text
        );
        assert!(
            text.text.contains("debug what the user just said"),
            "prompt block must direct the model to debug: {}",
            text.text
        );
        assert!(
            display_text.contains("debug-bare-sess.txt"),
            "scrollback display must reference the session log: {display_text}"
        );
        // The ensure-step must have provisioned a real file at that path.
        assert!(
            expected.is_file(),
            "bare /debug must ensure the session log file exists: {expected:?}"
        );
    }

    /// When a single firehose file is configured (`GROK_LOG_FILE` or
    /// `GROK_DEBUG_LOG=<path>`), `run()` must inject THAT file's path — the
    /// only file the firehose actually writes to — not an empty per-session
    /// `<debug>/<sid>.txt` file. Drives the real `run()` glue with the real
    /// env-backed state resolver.
    #[test]
    fn debug_bare_invocation_with_grok_log_file_injects_single_file_path() {
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
            panic!("bare /debug with GROK_LOG_FILE set must InjectSkill, got {result:?}");
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
        // It must NOT fall back to the per-session file: name the session sibling?
        // No — for OnSingleFile the per-session file is never used; assert the
        // text is firehose-on and names the target.
        assert!(
            text.text.contains("confirmed ON"),
            "a configured single file means the firehose is on: {}",
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
