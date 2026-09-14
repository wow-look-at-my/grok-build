//! `ci` — read GitHub CI state for the branch this session is working on.
//!
//! The pipeline this tool exists for is commit → push → wait for CI → read the
//! failing logs → fix → push again. Under `--sandbox` a `gh` spawned from the
//! session reaches neither the host credentials nor the network, so every
//! query here rides the same unsandboxed host worker the status dot polls
//! ([`xai_grok_sandbox::ci_host`]). The worker's allowlist is what keeps this
//! read-only: no rerun, no cancel, no merge.

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use xai_grok_sandbox::ci_state::{self, CiStatus};

pub const CI_TOOL_NAME: &str = "ci";

/// How long one `wait` call may block before reporting what it last saw.
const DEFAULT_WAIT_SECS: u64 = 300;
const MAX_WAIT_SECS: u64 = 1800;
/// Gap between polls while waiting. `gh run list` is one API call, and a
/// workflow's state does not move faster than this.
const WAIT_POLL_SECS: u64 = 15;

/// How much of a failing log one call returns. The log's tail is what carries
/// the error, so an oversized body is cut from the front.
const LOG_TAIL_BYTES: usize = 24_000;

const DEFAULT_RUN_LIMIT: u32 = 10;
const MAX_RUN_LIMIT: u32 = 50;

// ---------------------------------------------------------------------------
// Input schema
// ---------------------------------------------------------------------------

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CiAction {
    /// Fold the branch's runs into one state: passing, failing, in_progress,
    /// or none.
    Status,
    /// List the branch's recent runs with their ids, workflows and states.
    Runs,
    /// Block until the branch's runs settle, then report the state.
    Wait,
    /// Return the failing steps' logs for a run.
    Logs,
    /// Report the checks on the pull request for this branch.
    Checks,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiInput {
    #[schemars(
        description = "What to ask about CI. `status` folds the branch's runs into one state; `runs` lists them with their ids; `wait` blocks until they settle; `logs` returns the failing steps' output for a run; `checks` reports the pull request's checks."
    )]
    pub action: CiAction,

    #[serde(default)]
    #[schemars(
        description = "Branch to ask about. Defaults to the checked-out branch, which is what you just pushed."
    )]
    pub branch: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Run id for `logs`. Defaults to the newest failing run on the branch, which is the one to read after a red status."
    )]
    pub run_id: Option<String>,

    #[serde(default)]
    #[schemars(description = "How many runs `runs` returns. Defaults to 10, capped at 50.")]
    pub limit: Option<u32>,

    #[serde(default)]
    #[schemars(
        description = "How long `wait` may block, in seconds. Defaults to 300, capped at 1800. A wait that runs out reports the state it last saw rather than failing."
    )]
    pub timeout_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiRunSummary {
    pub workflow: String,
    pub status: String,
    pub conclusion: String,
    pub run_id: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiOutput {
    /// `passing`, `failing`, `in_progress`, or `none`. `none` means the branch
    /// has no runs at all, which is not the same as passing.
    pub state: String,
    pub branch: String,
    /// Whether anything on this branch can still change on its own.
    pub settled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<CiRunSummary>,
    /// Log or check text, for the actions that return it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether the returned text had its head cut off.
    #[serde(default)]
    pub truncated: bool,
    /// What the caller should understand from this answer.
    pub summary: String,
}

impl xai_tool_runtime::ToolOutput for CiOutput {}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The `gh` argv for a branch's run list, kept in one place so the tool and
/// the stop gate ask the same question.
pub fn run_list_args<'a>(branch: &'a str, limit: &'a str) -> Vec<&'a str> {
    vec![
        "run",
        "list",
        "--branch",
        branch,
        "--limit",
        limit,
        "--json",
        ci_state::RUN_JSON_FIELDS,
    ]
}

/// Why a run list could not be read. A branch with no runs is not one of these: that is an empty `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiQueryError {
    /// No `gh` could be run at all: none installed, or the host worker refused the request.
    Unreachable,
    /// `gh` ran and failed. A dead token and a rate limit both land here, with what `gh` said.
    Failed { code: i32, stderr: String },
    /// `gh` exited 0 with a body that is not a run list.
    Unparseable { stdout: String },
}

impl std::fmt::Display for CiQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiQueryError::Unreachable => write!(
                f,
                "Could not run `gh`. In a sandboxed session the host worker answers these queries; outside one, `gh` must be installed."
            ),
            CiQueryError::Failed { code, stderr } => {
                write!(f, "`gh run list` failed (exit {code}): {}", stderr.trim())
            }
            CiQueryError::Unparseable { stdout } => {
                write!(
                    f,
                    "`gh run list` returned something that is not a run list: {}",
                    stdout.trim()
                )
            }
        }
    }
}

