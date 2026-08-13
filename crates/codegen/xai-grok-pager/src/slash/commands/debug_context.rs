//! The execution-context snapshot `/debug` hands the model.
//!
//! `/debug` is a skill whose subject is grok itself, so the injected prompt has
//! to answer "what am I, where do I live, and what did I write down" without a
//! round trip. Everything here is either a pure resolver or a thin `fs`/`env`
//! read, kept out of [`super::debug`] so the text is unit-testable with explicit
//! inputs.

use std::path::{Path, PathBuf};

/// Where the running process stands relative to the installed binary.
///
/// `current_exe()` resolves through the `$GROK_HOME/bin/grok` symlink to the
/// versioned target it pointed at *at exec time*, so an update that re-points
/// the symlink leaves the two disagreeing — the one observable difference
/// between "the code you are reading" and "the code that is running".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryFreshness {
    /// The process is running the installed binary.
    Current,
    /// The installed binary is a different file: grok was updated after this
    /// process started.
    Stale { installed: PathBuf },
    /// No binary at `$GROK_HOME/bin/grok` — a dev build or a vendored install.
    Unmanaged,
    /// `current_exe()` failed; nothing can be said either way.
    Unknown,
}

/// Compare the running binary with the installed one. Both paths must already
/// be canonicalized by the caller, or an unresolved symlink reads as stale.
pub fn binary_freshness(running: Option<&Path>, installed: Option<&Path>) -> BinaryFreshness {
    match (running, installed) {
        (None, _) => BinaryFreshness::Unknown,
        (Some(_), None) => BinaryFreshness::Unmanaged,
        (Some(running), Some(installed)) if running == installed => BinaryFreshness::Current,
        (Some(_), Some(installed)) => BinaryFreshness::Stale {
            installed: installed.to_path_buf(),
        },
    }
}

/// `dunce::canonicalize`, dropped to `None` on any error so a missing or
/// unreadable path never aborts the snapshot.
fn resolve(path: &Path) -> Option<PathBuf> {
    dunce::canonicalize(path).ok()
}

/// A config file the model may want to read, and whether it is actually there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFact {
    pub path: PathBuf,
    pub state: FileState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Present {
        bytes: u64,
        modified: Option<String>,
    },
    Missing,
}

impl FileFact {
    /// Stat `path`. Any `fs` error reads as [`FileState::Missing`]: the model is
    /// told to look, and a file it cannot stat is one it cannot read either.
    pub fn stat(path: PathBuf) -> Self {
        let state = match std::fs::metadata(&path) {
            Ok(meta) => FileState::Present {
                bytes: meta.len(),
                modified: meta.modified().ok().map(format_time),
            },
            Err(_) => FileState::Missing,
        };
        Self { path, state }
    }

    fn render(&self) -> String {
        match &self.state {
            FileState::Present { bytes, modified } => match modified {
                Some(when) => format!("{} ({bytes} bytes, modified {when})", self.path.display()),
                None => format!("{} ({bytes} bytes)", self.path.display()),
            },
            FileState::Missing => format!("{} (missing)", self.path.display()),
        }
    }
}

/// Local-time RFC 3339 rendering of a file timestamp.
fn format_time(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).to_rfc3339()
}

/// One `GROK_*` / `XAI_*` environment variable as it will be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFact {
    pub name: String,
    pub value: EnvValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    Shown(String),
    /// Name-only: the variable is set, but its value looks like a credential.
    Redacted,
}

/// Whether a variable's VALUE must never appear in the prompt. Substring match
/// on the name, so `XAI_API_KEY`, `GROK_AUTH_TOKEN` and anything else spelled
/// like a credential are named but not printed.
pub fn is_secret_name(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CRED",
        "COOKIE",
        "AUTH",
        "SESSION_ID",
    ];
    let upper = name.to_ascii_uppercase();
    MARKERS.iter().any(|m| upper.contains(m))
}

/// Longest value printed verbatim; anything longer is truncated with a marker
/// so one huge variable cannot swamp the block.
const MAX_ENV_VALUE: usize = 200;

