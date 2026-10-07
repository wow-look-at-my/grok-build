//! The background `wait`: the tool returns at once, and the session wakes when CI settles.

use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::Duration;

use super::{
    CiOutput, CiQuery, DEFAULT_WAIT_SECS, MAX_WAIT_SECS, WAIT_EXIT_FAILING, WAIT_EXIT_NO_RUNS,
    WAIT_EXIT_PASSING, WAIT_EXIT_QUERY_FAILED, WAIT_EXIT_STILL_RUNNING, WAIT_POLL_SECS,
    WAIT_TASK_MARGIN_SECS, off_executor,
};
use crate::computer::types::TerminalBackend;
use crate::types::tool::ToolKind;
use xai_tool_runtime::ToolError;

/// How a background wait ended: the text the session wakes with, and the exit code of its task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WaitVerdict {
    pub(super) code: i32,
    pub(super) text: String,
}

impl WaitVerdict {
    fn from_status(output: &CiOutput, budget: Duration) -> Self {
        let code = match output.state.as_str() {
            "passing" => WAIT_EXIT_PASSING,
            "failing" => WAIT_EXIT_FAILING,
            "in_progress" => WAIT_EXIT_STILL_RUNNING,
            _ => WAIT_EXIT_NO_RUNS,
        };
        let summary = if code == WAIT_EXIT_STILL_RUNNING {
            format!(
                "Watched for {}s and CI is still running on {}. Call `wait` again to keep watching.",
                budget.as_secs(),
                output.branch
            )
        } else {
            output.summary.clone()
        };
        let mut text = format!("CI {}: {summary}\n", output.state);
        for run in &output.runs {
            let id = run
                .run_id
                .map(|id| format!(" (run {id})"))
                .unwrap_or_default();
            let result = if run.conclusion.is_empty() {
                run.status.as_str()
            } else {
                run.conclusion.as_str()
            };
            text.push_str(&format!("- {}: {result}{id}\n", run.workflow));
        }
        Self { code, text }
    }

    fn query_failed(error: &str) -> Self {
        Self {
            code: WAIT_EXIT_QUERY_FAILED,
            text: format!("CI wait could not read CI: {error}\n"),
        }
    }
}

/// Poll until the runs settle or the budget runs out.
///
/// `None` means `stopped` said yes: nobody is left to read a verdict. A failed
/// query is tried again on the same cadence.
/// the last answer before the budget ends.
pub(super) async fn watch_until_settled<P, PF, S, SF>(
    mut poll: P,
    mut stopped: S,
    budget: Duration,
    interval: Duration,
) -> Option<WaitVerdict>
where
    P: FnMut() -> PF,
    PF: std::future::Future<Output = Result<CiOutput, ToolError>>,
    S: FnMut() -> SF,
    SF: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if stopped().await {
            return None;
        }
        let verdict = match poll().await {
            Ok(output) if output.settled => {
                return Some(WaitVerdict::from_status(&output, budget));
            }
            Ok(output) => WaitVerdict::from_status(&output, budget),
            Err(error) => {
                tracing::warn!(error = %error, "ci wait: query failed; trying again");
                WaitVerdict::query_failed(&error.to_string())
            }
        };
        if tokio::time::Instant::now() >= deadline {
            return Some(verdict);
        }
        tokio::time::sleep(interval).await;
    }
}

/// The files the poller gives its verdict to the task in.
#[derive(Debug, Clone)]
pub(super) struct VerdictFiles {
    text: PathBuf,
    done: PathBuf,
}

impl VerdictFiles {
    pub(super) fn new(dir: &Path, call_id: &str) -> Self {
        Self {
            text: dir.join(format!("ci-wait-{call_id}.txt")),
            done: dir.join(format!("ci-wait-{call_id}.done")),
        }
    }

    /// The task's script: wait for `done`, print the verdict, exit with its code.
    pub(super) fn shell_command(&self) -> Result<String, String> {
        let text = shell_quote(&self.text)?;
        let done = shell_quote(&self.done)?;
        Ok(format!(
            "until [ -f {done} ]; do sleep 2; done; cat {text}; code=$(cat {done}); rm -f {text} {done}; exit \"$code\""
        ))
    }