/// Read a branch's runs through whichever `gh` path this process can reach.
///
/// An empty `Ok` means the branch has no runs. Every failure to ask is an `Err` that says what `gh` said, so a dead token never reads as "nothing pushed".
pub fn fetch_runs(
    cwd: &std::path::Path,
    branch: &str,
    limit: u32,
) -> Result<Vec<ci_state::GhRun>, CiQueryError> {
    let limit = limit.clamp(1, MAX_RUN_LIMIT).to_string();
    let response = xai_grok_sandbox::ci_host::run_gh(cwd, &run_list_args(branch, &limit))
        .ok_or(CiQueryError::Unreachable)?;
    if !response.success() {
        return Err(CiQueryError::Failed {
            code: response.code,
            stderr: response.stderr,
        });
    }
    if response.stdout.trim() == "[]" {
        return Ok(Vec::new());
    }
    let Some(mut runs) = ci_state::parse_gh_runs(response.stdout.as_bytes()) else {
        return Err(CiQueryError::Unparseable {
            stdout: response.stdout,
        });
    };
    // `--branch` filters server-side; this is the belt to those suspenders,
    // because one cancelled run from another branch is enough to report red.
    runs.retain(|run| run.head_branch.as_deref().is_none_or(|head| head == branch));
    Ok(runs)
}

/// The branch the session is on, read from git rather than guessed.
pub fn current_branch(cwd: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// The newest run that failed, which is the one whose logs answer "why is this
/// branch red".
fn newest_failing_run(runs: &[ci_state::GhRun]) -> Option<u64> {
    ci_state::newest_run_per_workflow(runs.iter().cloned().collect::<Vec<_>>())
        .iter()
        .find(|run| run.is_terminal_failure())
        .and_then(|run| run.database_id)
}

fn summarize(runs: &[ci_state::GhRun]) -> Vec<CiRunSummary> {
    runs.iter()
        .map(|run| CiRunSummary {
            workflow: run.workflow_name.clone(),
            status: run.status.clone(),
            conclusion: run.conclusion.clone(),
            run_id: run.database_id,
        })
        .collect()
}

/// Cut a body to its last `max` bytes on a character boundary.
fn tail(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

/// The sentence a model reads off a state, phrased as what to do next.
fn state_summary(state: CiStatus, branch: &str) -> String {
    match state {
        CiStatus::Green => format!("CI is passing on {branch}."),
        CiStatus::Red => format!(
            "CI is FAILING on {branch}. Read the failing logs (action `logs`), fix the cause, and push again."
        ),
        CiStatus::Yellow => format!(
            "CI is still running on {branch}. Work on something else, or call `wait` to block until it settles."
        ),
        CiStatus::Off => format!(
            "No CI runs for {branch}. Nothing has been pushed yet, or this repository runs no workflows."
        ),
    }
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct CiTool;

impl crate::types::tool_metadata::ToolMetadata for CiTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Ci
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Read GitHub CI state for the branch you are working on: fold it to one state, list runs, block until they settle, read a failing run's logs, or report a pull request's checks. Read-only, and it works inside the sandbox, where `gh` run from a shell does not."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for CiTool {
    type Args = CiInput;
    type Output = CiOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(CI_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            CI_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "new_tool.ci", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: CiInput,
    ) -> Result<CiOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::{resolve_cwd, shared_resources};
        let resources = shared_resources(&ctx)?;
        let cwd = resolve_cwd(&ctx, &resources).await?;

        // Every query shells out, so the whole call runs off the async
        // executor rather than blocking a reactor thread.
        tokio::task::spawn_blocking(move || run_blocking(&cwd, input))
            .await
            .map_err(|error| {
                xai_tool_runtime::ToolError::custom(
                    "ci_join",
                    format!("ci query panicked: {error}"),
                )
            })?
    }
}

/// The whole tool, off the executor and free of async: a blocking `gh` call
/// per poll, which is also what makes it directly testable.
fn run_blocking(
    cwd: &std::path::Path,
    input: CiInput,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let branch = match input.branch.clone().or_else(|| current_branch(cwd)) {
        Some(branch) => branch,
        None => {
            return Err(xai_tool_runtime::ToolError::custom(
                "ci_no_branch",
                "Could not determine the current branch. Pass `branch` explicitly.",
            ));
        }
    };
    let limit = input.limit.unwrap_or(DEFAULT_RUN_LIMIT);
    match input.action {
        CiAction::Status | CiAction::Runs => status_output(cwd, &branch, limit),
        CiAction::Wait => wait_output(cwd, &branch, limit, input.timeout_secs),
        CiAction::Logs => logs_output(cwd, &branch, input.run_id.as_deref()),
        CiAction::Checks => Ok(checks_output(cwd, &branch)),
    }
}

/// A failed `gh` query is the tool's error, never an empty answer.
fn query_error(error: CiQueryError) -> xai_tool_runtime::ToolError {
    let code = match error {
        CiQueryError::Unreachable => "ci_gh_unavailable",
        CiQueryError::Failed { .. } => "ci_gh_failed",
        CiQueryError::Unparseable { .. } => "ci_gh_unparseable",
    };
    xai_tool_runtime::ToolError::custom(code, error.to_string())
}

fn status_output(
    cwd: &std::path::Path,
    branch: &str,
    limit: u32,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let runs = fetch_runs(cwd, branch, limit).map_err(query_error)?;
    let state = ci_state::ci_from_runs(runs.iter().cloned().collect::<Vec<_>>());
    Ok(CiOutput {
        state: state.as_str().to_string(),
        branch: branch.to_string(),
        settled: state.is_terminal(),
        runs: summarize(&runs),
        text: None,
        truncated: false,
        summary: state_summary(state, branch),
    })
}

/// Poll until the branch's runs settle or the budget runs out.
///
/// A timeout is not a failure: it answers with the state it last saw, so the
/// caller learns the branch is still moving rather than that the tool broke.
fn wait_output(
    cwd: &std::path::Path,
    branch: &str,
    limit: u32,
    timeout_secs: Option<u64>,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let budget = std::time::Duration::from_secs(
        timeout_secs.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS),
    );
    let deadline = std::time::Instant::now() + budget;
    loop {
        let output = status_output(cwd, branch, limit)?;
        if output.settled || std::time::Instant::now() >= deadline {
            if !output.settled {
                return Ok(CiOutput {
                    summary: format!(
                        "Waited {}s and CI is still running on {branch}. Do other work and ask again.",
                        budget.as_secs()
                    ),
                    ..output
                });
            }
            return Ok(output);
        }
        std::thread::sleep(std::time::Duration::from_secs(WAIT_POLL_SECS));
    }
}