/// Select and redact the grok-relevant variables from an environment.
///
/// Pure over the iterator so the redaction is testable without touching the
/// process environment. Sorted by name for a stable, diffable block.
pub fn grok_env_facts<I>(vars: I) -> Vec<EnvFact>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut facts: Vec<EnvFact> = vars
        .into_iter()
        .filter(|(name, _)| name.starts_with("GROK_") || name.starts_with("XAI_"))
        .map(|(name, value)| {
            let value = if is_secret_name(&name) {
                EnvValue::Redacted
            } else if value.chars().count() > MAX_ENV_VALUE {
                let head: String = value.chars().take(MAX_ENV_VALUE).collect();
                EnvValue::Shown(format!("{head}… (truncated)"))
            } else {
                EnvValue::Shown(value)
            };
            EnvFact { name, value }
        })
        .collect();
    facts.sort_by(|a, b| a.name.cmp(&b.name));
    facts
}

/// The model facts `/debug` reports, taken from the pager's own model state so
/// the numbers match what the TUI is acting on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelFacts {
    pub name: Option<String>,
    pub id: Option<String>,
    pub context_window: Option<u64>,
    pub reasoning_effort: Option<String>,
}

/// Everything `/debug` knows about this process, ready to render.
#[derive(Debug, Clone)]
pub struct DebugContext {
    pub version: String,
    pub commit: String,
    pub pid: u32,
    pub running_binary: Option<PathBuf>,
    pub installed_link: PathBuf,
    pub freshness: BinaryFreshness,
    pub grok_home: PathBuf,
    pub grok_home_from_env: bool,
    pub config_files: Vec<FileFact>,
    pub session_id: String,
    pub log: FileFact,
    pub log_summary: String,
    pub cwd: Option<PathBuf>,
    pub model: ModelFacts,
    pub env: Vec<EnvFact>,
}

impl DebugContext {
    /// Read the live process/filesystem state. `log_path` and `log_summary` come
    /// from the caller because the firehose's routing rules live with the
    /// command that resolves them ([`super::debug`]).
    pub fn gather(
        session_id: &str,
        log_path: PathBuf,
        log_summary: String,
        model: ModelFacts,
    ) -> Self {
        let grok_home = xai_grok_config::grok_home();
        let installed_link = xai_grok_config::grok_application_in(&grok_home);
        let running = std::env::current_exe().ok().and_then(|p| resolve(&p));
        let installed = resolve(&installed_link);
        let config_files = config_file_paths(&grok_home)
            .into_iter()
            .map(FileFact::stat)
            .collect();
        Self {
            version: xai_grok_version::installed(),
            commit: xai_grok_version::BUILD_COMMIT_SHORT.to_string(),
            pid: std::process::id(),
            freshness: binary_freshness(running.as_deref(), installed.as_deref()),
            running_binary: running,
            installed_link,
            grok_home_from_env: std::env::var_os("GROK_HOME").is_some(),
            grok_home,
            config_files,
            session_id: session_id.to_string(),
            log: FileFact::stat(log_path),
            log_summary,
            cwd: std::env::current_dir().ok(),
            model,
            env: grok_env_facts(std::env::vars()),
        }
    }

    /// The whole model-facing message: the user's request, the context block,
    /// and how to attack it.
    pub fn render(&self, request: &str) -> String {
        let mut out = String::new();
        out.push_str(
            "You are in DEBUG mode for this grok session. This is a self-debugging \
             skill: the subject under investigation is grok itself — this process, \
             the binary it is running, its configuration, and what it logged — not \
             the user's project.\n\n",
        );
        out.push_str("WHAT THE USER WANTS DEBUGGED\n");
        let request = request.trim();
        if request.is_empty() {
            out.push_str("(no argument given — debug what the user says next)\n");
        } else {
            out.push_str(request);
            out.push('\n');
        }
        out.push_str("\nEXECUTION CONTEXT\n");
        out.push_str(&self.context_block());
        out.push_str(
            "\nHOW TO GO AT IT\n\
             - Read the session log above with your file tools; it is the firehose \
             this process wrote, and it usually names the subsystem that made the \
             decision. Search it for the value, flag, or model id the user is asking \
             about instead of reading it end to end.\n\
             - Read the config files listed above, every layer of them: a value the \
             user did not choose usually came from one of those files, from a \
             GROK_*/XAI_* variable, or from a built-in default in the binary.\n\
             - Check the numbers against what this process actually resolved (model \
             id, context window, effort above) rather than what the model or the docs \
             are supposed to say. When those two disagree, that gap IS the bug.\n\
             - Run things. `grok --version`, listing the binary's directory, grepping \
             the log, reading config — you have shell and file tools here and they \
             answer these questions in seconds.\n\
             - If a source checkout of grok is available, verify against the source \
             for this commit; otherwise reason from the binary, the config, and the \
             log, and say which one you used.\n\
             - Trust what you observe over what you remember about how grok works. \
             This binary is a fork and may not behave the way upstream does.\n\
             \n\
             Then report the root cause plainly, with the evidence that proves it \
             (log lines, config keys, file paths), and fix what is broken. If the fix \
             is outside this session's reach, say exactly what to change and where.\n",
        );
        out
    }