    /// Write the text, then rename `done` into place. A shell that sees
    /// `done` therefore reads a whole verdict.
    pub(super) fn publish(&self, verdict: &WaitVerdict) -> std::io::Result<()> {
        std::fs::write(&self.text, &verdict.text)?;
        let staged = self.done.with_extension("done.tmp");
        std::fs::write(&staged, verdict.code.to_string())?;
        std::fs::rename(&staged, &self.done)
    }
}

/// A path as one single-quoted shell word.
fn shell_quote(path: &Path) -> Result<String, String> {
    let Some(text) = path.to_str() else {
        return Err(format!(
            "the session folder path is not UTF-8: {}",
            path.display()
        ));
    };
    Ok(format!("'{}'", text.replace('\'', r"'\''")))
}

/// Start the task and its poller, and answer with the task id.
pub(super) async fn start_background_wait(
    ctx: &xai_tool_runtime::ToolCallContext,
    resources: &crate::types::resources::SharedResources,
    query: CiQuery,
    timeout_secs: Option<u64>,
    first: CiOutput,
) -> Result<CiOutput, ToolError> {
    use crate::types::resources::{NotificationHandle, OwnerSessionId, SessionFolder, Terminal};

    let budget = Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_WAIT_SECS)
            .clamp(1, MAX_WAIT_SECS),
    );
    let (terminal, notification_handle, session_folder, owner_session_id) = {
        let res = resources.lock().await;
        (
            res.require::<Terminal>()?.0.clone(),
            res.get::<NotificationHandle>()
                .map(|h| h.0.clone())
                .unwrap_or_default(),
            res.require::<SessionFolder>()?.0.clone(),
            res.get::<OwnerSessionId>().map(|o| o.0.clone()),
        )
    };

    let call_id = ctx.call_id.as_str().to_owned();
    let terminal_dir = session_folder.join("terminal");
    std::fs::create_dir_all(&terminal_dir).map_err(|error| {
        ToolError::custom(
            "ci_wait_setup",
            format!("could not create {}: {error}", terminal_dir.display()),
        )
    })?;
    let files = VerdictFiles::new(&terminal_dir, &call_id);
    let command = files
        .shell_command()
        .map_err(|reason| ToolError::custom("ci_wait_setup", reason))?;

    let target = match query.repo() {
        Some(repo) => format!("{} in {repo}", query.branch),
        None => query.branch.clone(),
    };
    let description = format!("CI wait: {target}");
    let display = format!("ci wait {target}");
    let handle = terminal
        .run_background(crate::computer::types::TerminalRunRequest {
            command,
            working_directory: query.cwd.clone(),
            env: std::collections::HashMap::new(),
            timeout: budget + Duration::from_secs(WAIT_TASK_MARGIN_SECS),
            output_byte_limit: 1024 * 1024,
            output_file: terminal_dir.join(format!("{call_id}.log")),
            notification_handle: notification_handle.clone(),
            tool_call_id: call_id.clone(),
            display_command: Some(display.clone()),
            auto_background_on_timeout: false,
            foreground_block_budget: None,
            kind: crate::computer::types::TaskKind::Bash,
            owner_session_id,
            description: Some(description.clone()),
        })
        .await
        .map_err(|error| ToolError::custom("process_manager", error.to_string()))?;
    let task_id = handle.task_id.clone();

    notification_handle.send_backgrounded(crate::notification::BashExecutionBackgrounded {
        base: crate::notification::BashNotificationBase {
            tool_call_id: call_id,
            command: display,
            output: Vec::new(),
            total_bytes: 0,
            truncated: false,
            cwd: query.cwd.clone(),
        },
        output_file: handle.output_file,
        task_id: task_id.clone(),
        monitor_description: None,
        description: Some(description),
    });

    // Weak: the poller must not keep the session's terminal alive after the session ends.
    let poller_terminal = std::sync::Arc::downgrade(&terminal);
    drop(terminal);
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(crate::util::detached::fire_and_forget(
        "ci wait poller",
        run_wait_poller(
            task_id.clone(),
            poller_terminal,
            query,
            files,
            budget,
            Duration::from_secs(WAIT_POLL_SECS),
        ),
    ));

    let kill_tool = crate::types::template_renderer::TemplateRenderer::resolve_tool_name(
        resources,
        ToolKind::KillTaskAction,
    )
    .await
    .unwrap_or_else(|| "kill_command_or_subagent".to_string());
    Ok(CiOutput {
        task_id: Some(task_id.clone()),
        summary: format!(
            "CI is still running on {}. Watching it in the background (task {task_id}, up to {}s). \
             You will be woken with the state when it settles. Keep working: do not poll, sleep, or call `status` in a loop. \
             {kill_tool} stops the watch.",
            first.branch,
            budget.as_secs()
        ),
        ..first
    })
}

