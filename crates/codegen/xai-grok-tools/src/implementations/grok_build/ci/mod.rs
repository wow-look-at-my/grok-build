//! `ci` - read GitHub CI state for the branch this session is working on.

use crate::notification::types::{MonitorEvent, ToolNotificationHandle};
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::{NotificationHandle, OwnerSessionId};
use crate::types::tool::{ToolKind, ToolNamespace};
use xai_grok_sandbox::ci_state::{self, CiStatus};

pub const CI_TOOL_NAME: &str = "ci";

/// How long one `wait` keeps polling before its notification reports what it last saw.
const DEFAULT_WAIT_SECS: u64 = 300;
const MAX_WAIT_SECS: u64 = 1800;
/// Gap between polls while waiting.
const WAIT_POLL_SECS: u64 = 15;

/// How much of a failing log one call returns.
const LOG_TAIL_BYTES: usize = 24_000;

const DEFAULT_RUN_LIMIT: u32 = 10;
const MAX_RUN_LIMIT: u32 = 50;

// Input schema

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CiAction {
    /// Fold the branch's runs into one state: passing, failing, in_progress, or none.
    Status,
    /// List the branch's recent runs with their ids, workflows and states.
    Runs,
    /// Watch until the branch's runs settle, then notify with the state.
    Wait,
    /// Return the failing steps' logs for a run.
    Logs,
    /// Report the checks on the pull request for this branch.
    Checks,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiInput {
    #[schemars(
        description = "What to ask about CI. `status` folds the branch's runs into one state; `runs` lists them with their ids; `wait` watches until they settle and notifies you when they do; `logs` returns the failing steps' output for a run; `checks` reports the pull request's checks."
    )]
    pub action: CiAction,

    #[serde(default)]
    #[schemars(
        description = "Branch to ask about. Defaults to the checked-out branch, which is what you just pushed."
    )]
    pub branch: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Repository to ask about, as `owner/name`. Defaults to the repository the session's working directory is a checkout of. Set it when the CI you need belongs to another repository."
    )]
    pub repo: Option<String>,

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
        description = "How long `wait` keeps watching in the background, in seconds. Defaults to 300, capped at 1800. A wait that runs out reports the state it last saw rather than failing."
    )]
    pub timeout_secs: Option<u64>,
}