    /// The labeled facts, one per line.
    fn context_block(&self) -> String {
        let mut lines: Vec<(&str, String)> = Vec::new();
        lines.push((
            "Version",
            format!("{} (commit {})", self.version, self.commit),
        ));
        lines.push(("PID", self.pid.to_string()));
        lines.push((
            "Running binary",
            match &self.running_binary {
                Some(path) => path.display().to_string(),
                None => "unknown (current_exe() failed)".to_string(),
            },
        ));
        lines.push(("Installed grok", self.freshness_line()));
        lines.push((
            "Config dir",
            format!(
                "{} ({})",
                self.grok_home.display(),
                if self.grok_home_from_env {
                    "from $GROK_HOME"
                } else {
                    "default ~/.grok"
                }
            ),
        ));
        for (i, file) in self.config_files.iter().enumerate() {
            lines.push((if i == 0 { "Config files" } else { "" }, file.render()));
        }
        lines.push(("Session id", self.session_id.clone()));
        // Size included: an empty file is the difference between "the firehose
        // has nothing to say" and "nothing was ever written here".
        lines.push((
            "Session log",
            format!("{} — {}", self.log.render(), self.log_summary),
        ));
        lines.push((
            "Process cwd",
            match &self.cwd {
                Some(cwd) => cwd.display().to_string(),
                None => "unknown".to_string(),
            },
        ));
        lines.push((
            "Model",
            match (&self.model.name, &self.model.id) {
                (Some(name), Some(id)) => format!("{name} (id: {id})"),
                (None, Some(id)) => id.clone(),
                (Some(name), None) => name.clone(),
                (None, None) => "none selected".to_string(),
            },
        ));
        lines.push((
            "Context window",
            match self.model.context_window {
                Some(tokens) => format!("{tokens} tokens, as this session resolved it"),
                None => "not reported to the TUI for this model".to_string(),
            },
        ));
        lines.push((
            "Reasoning effort",
            self.model
                .reasoning_effort
                .clone()
                .unwrap_or_else(|| "not set".to_string()),
        ));
        if self.env.is_empty() {
            lines.push(("Environment", "no GROK_*/XAI_* variables set".to_string()));
        }
        for (i, fact) in self.env.iter().enumerate() {
            let rendered = match &fact.value {
                EnvValue::Shown(v) => format!("{}={v}", fact.name),
                EnvValue::Redacted => format!("{} is set (value withheld)", fact.name),
            };
            lines.push((if i == 0 { "Environment" } else { "" }, rendered));
        }

        let width = lines
            .iter()
            .map(|(label, _)| label.len())
            .max()
            .unwrap_or(0);
        lines
            .into_iter()
            .map(|(label, value)| {
                let label = if label.is_empty() {
                    " ".repeat(width + 1)
                } else {
                    format!("{label}:{}", " ".repeat(width - label.len()))
                };
                format!("{label} {value}\n")
            })
            .collect()
    }