/// Watch CI, then give the verdict to the task.
async fn run_wait_poller(
    task_id: String,
    terminal: Weak<dyn TerminalBackend>,
    query: CiQuery,
    files: VerdictFiles,
    budget: Duration,
    interval: Duration,
) {
    let poll = || {
        let query = query.clone();
        off_executor(move || query.status())
    };
    let stopped = || {
        let terminal = terminal.clone();
        let task_id = task_id.clone();
        async move { task_ended(&terminal, &task_id).await }
    };
    run_poll_and_publish(&task_id, &terminal, &files, poll, stopped, budget, interval).await;
}

/// The poller's body, with the poll and the stop check given as arguments.
///
/// A poll loop that panics still writes a verdict. A verdict that cannot be
/// written kills the task. Either way the task ends and the session wakes.
async fn run_poll_and_publish<P, PF, S, SF>(
    task_id: &str,
    terminal: &Weak<dyn TerminalBackend>,
    files: &VerdictFiles,
    poll: P,
    stopped: S,
    budget: Duration,
    interval: Duration,
) where
    P: FnMut() -> PF,
    PF: std::future::Future<Output = Result<CiOutput, ToolError>>,
    S: FnMut() -> SF,
    SF: std::future::Future<Output = bool>,
{
    let round = crate::util::detached::guarded(
        "ci wait poll loop",
        watch_until_settled(poll, stopped, budget, interval),
    )
    .await;
    let verdict = match round {
        Ok(Some(verdict)) => verdict,
        Ok(None) => return,
        Err(panic) => {
            tracing::error!(task_id, panic = %panic, "ci wait: poll loop panicked");
            WaitVerdict::query_failed(&format!("the poll loop stopped: {panic}"))
        }
    };
    let Err(error) = files.publish(&verdict) else {
        return;
    };
    tracing::error!(task_id, error = %error, "ci wait: could not write the verdict; killing the task");
    if let Some(terminal) = terminal.upgrade() {
        terminal.kill_task(task_id).await;
    }
}

