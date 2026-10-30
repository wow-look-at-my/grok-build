//! `git` — read-only git for the explore agent, which has no shell.
mod policy;

use crate::types::output::{TextOutput, ToolOutput};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::ToolMetadata;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub const GIT_TOOL_NAME: &str = "git";

/// Options before the subcommand. The caller cannot add global options.
const GLOBAL_ARGS: &[&str] = &[
    "--no-pager",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "color.ui=never",
];

/// `GIT_NO_LAZY_FETCH` stops a partial clone from fetching a missing object.
const ENV: &[(&str, &str)] = &[
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_PAGER", "cat"),
];

const MAX_STDOUT_BYTES: usize = crate::DEFAULT_TOOL_OUTPUT_BYTES;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

const DESCRIPTION: &str = r#"Run a read-only git command. Pass the arguments after `git` as an array, for example ["log", "--oneline", "-n", "20"]. No shell runs them, so pipes and redirects do not work.

These subcommands run: log, show, diff, diff-tree, diff-index, diff-files, whatchanged, range-diff, blame, annotate, status, grep, ls-files, ls-tree, cat-file, rev-parse, rev-list, describe, name-rev, merge-base, cherry, for-each-ref, show-ref, show-branch, shortlog, count-objects, check-ignore, check-attr, check-mailmap, check-ref-format, version.

These run only in their listing form: branch (patterns need --list), tag (patterns need -l), config (get, list, --get, --get-all, --get-regexp, --list), remote (bare, -v, get-url), stash (list, show), worktree list, notes (list, show), reflog (show, list, exists), symbolic-ref <name>, submodule status.

- Every subcommand and option that writes is refused, and so are --output, --ext-diff and grep -O.
- In branch, tag and config, an option with a value must be written as --opt=value.
- The command runs in the workspace with every file write denied by the kernel.
- Pass a revision to shortlog (for example HEAD), because it reads standard input without one."#;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GitInput {
    #[schemars(
        description = "The arguments after `git`, one per element, for example [\"log\", \"--oneline\", \"-n\", \"20\"]. The first element is the subcommand."
    )]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct GitOutput {
    pub text: String,
}

impl xai_tool_runtime::ToolOutput for GitOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.text.clone(),
        }]
    }
}

impl From<GitOutput> for ToolOutput {
    fn from(o: GitOutput) -> Self {
        ToolOutput::Text(TextOutput::from(o.text))
    }
}

#[derive(Debug, Default)]
pub struct GitTool;

impl ToolMetadata for GitTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Git
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        DESCRIPTION
    }
}

fn tool_id() -> xai_tool_protocol::ToolId {
    xai_tool_protocol::ToolId::new(GIT_TOOL_NAME).expect("valid tool id")
}

impl xai_tool_runtime::Tool for GitTool {
    type Args = GitInput;
    type Output = GitOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        tool_id()
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            GIT_TOOL_NAME,
            ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.git", skip_all, fields(args = ?input.args))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: GitInput,
    ) -> Result<GitOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::{resolve_cwd, shared_resources};
        let approved =
            policy::approve(&input.args).map_err(xai_tool_runtime::ToolError::invalid_arguments)?;
        let resources = shared_resources(&ctx)?;
        let cwd = resolve_cwd(&ctx, &resources).await?;
        run_git(&cwd, &approved.args).await
    }
}

/// Run `git` with `args` in `cwd`, write-confined, and render the result for the model.
async fn run_git(
    cwd: &std::path::Path,
    args: &[String],
) -> Result<GitOutput, xai_tool_runtime::ToolError> {
    let mut cmd = xai_grok_sandbox::read_only_child::read_only_command(std::ffi::OsStr::new("git"))
        .map_err(|e| xai_tool_runtime::ToolError::execution(tool_id(), e.to_string()))?;
    cmd.args(GLOBAL_ARGS)
        .args(args)
        .current_dir(cwd)
        .envs(ENV.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::util::detach_search_command(&mut cmd);
    #[allow(clippy::disallowed_methods)]
    // killed on drop; the timeout and the output cap both drop it
    let mut child = cmd.spawn().map_err(|e| {
        xai_tool_runtime::ToolError::execution(tool_id(), format!("failed to start git: {e}"))
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read = async {
        let (out, err) = tokio::join!(
            read_capped(stdout, MAX_STDOUT_BYTES),
            read_capped(stderr, MAX_STDERR_BYTES)
        );
        let status = if out.1 || err.1 {
            // Git still has output to write. Stop it rather than wait for the pipe.
            let _ = child.start_kill();
            None
        } else {
            child.wait().await.ok()
        };
        (out, err, status)
    };
    let Ok(((stdout, stdout_cut), (stderr, stderr_cut), status)) =
        tokio::time::timeout(TIMEOUT, read).await
    else {
        return Err(xai_tool_runtime::ToolError::execution(
            tool_id(),
            format!(
                "git did not finish in {} seconds and was stopped",
                TIMEOUT.as_secs()
            ),
        ));
    };
    Ok(GitOutput {
        text: render(&stdout, stdout_cut, &stderr, stderr_cut, status),
    })
}

/// Read at most `cap` bytes. The flag is true when the stream had more.
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    cap: usize,
) -> (Vec<u8>, bool) {
    let Some(pipe) = pipe else {
        return (Vec::new(), false);
    };
    let mut buf = Vec::new();
    let mut limited = pipe.take(cap as u64 + 1);
    if let Err(e) = limited.read_to_end(&mut buf).await {
        tracing::warn!(error = %e, "reading git output failed");
    }
    let cut = buf.len() > cap;
    buf.truncate(cap);
    (buf, cut)
}

fn render(
    stdout: &[u8],
    stdout_cut: bool,
    stderr: &[u8],
    stderr_cut: bool,
    status: Option<std::process::ExitStatus>,
) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut text = String::new();
    match status {
        Some(s) if s.success() => {}
        Some(s) => text.push_str(&format!("git exited with {s}\n")),
        None => {}
    }
    if !stderr.trim().is_empty() && !status.is_some_and(|s| s.success()) {
        text.push_str(stderr.trim_end());
        if stderr_cut {
            text.push_str("\n[stderr cut]");
        }
        text.push('\n');
    }
    text.push_str(&stdout);
    if stdout_cut {
        text.push_str(&format!(
			"\n[output cut at {MAX_STDOUT_BYTES} bytes and git was stopped. Narrow the command, for example with -n <count> or a path]"
		));
    }
    if text.trim().is_empty() {
        text = "(no output)".to_owned();
    }
    text
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;