    /// The installed-binary line, carrying the staleness warning when the
    /// running process is not the installed build.
    fn freshness_line(&self) -> String {
        match &self.freshness {
            BinaryFreshness::Current => format!(
                "{} — this IS the running binary",
                self.installed_link.display()
            ),
            BinaryFreshness::Stale { installed } => format!(
                "{} -> {} — STALE: you are NOT running this. grok was updated after \
                 this process started, so what is on disk can already differ from \
                 what you observe here. Check this before blaming the code you read.",
                self.installed_link.display(),
                installed.display()
            ),
            BinaryFreshness::Unmanaged => format!(
                "none at {} — this process runs an unmanaged build (dev or vendored)",
                self.installed_link.display()
            ),
            BinaryFreshness::Unknown => format!(
                "{} — cannot compare: current_exe() failed",
                self.installed_link.display()
            ),
        }
    }
}

/// Every config layer grok loads, in apply order. Named whether or not they
/// exist: "the file you would edit is missing" is an answer too.
fn config_file_paths(grok_home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(system) = xai_grok_config::system_config_dir() {
        paths.push(system.join(xai_grok_config::MANAGED_CONFIG_FILENAME));
    }
    paths.push(grok_home.join(xai_grok_config::MANAGED_CONFIG_FILENAME));
    paths.push(grok_home.join(xai_grok_config::USER_CONFIG_FILENAME));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_flags_a_replaced_install_as_stale() {
        let state = binary_freshness(
            Some(Path::new("/h/.grok/versions/0.2.7/grok")),
            Some(Path::new("/h/.grok/versions/0.2.8/grok")),
        );
        assert_eq!(
            state,
            BinaryFreshness::Stale {
                installed: PathBuf::from("/h/.grok/versions/0.2.8/grok")
            }
        );
    }

    #[test]
    fn freshness_is_current_when_paths_match_and_unmanaged_without_an_install() {
        assert_eq!(
            binary_freshness(Some(Path::new("/h/grok")), Some(Path::new("/h/grok"))),
            BinaryFreshness::Current
        );
        assert_eq!(
            binary_freshness(Some(Path::new("/build/target/debug/grok")), None),
            BinaryFreshness::Unmanaged
        );
        assert_eq!(
            binary_freshness(None, Some(Path::new("/h/grok"))),
            BinaryFreshness::Unknown
        );
    }

    #[test]
    fn stale_line_warns_and_names_both_binaries() {
        let ctx = sample_context(BinaryFreshness::Stale {
            installed: PathBuf::from("/h/.grok/versions/0.2.8/grok"),
        });
        let line = ctx.freshness_line();
        assert!(line.contains("STALE"), "must shout staleness: {line}");
        assert!(
            line.contains("/h/.grok/versions/0.2.8/grok") && line.contains("/h/.grok/bin/grok"),
            "must name the installed target and the link: {line}"
        );
    }

    #[test]
    fn secret_shaped_variables_are_named_but_never_printed() {
        let facts = grok_env_facts([
            (
                "XAI_API_KEY".to_string(),
                "sk-live-do-not-print".to_string(),
            ),
            ("GROK_AUTH_TOKEN".to_string(), "bearer-nope".to_string()),
            ("GROK_HOME".to_string(), "/h/.grok".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        let names: Vec<&str> = facts.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["GROK_AUTH_TOKEN", "GROK_HOME", "XAI_API_KEY"],
            "only GROK_*/XAI_* vars, sorted"
        );
        for fact in &facts {
            match (fact.name.as_str(), &fact.value) {
                ("GROK_HOME", EnvValue::Shown(v)) => assert_eq!(v, "/h/.grok"),
                ("GROK_HOME", other) => panic!("GROK_HOME must be shown, got {other:?}"),
                (_, EnvValue::Redacted) => {}
                (name, other) => panic!("{name} must be redacted, got {other:?}"),
            }
        }
    }

    #[test]
    fn long_env_values_are_truncated() {
        let facts = grok_env_facts([("GROK_BASE_URL".to_string(), "x".repeat(500))]);
        let EnvValue::Shown(value) = &facts[0].value else {
            panic!("expected a shown value");
        };
        assert!(value.contains("(truncated)"), "{value}");
        assert!(value.chars().count() < 250, "{}", value.chars().count());
    }

    #[test]
    fn stat_reports_size_for_a_real_file_and_missing_for_an_absent_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let present = dir.path().join("config.toml");
        std::fs::write(&present, "model = \"grok-4.5\"\n").expect("write");
        let fact = FileFact::stat(present.clone());
        match fact.state {
            FileState::Present { bytes, .. } => assert_eq!(bytes, 19),
            other => panic!("expected Present, got {other:?}"),
        }
        assert!(fact.render().contains("bytes"), "{}", fact.render());

        let absent = FileFact::stat(dir.path().join("managed_config.toml"));
        assert_eq!(absent.state, FileState::Missing);
        assert!(
            absent.render().ends_with("(missing)"),
            "{}",
            absent.render()
        );
    }

    #[test]
    fn config_paths_cover_every_layer_grok_loads() {
        let paths = config_file_paths(Path::new("/h/.grok"));
        assert!(
            paths.contains(&PathBuf::from("/h/.grok/config.toml"))
                && paths.contains(&PathBuf::from("/h/.grok/managed_config.toml")),
            "{paths:?}"
        );
        if cfg!(unix) {
            assert!(
                paths.contains(&PathBuf::from("/etc/grok/managed_config.toml")),
                "the system layer must be named on unix: {paths:?}"
            );
        }
    }

    #[test]
    fn render_carries_the_request_the_paths_and_the_marching_orders() {
        let ctx = sample_context(BinaryFreshness::Current);
        let text = ctx.render("  why was the context size defaulted to 256k?  ");
        assert!(
            text.contains("why was the context size defaulted to 256k?"),
            "the user's question must reach the model verbatim: {text}"
        );
        for expected in [
            "/h/.grok/debug/sid.txt", // the log to read
            "/h/.grok/config.toml",   // the config to read
            "262144",                 // the number the user is asking about
            "grok-4.5",               // the model it belongs to
            "GROK_DEBUG_LOG=1",       // how this process was launched
            "0.2.7",                  // what is running
        ] {
            assert!(text.contains(expected), "missing {expected:?} in: {text}");
        }
        assert!(
            text.contains("DEBUG mode") && text.contains("HOW TO GO AT IT"),
            "framing and instructions must both survive: {text}"
        );
    }

    #[test]
    fn render_without_a_request_points_at_the_next_message() {
        let text = sample_context(BinaryFreshness::Current).render("   ");
        assert!(
            text.contains("debug what the user says next"),
            "a bare /debug must still be actionable: {text}"
        );
    }

    #[test]
    fn a_secret_env_value_never_reaches_the_rendered_prompt() {
        let mut ctx = sample_context(BinaryFreshness::Current);
        ctx.env = grok_env_facts([("XAI_API_KEY".to_string(), "sk-live-leaked".to_string())]);
        let text = ctx.render("check auth");
        assert!(
            !text.contains("sk-live-leaked"),
            "leaked a credential: {text}"
        );
        assert!(
            text.contains("XAI_API_KEY is set (value withheld)"),
            "the variable must still be named: {text}"
        );
    }

    fn sample_context(freshness: BinaryFreshness) -> DebugContext {
        DebugContext {
            version: "0.2.7".to_string(),
            commit: "abc1234".to_string(),
            pid: 4242,
            running_binary: Some(PathBuf::from("/h/.grok/versions/0.2.7/grok")),
            installed_link: PathBuf::from("/h/.grok/bin/grok"),
            freshness,
            grok_home: PathBuf::from("/h/.grok"),
            grok_home_from_env: false,
            config_files: vec![FileFact {
                path: PathBuf::from("/h/.grok/config.toml"),
                state: FileState::Present {
                    bytes: 120,
                    modified: None,
                },
            }],
            session_id: "sid".to_string(),
            log: FileFact {
                path: PathBuf::from("/h/.grok/debug/sid.txt"),
                state: FileState::Present {
                    bytes: 8192,
                    modified: None,
                },
            },
            log_summary: "firehose ON (per-session routing)".to_string(),
            cwd: Some(PathBuf::from("/workspace")),
            model: ModelFacts {
                name: Some("Grok 4.5".to_string()),
                id: Some("grok-4.5".to_string()),
                context_window: Some(262_144),
                reasoning_effort: Some("high".to_string()),
            },
            env: grok_env_facts([("GROK_DEBUG_LOG".to_string(), "1".to_string())]),
        }
    }
}