/// Whether the task is gone: killed, finished, or its session dropped the terminal.
async fn task_ended(terminal: &Weak<dyn TerminalBackend>, task_id: &str) -> bool {
    let Some(terminal) = terminal.upgrade() else {
        return true;
    };
    terminal
        .get_task(task_id)
        .await
        .is_none_or(|snapshot| snapshot.completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::local::LocalTerminalBackend;
    use crate::computer::types::{TaskKind, TerminalRunRequest};
    use crate::implementations::grok_build::ci::CiRunSummary;
    use crate::notification::types::ToolNotificationHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn status(state: &str, settled: bool) -> CiOutput {
        CiOutput {
            state: state.to_string(),
            branch: "feat/x".to_string(),
            settled,
            runs: vec![CiRunSummary {
                workflow: "CI".to_string(),
                status: if settled { "completed" } else { "in_progress" }.to_string(),
                conclusion: match state {
                    "passing" => "success",
                    "failing" => "failure",
                    _ => "",
                }
                .to_string(),
                run_id: Some(42),
            }],
            text: None,
            truncated: false,
            task_id: None,
            summary: format!("summary for {state}"),
        }
    }

    fn never_stopped() -> impl FnMut() -> std::future::Ready<bool> {
        || std::future::ready(false)
    }

    /// The verdict is the first settled answer, after the in-flight ones.
    #[tokio::test]
    async fn the_watch_ends_on_the_first_settled_answer() {
        let answers = ["in_progress", "in_progress", "failing", "passing"];
        let calls = AtomicUsize::new(0);
        let verdict = watch_until_settled(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                let state = answers[n];
                std::future::ready(Ok(status(state, state != "in_progress")))
            },
            never_stopped(),
            Duration::from_secs(60),
            Duration::from_millis(1),
        )
        .await
        .expect("a settled branch gives a verdict");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(verdict.code, WAIT_EXIT_FAILING);
        assert!(
            verdict.text.starts_with("CI failing: summary for failing"),
            "{}",
            verdict.text
        );
        assert!(
            verdict.text.contains("- CI: failure (run 42)"),
            "{}",
            verdict.text
        );
    }

    /// A budget that runs out reports the branch as still running, with its own exit code.
    #[tokio::test]
    async fn a_spent_budget_reports_the_state_it_last_saw() {
        let verdict = watch_until_settled(
            || std::future::ready(Ok(status("in_progress", false))),
            never_stopped(),
            Duration::from_millis(20),
            Duration::from_millis(5),
        )
        .await
        .expect("a spent budget is still a verdict");
        assert_eq!(verdict.code, WAIT_EXIT_STILL_RUNNING);
        assert!(
            verdict.text.contains("still running on feat/x"),
            "{}",
            verdict.text
        );
        assert!(
            verdict.text.contains("Call `wait` again"),
            "{}",
            verdict.text
        );
    }

    /// One failed query does not end the watch.
    #[tokio::test]
    async fn a_failed_query_is_retried_and_reported_only_if_it_is_the_last_answer() {
        let calls = AtomicUsize::new(0);
        let verdict = watch_until_settled(
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(if n == 0 {
                    Err(ToolError::custom("ci_gh_failed", "HTTP 502"))
                } else {
                    Ok(status("passing", true))
                })
            },
            never_stopped(),
            Duration::from_secs(60),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(verdict.code, WAIT_EXIT_PASSING);

        let verdict = watch_until_settled(
            || {
                std::future::ready(Err(ToolError::custom(
                    "ci_gh_failed",
                    "HTTP 401: bad token",
                )))
            },
            never_stopped(),
            Duration::from_millis(20),
            Duration::from_millis(5),
        )
        .await
        .unwrap();
        assert_eq!(verdict.code, WAIT_EXIT_QUERY_FAILED);
        assert!(verdict.text.contains("bad token"), "{}", verdict.text);
    }

    /// A killed task stops the watch without a verdict and without another query.
    #[tokio::test]
    async fn a_stopped_task_ends_the_watch_without_a_verdict() {
        let calls = AtomicUsize::new(0);
        let verdict = watch_until_settled(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(status("in_progress", false)))
            },
            || std::future::ready(true),
            Duration::from_secs(60),
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(verdict, None);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_path_with_a_quote_stays_one_shell_word() {
        assert_eq!(
            shell_quote(Path::new("/tmp/it's here")).unwrap(),
            r"'/tmp/it'\''s here'"
        );
    }

    fn task_request(command: String, dir: &Path) -> TerminalRunRequest {
        TerminalRunRequest {
            command,
            working_directory: dir.to_path_buf(),
            env: std::collections::HashMap::new(),
            timeout: Duration::from_secs(60),
            output_byte_limit: 1024 * 1024,
            output_file: dir.join("task.log"),
            notification_handle: ToolNotificationHandle::noop(),
            tool_call_id: "tc-ci-wait".to_string(),
            display_command: Some("ci wait feat/x".to_string()),
            auto_background_on_timeout: false,
            foreground_block_budget: None,
            kind: TaskKind::Bash,
            owner_session_id: Some("session-A".to_string()),
            description: Some("CI wait: feat/x".to_string()),
        }
    }

    /// The real task script and the real poller body, on a real terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_task_prints_the_verdict_and_exits_with_its_code() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("it's a session");
        std::fs::create_dir_all(&dir).unwrap();
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalTerminalBackend::new());
        let files = VerdictFiles::new(&dir, "tc-ci-wait");
        let handle = backend
            .run_background(task_request(files.shell_command().unwrap(), &dir))
            .await
            .expect("spawn the wait task");
        let task_id = handle.task_id.clone();
        let weak = Arc::downgrade(&backend);

        let calls = AtomicUsize::new(0);
        let poller_backend = backend.clone();
        run_poll_and_publish(
            &task_id,
            &weak,
            &files,
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(if n < 2 {
                    status("in_progress", false)
                } else {
                    status("failing", true)
                }))
            },
            || {
                let backend = poller_backend.clone();
                let task_id = task_id.clone();
                async move {
                    let snapshot = backend.get_task(&task_id).await;
                    assert!(
                        snapshot.as_ref().is_some_and(|s| !s.completed),
                        "the task must stay running until the verdict is written"
                    );
                    false
                }
            },
            Duration::from_secs(60),
            Duration::from_millis(50),
        )
        .await;

        let snapshot = backend
            .wait_for_completion(&task_id, Some(Duration::from_secs(20)))
            .await
            .expect("the task must end once the verdict is written");
        assert!(snapshot.completed);
        assert_eq!(snapshot.exit_code, Some(WAIT_EXIT_FAILING));
        assert!(
            snapshot.output.contains("CI failing: summary for failing"),
            "the verdict is the task's output: {:?}",
            snapshot.output
        );
        assert!(
            !files.text.exists() && !files.done.exists(),
            "the task removes its hand-off files"
        );
    }

    /// A killed task stops its poller: no verdict file is written for nobody.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_killed_task_stops_its_poller() {
        let tmp = tempfile::tempdir().unwrap();
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalTerminalBackend::new());
        let files = VerdictFiles::new(tmp.path(), "tc-ci-wait");
        let handle = backend
            .run_background(task_request(files.shell_command().unwrap(), tmp.path()))
            .await
            .unwrap();
        let task_id = handle.task_id.clone();
        let weak = Arc::downgrade(&backend);
        backend.kill_task(&task_id).await;

        let finished = tokio::time::timeout(
            Duration::from_secs(10),
            run_poll_and_publish(
                &task_id,
                &weak,
                &files,
                || std::future::ready(Ok(status("in_progress", false))),
                || {
                    let weak = weak.clone();
                    let task_id = task_id.clone();
                    async move { task_ended(&weak, &task_id).await }
                },
                Duration::from_secs(60),
                Duration::from_millis(10),
            ),
        )
        .await;
        assert!(
            finished.is_ok(),
            "the poller must stop once its task is gone"
        );
        assert!(!files.done.exists(), "a stopped watch writes no verdict");
    }

    /// A panic in the poll loop still ends the task, with the panic as the verdict.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_poll_still_ends_the_task() {
        let tmp = tempfile::tempdir().unwrap();
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalTerminalBackend::new());
        let files = VerdictFiles::new(tmp.path(), "tc-ci-wait");
        let handle = backend
            .run_background(task_request(files.shell_command().unwrap(), tmp.path()))
            .await
            .unwrap();
        let task_id = handle.task_id.clone();
        let weak = Arc::downgrade(&backend);

        run_poll_and_publish(
            &task_id,
            &weak,
            &files,
            || -> std::future::Ready<Result<CiOutput, ToolError>> {
                panic!("the gh worker went away")
            },
            never_stopped(),
            Duration::from_secs(60),
            Duration::from_millis(10),
        )
        .await;

        let snapshot = backend
            .wait_for_completion(&task_id, Some(Duration::from_secs(20)))
            .await
            .unwrap();
        assert_eq!(snapshot.exit_code, Some(WAIT_EXIT_QUERY_FAILED));
        assert!(
            snapshot.output.contains("the gh worker went away"),
            "{:?}",
            snapshot.output
        );
    }
}