// Output

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiRunSummary {
    pub workflow: String,
    pub status: String,
    pub conclusion: String,
    pub run_id: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct CiOutput {
    /// `passing`, `failing`, `in_progress`, `none`, or `pending`.
    pub state: String,
    pub branch: String,
    /// Handle for the background query this call accepted. The result arrives later as a notification carrying this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
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

impl CiOutput {
    /// The handle a background query answers the call with: the request is
    /// accepted, and the real result will arrive as a notification named by
    /// `task_id`.
    fn started(action: CiAction, branch: String, task_id: String) -> Self {
        Self {
            state: "pending".to_string(),
            branch,
            task_id: Some(task_id.clone()),
            settled: false,
            runs: Vec::new(),
            text: None,
            truncated: false,
            summary: format!(
                "CI `{}` started (task {task_id}). Its result will arrive as a <monitor-event> notification; do not poll for it or block on it.",
                action_label(action)
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The `gh` argv for a branch's run list, kept in one place so the tool and
/// the stop gate ask the same question.
pub fn run_list_args<'a>(branch: &'a str, limit: &'a str, repo: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec![
        "run",
        "list",
        "--branch",
        branch,
        "--limit",
        limit,
        "--json",
        ci_state::RUN_JSON_FIELDS,
    ];
    push_repo(&mut args, repo);
    args
}

/// Name the repository on the command line rather than letting `gh` discover
/// it.
fn push_repo<'a>(args: &mut Vec<&'a str>, repo: Option<&'a str>) {
    if let Some(repo) = repo {
        args.extend(["--repo", repo]);
    }
}

/// The `owner/name` of the repository at `cwd`, read from its `origin`
/// remote. One local `git` call, so naming the repository costs no API
/// request.
pub fn remote_repo(cwd: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    repo_from_remote_url(String::from_utf8_lossy(&output.stdout).trim())
}

/// `owner/name` out of the remote forms git accepts: `https://host/o/r`,
/// `ssh://git@host/o/r`, `git@host:o/r`, each with an optional trailing `.git`
/// or `/`. `None` when what is left does not end in usable segments.
fn repo_from_remote_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .unwrap_or(url);
    let url = url.strip_prefix("git@").unwrap_or(url);
    // A scp-style remote separates host from path with a colon and a
    // URL-style one with a slash.
    let path = match url.split_once(['/', ':']) {
        Some((_, path)) => path,
        None => url,
    };
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let owner = *segments.get(segments.len().checked_sub(2)?)?;
    let name = *segments.last()?;
    let repo = format!("{owner}/{name}");
    valid_repo_token(&repo).then_some(repo)
}

fn valid_repo_token(token: &str) -> bool {
    let Some((owner, name)) = token.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && !owner.starts_with('-')
        && !name.starts_with('-')
        && owner.len() <= 200
        && name.len() <= 200
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/'))
}

/// The repository to query: the caller's, named by the git remote at `cwd`.
/// An explicitly requested name that is not a valid `owner/name` is refused
/// rather than quietly replaced by the session's own repository.
fn query_repo(cwd: &std::path::Path, requested: Option<&str>) -> Result<Option<String>, String> {
    match requested {
        Some(repo) if valid_repo_token(repo) => Ok(Some(repo.to_string())),
        Some(repo) => Err(format!(
            "`repo` must be `owner/name`; got {repo:?}. Name the repository as its git remote does, without a host or a scheme."
        )),
        None => Ok(remote_repo(cwd)),
    }
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
/// `repo` must be a name [`valid_repo_token`] accepted, or `None` to ask the
/// repository the git remote at `cwd` points at. An empty `Ok` means the
/// branch has no runs. Every failure to ask is an `Err` that says what `gh`
/// said, so a dead token never reads as "nothing pushed".
pub fn fetch_runs(
    cwd: &std::path::Path,
    branch: &str,
    limit: u32,
    repo: Option<&str>,
) -> Result<Vec<ci_state::GhRun>, CiQueryError> {
    let limit = limit.clamp(1, MAX_RUN_LIMIT).to_string();
    let named = repo.map(str::to_string).or_else(|| remote_repo(cwd));
    let response =
        xai_grok_sandbox::ci_host::run_gh(cwd, &run_list_args(branch, &limit, named.as_deref()))
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
    // `--branch` filters server-side; this is the belt to those suspenders.
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

/// The sentence a model reads off a state, phrased as what to do next. `repo`
/// names the repository the query went to, so an empty answer says where it
/// was empty rather than guessing what the repository has.
fn state_summary(state: CiStatus, branch: &str, repo: Option<&str>) -> String {
    match state {
        CiStatus::Green => format!("CI is passing on {branch}."),
        CiStatus::Red => format!(
            "CI is FAILING on {branch}. Read the failing logs (action `logs`), fix the cause, and push again."
        ),
        CiStatus::Yellow => format!(
            "CI is still running on {branch}. Work on something else, and call `wait` to be notified when it settles."
        ),
        CiStatus::Off => match repo {
            Some(repo) => format!(
                "No CI runs for {branch} in {repo}. Nothing has been pushed to that branch, or that branch does not exist there; pass `repo` to ask about another repository."
            ),
            None => format!(
                "No CI runs for {branch}. This directory's git remote names no repository to ask, so nothing was queried. Pass `repo` as `owner/name`."
            ),
        },
    }
}

// Tool implementation

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
        "Read GitHub CI state for the branch you are working on: fold it to one state, list runs, watch until they settle, read a failing run's logs, or report a pull request's checks. Every query answers immediately with a handle and delivers its result later as a notification, so none of these ever blocks the turn. Read-only, and it works inside the sandbox, where `gh` run from a shell does not."
    }

    fn emitted_notifications(&self) -> &'static [&'static str] {
        &["MonitorEvent"]
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

        // Branch and repository come from local git, so a bad request is still
        // refused on the call itself rather than as a later notification.
        let branch = match input.branch.clone().or_else(|| current_branch(&cwd)) {
            Some(branch) => branch,
            None => {
                return Err(xai_tool_runtime::ToolError::custom(
                    "ci_no_branch",
                    "Could not determine the current branch. Pass `branch` explicitly.",
                ));
            }
        };
        let repo = query_repo(&cwd, input.repo.as_deref())
            .map_err(|reason| xai_tool_runtime::ToolError::custom("ci_bad_repo", reason))?;

        let (notification_handle, owner_session_id) = {
            let res = resources.lock().await;
            let handle = res
                .get::<NotificationHandle>()
                .map(|h| h.0.clone())
                .unwrap_or_default();
            let owner = res.get::<OwnerSessionId>().map(|o| o.0.clone());
            (handle, owner)
        };

        let task_id = ctx.call_id.as_str().to_owned();
        let description = format!("ci {} on {branch}", action_label(input.action));
        let query = CiQuery {
            action: input.action,
            limit: input.limit.unwrap_or(DEFAULT_RUN_LIMIT),
            run_id: input.run_id.clone(),
            timeout_secs: input.timeout_secs,
        };
        spawn_query(
            cwd,
            branch.clone(),
            repo,
            query,
            task_id.clone(),
            description,
            notification_handle,
            owner_session_id,
        );

        Ok(CiOutput::started(input.action, branch, task_id))
    }
}

/// The parts of a request the worker needs once the branch and repository are
/// already resolved on the call.
struct CiQuery {
    action: CiAction,
    limit: u32,
    run_id: Option<String>,
    timeout_secs: Option<u64>,
}

/// Run the query off the turn and deliver its outcome - success or failure -
/// as the notification the caller was told to expect.
fn spawn_query(
    cwd: std::path::PathBuf,
    branch: String,
    repo: Option<String>,
    query: CiQuery,
    task_id: String,
    description: String,
    notification_handle: ToolNotificationHandle,
    owner_session_id: Option<String>,
) {
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(crate::util::detached::fire_and_forget(
        "ci query",
        async move {
            let outcome = crate::util::detached::guarded("ci blocking query", async {
                tokio::task::spawn_blocking(move || {
                    run_blocking(&cwd, &branch, repo.as_deref(), &query)
                })
                .await
                .map_err(|error| {
                    xai_tool_runtime::ToolError::custom(
                        "ci_join",
                        format!("ci query panicked: {error}"),
                    )
                })?
            })
            .await;
            let raw_text = match outcome {
                Ok(Ok(output)) => render_output(&output),
                Ok(Err(error)) => format!("CI query failed: {error}"),
                Err(panic) => format!("CI query panicked: {panic}"),
            };
            send_ci_event(
                &notification_handle,
                owner_session_id,
                &task_id,
                &description,
                raw_text,
            );
        },
    ));
}

/// Deliver one background query's outcome as a `<monitor-event>` the bridge
/// wakes the session with.
fn send_ci_event(
    handle: &ToolNotificationHandle,
    owner_session_id: Option<String>,
    task_id: &str,
    description: &str,
    raw_text: String,
) {
    let event_text = crate::implementations::grok_build::monitor::event::wrap_monitor_event(
        description,
        &raw_text,
        task_id,
    );
    handle.send_monitor_event(MonitorEvent {
        task_id: task_id.to_string(),
        description: description.to_string(),
        event_text,
        raw_text,
        owner_session_id,
    });
}

/// Render a finished [`CiOutput`] as the body of its notification.
fn render_output(output: &CiOutput) -> String {
    let mut text = format!(
        "{}\nstate: {} (settled: {})",
        output.summary, output.state, output.settled
    );
    if !output.runs.is_empty() {
        text.push_str("\nruns:");
        for run in &output.runs {
            let id = run
                .run_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string());
            text.push_str(&format!(
                "\n  {} {} {} run={id}",
                run.workflow, run.status, run.conclusion
            ));
        }
    }
    if let Some(body) = &output.text {
        text.push('\n');
        text.push_str(body);
    }
    if output.truncated {
        text.push_str("\n(log truncated)");
    }
    text
}

