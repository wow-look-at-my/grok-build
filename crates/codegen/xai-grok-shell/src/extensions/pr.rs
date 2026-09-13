use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};

use super::{ExtResult, parse_params, to_ext_response};
use crate::agent::MvpAgent;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrStatusRequest {
    pub cwd: String,
    pub branch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrStatusResponse {
    pub pr: Option<PrData>,
    pub updated_session_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrData {
    pub url: String,
    pub state: String,
    pub is_in_merge_queue: bool,
    pub number: Option<u64>,
    pub title: Option<String>,
    /// The pull request's check runs, as `gh pr checks --json` reports them.
    pub checks: Vec<PrCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrCheck {
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub conclusion: Option<String>,
}

/// The one-line document the sandbox host worker answers a `gh-pr` request
/// with (`xai_grok_sandbox::ci_host`): `gh pr view` fields plus the checks.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostPrResponse {
    state: Option<String>,
    url: Option<String>,
    is_draft: Option<bool>,
    number: Option<u64>,
    title: Option<String>,
    #[serde(default)]
    checks: Vec<PrCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrViewResponse {
    state: Option<String>,
    url: Option<String>,
    is_draft: Option<bool>,
    number: Option<u64>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlResponse {
    data: Option<GhGraphqlData>,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlData {
    resource: Option<GhGraphqlPullRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhGraphqlPullRequest {
    is_in_merge_queue: Option<bool>,
}

pub async fn handle(_agent: &MvpAgent, args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        "x.ai/pr/status" => {
            let req = parse_params::<PrStatusRequest>(args)?;
            to_ext_response(handle_pr_status(&req.cwd, &req.branch).await)
        }
        _ => Err(acp::Error::method_not_found()),
    }
}

async fn handle_pr_status(cwd: &str, branch: &str) -> anyhow::Result<PrStatusResponse> {
    // A sandboxed session cannot spawn `gh` in the jail: the host worker is
    // the only route, and its answer (or nothing-usable sentinel) is final.
    let pr = match xai_grok_sandbox::ci_host::inherited_host_fd() {
        Some(fd) => pr_via_ci_host(fd, branch).await,
        None => gh_pr_view_by_branch(cwd, branch).await,
    };
    Ok(PrStatusResponse {
        pr,
        updated_session_ids: Vec::new(),
    })
}

/// The `state` the client shows, from `gh pr view`'s `state` and `isDraft`.
fn pr_state(state: Option<&str>, is_draft: bool) -> &'static str {
    match state.map(str::to_ascii_lowercase).as_deref() {
        Some("merged") => "merged",
        Some("closed") => "closed",
        _ if is_draft => "draft",
        _ => "open",
    }
}

/// Ask the sandbox host worker for the branch's pull request and checks
/// (`gh-pr`). The worker runs no GraphQL, so the merge-queue flag is not
/// known there and reads false.
#[cfg(unix)]
async fn pr_via_ci_host(fd: i32, branch: &str) -> Option<PrData> {
    let branch = branch.to_owned();
    let body = tokio::task::spawn_blocking(move || {
        let stream = xai_grok_sandbox::ci_host::inherited_host_stream(fd)?;
        xai_grok_sandbox::ci_host::query_ci_host_stream_pr(stream, &branch)
    })
    .await
    .ok()??;
    let parsed = serde_json::from_slice::<HostPrResponse>(&body).ok()?;
    Some(PrData {
        url: parsed.url?,
        state: pr_state(parsed.state.as_deref(), parsed.is_draft.unwrap_or(false)).to_string(),
        is_in_merge_queue: false,
        number: parsed.number,
        title: parsed.title,
        checks: parsed.checks,
    })
}

#[cfg(not(unix))]
async fn pr_via_ci_host(_fd: i32, _branch: &str) -> Option<PrData> {
    None
}

async fn gh_pr_view_by_branch(cwd: &str, branch: &str) -> Option<PrData> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args([
        "pr",
        "view",
        branch,
        "--json",
        "state,url,isDraft,number,title",
    ])
    .current_dir(cwd)
    .stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    cmd.envs(xai_grok_tools::util::pager_env());
    // gh colorizes even piped --json output under CLICOLOR_FORCE or
    // GH_FORCE_TTY (inherited from terminal-launched dev environments), and
    // forcing beats NO_COLOR in gh's precedence; there is no --no-color flag
    // (cli/cli#9436). CLICOLOR_FORCE=0 is gh's documented off-switch.
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    let output = cmd.output().await.ok()?;

    if !output.status.success() {
        return None;
    }

    let parsed =
        serde_json::from_slice::<GhPrViewResponse>(&strip_ansi_csi(&output.stdout)).ok()?;
    let url = parsed.url?;
    let state = pr_state(parsed.state.as_deref(), parsed.is_draft.unwrap_or(false));
    let is_in_merge_queue = state == "open" && gh_pr_is_in_merge_queue(cwd, &url).await;
    let checks = gh_pr_checks(cwd, branch).await;

    Some(PrData {
        url,
        state: state.to_string(),
        is_in_merge_queue,
        number: parsed.number,
        title: parsed.title,
        checks,
    })
}

/// `gh pr checks --json` for `branch`. The exit code is the verdict (1: a
/// check failed, 8: a check is pending) and the list is printed either way,
/// so only an unparseable stdout reads as no checks.
async fn gh_pr_checks(cwd: &str, branch: &str) -> Vec<PrCheck> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["pr", "checks", branch, "--json", "name,state,conclusion"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    cmd.envs(xai_grok_tools::util::pager_env());
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    let Ok(output) = cmd.output().await else {
        return Vec::new();
    };
    if !matches!(output.status.code(), Some(0 | 1 | 8)) {
        return Vec::new();
    }
    serde_json::from_slice(&strip_ansi_csi(&output.stdout)).unwrap_or_default()
}

/// `gh pr view --json` does not expose `isInMergeQueue`; query GraphQL via `gh api`.
async fn gh_pr_is_in_merge_queue(cwd: &str, pr_url: &str) -> bool {
    const QUERY: &str =
        "query($url: URI!) { resource(url: $url) { ... on PullRequest { isInMergeQueue } } }";
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args([
        "api",
        "graphql",
        "-f",
        &format!("query={QUERY}"),
        "-f",
        &format!("url={pr_url}"),
    ])
    .current_dir(cwd)
    .stdin(std::process::Stdio::null());
    xai_grok_tools::util::detach_command(&mut cmd);
    cmd.envs(xai_grok_tools::util::pager_env());
    // Forcing (CLICOLOR_FORCE/GH_FORCE_TTY) beats NO_COLOR in gh's precedence.
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    let output = match cmd.output().await {
        Ok(output) => output,
        Err(_) => return false,
    };
    if !output.status.success() {
        let stderr_snippet: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(200)
            .collect();
        tracing::warn!(
            status = %output.status,
            stderr = %stderr_snippet,
            "gh api graphql isInMergeQueue lookup failed"
        );
        return false;
    }
    parse_is_in_merge_queue(&output.stdout).unwrap_or(false)
}

fn parse_is_in_merge_queue(stdout: &[u8]) -> Option<bool> {
    let stripped = strip_ansi_csi(stdout);
    let parsed = match serde_json::from_slice::<GhGraphqlResponse>(&stripped) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse gh api graphql isInMergeQueue response");
            return None;
        }
    };
    parsed.data?.resource?.is_in_merge_queue
}

/// `gh` can colorize stdout even when piped (e.g. `GH_FORCE_TTY`, `--color always`
/// in config), which would break serde parsing of the JSON payload.
fn strip_ansi_csi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host worker's one-line `gh-pr` document maps onto the same
    /// `PrData` the direct `gh` path builds, checks included.
    #[test]
    fn host_worker_pr_document_parses_to_pr_data() {
        let body = br#"{"state":"OPEN","merged":false,"isDraft":true,"url":"https://github.com/o/r/pull/7","number":7,"title":"t","checks":[{"name":"CI","state":"PENDING","conclusion":null}]}"#;
        let parsed = serde_json::from_slice::<HostPrResponse>(body).unwrap();
        assert_eq!(
            pr_state(parsed.state.as_deref(), parsed.is_draft.unwrap_or(false)),
            "draft"
        );
        assert_eq!(parsed.url.as_deref(), Some("https://github.com/o/r/pull/7"));
        assert_eq!(parsed.number, Some(7));
        assert_eq!(
            parsed.checks,
            [PrCheck {
                name: "CI".into(),
                state: "PENDING".into(),
                conclusion: None,
            }]
        );
    }

    #[test]
    fn pr_state_prefers_merged_and_closed_over_draft() {
        assert_eq!(pr_state(Some("MERGED"), true), "merged");
        assert_eq!(pr_state(Some("CLOSED"), true), "closed");
        assert_eq!(pr_state(Some("OPEN"), true), "draft");
        assert_eq!(pr_state(Some("OPEN"), false), "open");
        assert_eq!(pr_state(None, false), "open");
    }

    #[test]
    fn gh_pr_view_json_parses_after_stripping_forced_color() {
        let stdout = b"\x1b[1;37m{\x1b[m\n  \x1b[1;34m\"isDraft\"\x1b[m\x1b[1;37m:\x1b[m \x1b[33mfalse\x1b[m\x1b[1;37m,\x1b[m\n  \x1b[1;34m\"number\"\x1b[m\x1b[1;37m:\x1b[m 242682\x1b[1;37m,\x1b[m\n  \x1b[1;34m\"state\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"OPEN\"\x1b[m\x1b[1;37m,\x1b[m\n  \x1b[1;34m\"title\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"t\"\x1b[m\x1b[1;37m,\x1b[m\n  \x1b[1;34m\"url\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"https://github.com/xai-org/xai/pull/242682\"\x1b[m\n\x1b[1;37m}\x1b[m\n";
        let parsed = serde_json::from_slice::<GhPrViewResponse>(&strip_ansi_csi(stdout)).unwrap();
        assert_eq!(parsed.number, Some(242682));
        assert_eq!(parsed.state.as_deref(), Some("OPEN"));
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://github.com/xai-org/xai/pull/242682")
        );
    }

    #[test]
    fn parse_is_in_merge_queue_true() {
        let stdout = br#"{"data":{"resource":{"isInMergeQueue":true}}}"#;
        assert_eq!(parse_is_in_merge_queue(stdout), Some(true));
    }

    #[test]
    fn parse_is_in_merge_queue_false() {
        let stdout = br#"{"data":{"resource":{"isInMergeQueue":false}}}"#;
        assert_eq!(parse_is_in_merge_queue(stdout), Some(false));
    }

    #[test]
    fn parse_is_in_merge_queue_missing_resource() {
        let stdout = br#"{"data":{"resource":null}}"#;
        assert_eq!(parse_is_in_merge_queue(stdout), None);
    }

    #[test]
    fn parse_is_in_merge_queue_missing_data() {
        assert_eq!(parse_is_in_merge_queue(b"{}"), None);
    }

    #[test]
    fn parse_is_in_merge_queue_malformed_json() {
        assert_eq!(parse_is_in_merge_queue(b"not json"), None);
    }

    #[test]
    fn parse_is_in_merge_queue_ansi_wrapped_json() {
        let stdout =
            b"\x1b[1;32m{\"data\":{\"resource\":{\"isInMergeQueue\":\x1b[0;36mtrue\x1b[0m}}}\x1b[0m";
        assert_eq!(parse_is_in_merge_queue(stdout), Some(true));
    }

    #[test]
    fn parse_is_in_merge_queue_ansi_wrapped_false() {
        let stdout = b"\x1b[38;5;208m{\"data\":{\"resource\":{\"isInMergeQueue\":false}}}\x1b[0m\n";
        assert_eq!(parse_is_in_merge_queue(stdout), Some(false));
    }
}