fn logs_output(
    cwd: &std::path::Path,
    branch: &str,
    run_id: Option<&str>,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let runs = fetch_runs(cwd, branch, DEFAULT_RUN_LIMIT).map_err(query_error)?;
    let state = ci_state::ci_from_runs(runs.iter().cloned().collect::<Vec<_>>());
    let run_id = match run_id {
        Some(id) => id.to_string(),
        None => match newest_failing_run(&runs) {
            Some(id) => id.to_string(),
            None => {
                return Err(xai_tool_runtime::ToolError::custom(
                    "ci_no_failing_run",
                    format!(
                        "No failing run on {branch} to read logs from (state: {}). Pass `run_id` to read a specific run.",
                        state.as_str()
                    ),
                ));
            }
        },
    };
    let response = xai_grok_sandbox::ci_host::run_gh(cwd, &["run", "view", &run_id, "--log-failed"])
        .ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "ci_gh_unavailable",
                "Could not reach `gh`. In a sandboxed session the host worker answers these queries; outside one, `gh` must be installed and authenticated.",
            )
        })?;
    // A run whose failure is a startup failure has no job log at all, and `gh`
    // says so on stderr. Reporting an empty body instead would read as "the
    // job printed nothing", which sends the reader looking in the wrong place.
    let body = if response.stdout.trim().is_empty() {
        response.stderr.clone()
    } else {
        response.stdout.clone()
    };
    let (text, cut) = tail(&body, LOG_TAIL_BYTES);
    Ok(CiOutput {
        state: state.as_str().to_string(),
        branch: branch.to_string(),
        settled: state.is_terminal(),
        runs: summarize(&runs),
        text: Some(text),
        truncated: cut || response.truncated,
        summary: format!("Failing-step logs for run {run_id} on {branch}."),
    })
}