fn action_label(action: CiAction) -> &'static str {
    match action {
        CiAction::Status => "status",
        CiAction::Runs => "runs",
        CiAction::Wait => "wait",
        CiAction::Logs => "logs",
        CiAction::Checks => "checks",
    }
}

/// The blocking half of one query, off the executor and free of async. A
/// blocking `gh` call per poll, which is also what makes it directly testable.
fn run_blocking(
    cwd: &std::path::Path,
    branch: &str,
    repo: Option<&str>,
    query: &CiQuery,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    match query.action {
        CiAction::Status | CiAction::Runs => status_output(cwd, branch, query.limit, repo),
        CiAction::Wait => wait_output(cwd, branch, query.limit, repo, query.timeout_secs),
        CiAction::Logs => logs_output(cwd, branch, repo, query.run_id.as_deref()),
        CiAction::Checks => Ok(checks_output(cwd, branch, repo)),
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
    repo: Option<&str>,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let runs = fetch_runs(cwd, branch, limit, repo).map_err(query_error)?;
    let state = ci_state::ci_from_runs(runs.iter().cloned().collect::<Vec<_>>());
    Ok(CiOutput {
        state: state.as_str().to_string(),
        branch: branch.to_string(),
        task_id: None,
        settled: state.is_terminal(),
        runs: summarize(&runs),
        text: None,
        truncated: false,
        summary: state_summary(state, branch, repo),
    })
}

/// Poll until the branch's runs settle or the budget runs out. A timeout is
/// not a failure: it answers with the state it last saw. The caller learns
/// the branch is still moving rather than that the tool broke.
fn wait_output(
    cwd: &std::path::Path,
    branch: &str,
    limit: u32,
    repo: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let budget = std::time::Duration::from_secs(
        timeout_secs.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS),
    );
    let deadline = std::time::Instant::now() + budget;
    loop {
        let output = status_output(cwd, branch, limit, repo)?;
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
    repo: Option<&str>,
    run_id: Option<&str>,
) -> Result<CiOutput, xai_tool_runtime::ToolError> {
    let runs = fetch_runs(cwd, branch, DEFAULT_RUN_LIMIT, repo).map_err(query_error)?;
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
    let mut args = vec!["run", "view", run_id.as_str(), "--log-failed"];
    push_repo(&mut args, repo);
    let response = xai_grok_sandbox::ci_host::run_gh(cwd, &args)
        .ok_or_else(|| {
            xai_tool_runtime::ToolError::custom(
                "ci_gh_unavailable",
                "Could not reach `gh`. In a sandboxed session the host worker answers these queries; outside one, `gh` must be installed and authenticated.",
            )
        })?;
    // A run whose failure is a startup failure has no job log at all, and
    // `gh` says so on stderr.
    let body = if response.stdout.trim().is_empty() {
        response.stderr.clone()
    } else {
        response.stdout.clone()
    };
    let (text, cut) = tail(&body, LOG_TAIL_BYTES);
    Ok(CiOutput {
        state: state.as_str().to_string(),
        branch: branch.to_string(),
        task_id: None,
        settled: state.is_terminal(),
        runs: summarize(&runs),
        text: Some(text),
        truncated: cut || response.truncated,
        summary: format!("Failing-step logs for run {run_id} on {branch}."),
    })
}

fn checks_output(cwd: &std::path::Path, branch: &str, repo: Option<&str>) -> CiOutput {
    // `gh pr checks` exits non-zero when a check is failing.
    let mut args = vec!["pr", "checks", branch];
    push_repo(&mut args, repo);
    let response = xai_grok_sandbox::ci_host::run_gh(cwd, &args).unwrap_or_else(|| {
        xai_grok_sandbox::ci_host::GhHostResponse {
            code: -1,
            stdout: String::new(),
            stderr: "could not reach `gh`".to_string(),
            truncated: false,
        }
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
        task_id: None,
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
        let args = run_list_args("feat/x", "10", None);
        assert!(args.contains(&ci_state::RUN_JSON_FIELDS));
        assert!(args.contains(&"feat/x"));
        // The worker refuses anything outside its allowlist.
        let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&owned));
    }

    /// A session whose `GH_HOST` names another host cannot let `gh` discover
    /// the repository, so every query has to carry the name itself.
    #[test]
    fn every_query_names_the_repository_when_one_is_known() {
        let list: Vec<String> = run_list_args("feat/x", "10", Some("o/r"))
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert!(list.windows(2).any(|w| w == ["--repo", "o/r"]), "{list:?}");
        let owned = xai_grok_sandbox::ci_host::gh_args_allowed(&list);
        assert!(owned, "the worker must accept a named repository: {list:?}");

        let logs: Vec<String> = {
            let mut args = vec!["run", "view", "12345", "--log-failed"];
            push_repo(&mut args, Some("o/r"));
            args.iter().map(|arg| arg.to_string()).collect()
        };
        assert!(logs.windows(2).any(|w| w == ["--repo", "o/r"]));
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&logs));

        let checks: Vec<String> = {
            let mut args = vec!["pr", "checks", "feat/x"];
            push_repo(&mut args, Some("o/r"));
            args.iter().map(|arg| arg.to_string()).collect()
        };
        assert!(checks.windows(2).any(|w| w == ["--repo", "o/r"]));
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&checks));

        // With no repository to name, the query is the one `gh` answered
        // before this existed: discovery from the cwd it is run in.
        let unnamed: Vec<String> = run_list_args("feat/x", "10", None)
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert!(!unnamed.iter().any(|arg| arg == "--repo"));
    }

    /// The name comes from the remote, in every form git accepts one.
    #[test]
    fn the_repository_name_comes_out_of_the_git_remote() {
        for url in [
            "git@github.com:wow-look-at-my/go-toolchain.git",
            "https://github.com/wow-look-at-my/go-toolchain.git",
            "https://github.com/wow-look-at-my/go-toolchain",
            "http://github.com/wow-look-at-my/go-toolchain/",
            "ssh://git@github.com/wow-look-at-my/go-toolchain.git",
            "ssh://git@github.com:22/wow-look-at-my/go-toolchain.git",
        ] {
            assert_eq!(
                repo_from_remote_url(url).as_deref(),
                Some("wow-look-at-my/go-toolchain"),
                "{url}"
            );
        }
        // Nothing to name: no path at all, or a path with no repository in it.
        for url in ["", "git@github.com:", "https://github.com/"] {
            assert_eq!(repo_from_remote_url(url), None, "{url}");
        }
    }

    /// A `repo` the model passes is either a repository name or a rejected
    /// request. A half-name must never turn into a query of some other repo.
    #[test]
    fn a_repository_token_is_checked_before_it_reaches_gh() {
        for good in ["o/r", "Wow-Look.at/my_repo", "a/b"] {
            assert!(valid_repo_token(good), "{good}");
        }
        for bad in [
            "",
            "r",
            "o/",
            "/r",
            "o/r/s",
            "o r/x",
            "-o/r",
            "o/r --hostname evil",
            "ohy\u{e9}/r",
        ] {
            assert!(!valid_repo_token(bad), "{bad} must be refused");
        }
        // A refused name is an error naming the problem, not a silent fallback.
        let refused = query_repo(std::path::Path::new("/nonexistent"), Some("not a repo"));
        match refused {
            Err(reason) => assert!(reason.contains("owner/name"), "{reason}"),
            Ok(repo) => panic!("an invalid repo must not resolve to {repo:?}"),
        }
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
        // that this replaced, so there is nothing to read yet.
        let runs = vec![
            run("CI", "in_progress", "", 5),
            run("CI", "completed", "failure", 4),
        ];
        assert_eq!(newest_failing_run(&runs), None);
    }

    #[test]
    fn every_state_tells_the_caller_what_to_do_next() {
        assert!(state_summary(CiStatus::Red, "feat/x", None).contains("logs"));
        assert!(state_summary(CiStatus::Yellow, "feat/x", None).contains("wait"));
        assert!(state_summary(CiStatus::Green, "feat/x", None).contains("passing"));
        // "No runs" must never read as "passing": nothing has been pushed.
        let none = state_summary(CiStatus::Off, "feat/x", Some("o/r"));
        assert!(none.contains("No CI runs"), "{none}");
        assert!(!none.contains("passing"), "{none}");
    }

    /// An empty answer has to say which repository it came out of. A branch
    /// that lives elsewhere is empty here for exactly the same reason an
    /// unpushed branch is. The reader cannot tell both apart without being
    /// told where was asked.
    #[test]
    fn an_empty_answer_names_the_repository_it_asked() {
        let asked = state_summary(CiStatus::Off, "feat/x", Some("o/r"));
        assert!(asked.contains("o/r"), "{asked}");
        assert!(
            asked.contains("does not exist"),
            "a branch from another repository must not read as an unpushed one: {asked}"
        );
        let unasked = state_summary(CiStatus::Off, "feat/x", None);
        assert!(unasked.contains("no repository"), "{unasked}");
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

    /// The call is answered before the `gh` it starts has finished, and the
    /// result arrives later as a notification. The stub `gh` blocks until the
    /// test releases it. The assertion is against that live signal - the
    /// subprocess is still running when `run` returns - rather than a
    /// wall-clock constant.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_answers_before_the_gh_subprocess_finishes() {
        use crate::notification::types::ToolNotification;
        use crate::types::resources::{Cwd, NotificationHandle, Resources};

        let tmp = tempfile::tempdir().unwrap();
        let started = tmp.path().join("gh-started");
        let finished = tmp.path().join("gh-finished");
        let release = tmp.path().join("gh-release");
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();

        // A `gh` that announces itself, blocks until released, then reports one
        // passing run. It is the real subprocess `run_gh` spawns.
        let script = format!(
            "#!/bin/sh\n: > '{started}'\nwhile [ ! -e '{release}' ]; do sleep 0.02; done\n: > '{finished}'\necho '[{{\"status\":\"completed\",\"conclusion\":\"success\",\"headBranch\":\"feat/x\",\"workflowName\":\"CI\",\"databaseId\":7}}]'\n",
            started = started.display(),
            release = release.display(),
            finished = finished.display(),
        );
        let gh = bin.join("gh");
        std::fs::write(&gh, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&gh).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&gh, perms).unwrap();

        let previous_path = std::env::var_os("PATH").unwrap_or_default();
        let mut with_stub = bin.into_os_string();
        with_stub.push(":");
        with_stub.push(&previous_path);
        // SAFETY: this test is the only one in the binary that reads or writes PATH.
        unsafe { std::env::set_var("PATH", &with_stub) };
        struct PathGuard(std::ffi::OsString);
        impl Drop for PathGuard {
            fn drop(&mut self) {
                // SAFETY: as above; this runs before the test's process is done.
                unsafe { std::env::set_var("PATH", &self.0) };
            }
        }
        let _guard = PathGuard(previous_path);

        let (handle, mut notifications) = ToolNotificationHandle::channel();
        let mut resources = Resources::new();
        resources.insert(Cwd(tmp.path().to_path_buf()));
        resources.insert(NotificationHandle(handle));

        // A guard, not the assertion: it turns "blocked on the subprocess"
        // into a test failure instead of a test that hangs. The subprocess is
        // released only after this call has already returned.
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            xai_tool_runtime::Tool::run(
                &CiTool,
                crate::types::tool_metadata::test_ctx_with_call_id(
                    resources.into_shared(),
                    "ci-call",
                ),
                CiInput {
                    action: CiAction::Status,
                    branch: Some("feat/x".into()),
                    repo: Some("o/r".into()),
                    run_id: None,
                    limit: None,
                    timeout_secs: None,
                },
            ),
        )
        .await
        .expect("the call must answer without waiting on its gh subprocess")
        .expect("the call must be accepted");

        // The handle comes back before the query can finish...
        assert_eq!(output.task_id.as_deref(), Some("ci-call"));

        // ...the stub process is up...
        let mut spawned = false;
        for _ in 0..500 {
            if started.exists() {
                spawned = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(spawned, "the tool must have started its gh subprocess");

        // ...and it has not finished: the tool returned without waiting on it.
        assert!(
            !finished.exists(),
            "the call returned only after its gh subprocess completed"
        );

        // Release the stub; the outcome still arrives, as the notification.
        std::fs::write(&release, b"go").unwrap();
        let delivered =
            tokio::time::timeout(std::time::Duration::from_secs(10), notifications.recv())
                .await
                .expect("the query result must arrive as a notification")
                .expect("the notification channel must stay open");
        let ToolNotification::MonitorEvent(event) = delivered else {
            panic!("the ci result must arrive as a monitor event, got {delivered:?}");
        };
        assert_eq!(event.task_id, "ci-call");
        assert!(
            event.raw_text.contains("passing") && event.raw_text.contains("feat/x"),
            "the notification must carry the query's result: {}",
            event.raw_text
        );
        assert!(
            finished.exists(),
            "the released gh subprocess must have completed"
        );
    }
}