fn checks_output(cwd: &std::path::Path, branch: &str) -> CiOutput {
    // `gh pr checks` exits non-zero when a check is failing, so its exit code
    // carries meaning and is not an error to report as one.
    let response = xai_grok_sandbox::ci_host::run_gh(cwd, &["pr", "checks", branch])
        .unwrap_or_else(|| xai_grok_sandbox::ci_host::GhHostResponse {
            code: -1,
            stdout: String::new(),
            stderr: "could not reach `gh`".to_string(),
            truncated: false,
        });
    let body = if response.stdout.trim().is_empty() {
        response.stderr.clone()
    } else {
        response.stdout.clone()
    };
    let (text, cut) = tail(&body, LOG_TAIL_BYTES);
    let state = if response.success() {
        CiStatus::Green
    } else {
        CiStatus::Red
    };
    CiOutput {
        state: state.as_str().to_string(),
        branch: branch.to_string(),
        settled: true,
        runs: Vec::new(),
        text: Some(text),
        truncated: cut || response.truncated,
        summary: format!("Pull-request checks for {branch}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(workflow: &str, status: &str, conclusion: &str, id: u64) -> ci_state::GhRun {
        ci_state::GhRun {
            status: status.to_string(),
            conclusion: conclusion.to_string(),
            head_branch: Some("feat/x".into()),
            workflow_name: workflow.to_string(),
            database_id: Some(id),
        }
    }

    /// A dead token answers with exit 1 and a reason on stderr. The model must
    /// read that reason, never "no runs".
    #[test]
    fn a_failed_gh_is_an_error_that_carries_what_gh_said() {
        let error = query_error(CiQueryError::Failed {
            code: 1,
            stderr: "HTTP 403: API rate limit exceeded\n".into(),
        });
        let text = error.to_string();
        assert!(text.contains("exit 1"), "{text}");
        assert!(text.contains("API rate limit exceeded"), "{text}");
        assert!(!text.contains("No CI runs"), "{text}");
        assert!(
            query_error(CiQueryError::Unreachable)
                .to_string()
                .contains("Could not run `gh`")
        );
    }

    #[test]
    fn the_run_list_query_asks_for_every_field_the_parser_reads() {
        let args = run_list_args("feat/x", "10");
        assert!(args.contains(&ci_state::RUN_JSON_FIELDS));
        assert!(args.contains(&"feat/x"));
        // The worker refuses anything outside its allowlist, so a query shape
        // this tool cannot send is a query it must not build.
        let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&owned));
    }

    #[test]
    fn the_logs_query_is_allowed_too() {
        let owned: Vec<String> = ["run", "view", "12345", "--log-failed"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&owned));
        let checks: Vec<String> = ["pr", "checks", "feat/x"]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&checks));
    }

    #[test]
    fn logs_default_to_the_newest_failing_run() {
        // gh lists newest first. The run to read is the failing one, not the
        // newest, and not one an older push already superseded.
        let runs = vec![
            run("Release", "in_progress", "", 3),
            run("CI", "completed", "failure", 2),
            run("CI", "completed", "success", 1),
        ];
        assert_eq!(newest_failing_run(&runs), Some(2));
        // Nothing failing: the caller is told to name a run instead.
        let green = vec![run("CI", "completed", "success", 9)];
        assert_eq!(newest_failing_run(&green), None);
    }

    #[test]
    fn a_superseded_failure_does_not_become_the_log_target() {
        // The newest CI run is live. The failure behind it belongs to a push
        // that this one replaced, so there is nothing to read yet.
        let runs = vec![
            run("CI", "in_progress", "", 5),
            run("CI", "completed", "failure", 4),
        ];
        assert_eq!(newest_failing_run(&runs), None);
    }

    #[test]
    fn every_state_tells_the_caller_what_to_do_next() {
        assert!(state_summary(CiStatus::Red, "feat/x").contains("logs"));
        assert!(state_summary(CiStatus::Yellow, "feat/x").contains("wait"));
        assert!(state_summary(CiStatus::Green, "feat/x").contains("passing"));
        // "No runs" must never read as "passing": nothing has been pushed.
        let none = state_summary(CiStatus::Off, "feat/x");
        assert!(none.contains("No CI runs"));
        assert!(!none.contains("passing"));
    }

    #[test]
    fn a_cut_body_keeps_its_tail_on_a_character_boundary() {
        let (text, cut) = tail("noise error: the real failure", 22);
        assert!(cut);
        assert_eq!(text, "error: the real failure".trim_start_matches("e"));
        let (whole, uncut) = tail("short", 100);
        assert_eq!(whole, "short");
        assert!(!uncut);
        // A multi-byte character straddling the cut must not panic.
        let wide = "aaaa\u{1F600}bbbb";
        let (tail_text, _) = tail(wide, 6);
        assert!(wide.ends_with(&tail_text));
    }
}
