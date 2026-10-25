//! Run stand-ins read off a branch's checks.

use xai_grok_sandbox::ci_state::GhRun;

use super::{BranchRuns, RunSource, push_repo};

/// The JSON body is on stdout in each case.
const PR_CHECKS_VERDICT_CODES: [i32; 3] = [0, 1, 8];
const PR_CHECK_JSON_FIELDS: &str = "name,bucket,workflow,link";
/// GitHub's page cap for check runs.
const CHECK_RUNS_PER_PAGE: usize = 100;

/// The `gh` argv for a pull request's checks. This is the endpoint the `checks`
/// action reads, asked for as JSON.
pub fn pr_checks_args<'a>(branch: &'a str, repo: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["pr", "checks", branch, "--json", PR_CHECK_JSON_FIELDS];
    push_repo(&mut args, repo);
    args
}

/// The `gh api` path for the check runs on one commit.
pub fn commit_checks_path(repo: &str, sha: &str) -> String {
    format!("repos/{repo}/commits/{sha}/check-runs?per_page={CHECK_RUNS_PER_PAGE}")
}

/// The checks of the branch's pull request, else of the branch's pushed head
/// commit. An empty answer means neither reports a check.
pub(super) fn fetch(cwd: &std::path::Path, branch: &str, repo: Option<&str>) -> BranchRuns {
    let pr_checks = fetch_pr_checks(cwd, branch, repo);
    if !pr_checks.is_empty() {
        return BranchRuns {
            runs: pr_checks,
            source: RunSource::PullRequestChecks,
        };
    }
    let commit_checks = fetch_commit_checks(cwd, branch, repo);
    let source = if commit_checks.is_empty() {
        RunSource::RunList
    } else {
        RunSource::CommitChecks
    };
    BranchRuns {
        runs: commit_checks,
        source,
    }
}

fn fetch_pr_checks(cwd: &std::path::Path, branch: &str, repo: Option<&str>) -> Vec<GhRun> {
    let Some(response) = xai_grok_sandbox::ci_host::run_gh(cwd, &pr_checks_args(branch, repo))
    else {
        return Vec::new();
    };
    // A branch with no pull request exits 1 with no JSON. That is not an error here.
    if !PR_CHECKS_VERDICT_CODES.contains(&response.code) {
        tracing::debug!(branch, code = response.code, stderr = %response.stderr.trim(), "ci: no pull-request checks");
        return Vec::new();
    }
    parse_pr_checks(&response.stdout).unwrap_or_default()
}

fn fetch_commit_checks(cwd: &std::path::Path, branch: &str, repo: Option<&str>) -> Vec<GhRun> {
    let Some(repo) = repo else {
        return Vec::new();
    };
    // Only this checkout knows the branch's commit. A branch of another
    // repository has no local ref to read.
    if super::remote_repo(cwd).as_deref() != Some(repo) {
        return Vec::new();
    }
    let Some(sha) = pushed_commit(cwd, branch) else {
        return Vec::new();
    };
    let path = commit_checks_path(repo, &sha);
    let Some(response) = xai_grok_sandbox::ci_host::run_gh(cwd, &["api", path.as_str()]) else {
        return Vec::new();
    };
    if !response.success() {
        tracing::warn!(branch, %sha, code = response.code, stderr = %response.stderr.trim(), "ci: could not read the commit's check runs");
        return Vec::new();
    }
    parse_commit_checks(&response.stdout).unwrap_or_default()
}

/// The commit the branch points at on `origin`, else locally. The tracking
/// ref is what the last push or fetch saw. A local branch can hold commits
/// that were never pushed.
fn pushed_commit(cwd: &std::path::Path, branch: &str) -> Option<String> {
    ["refs/remotes/origin/", "refs/heads/"]
        .iter()
        .find_map(|prefix| rev_parse(cwd, &format!("{prefix}{branch}^{{commit}}")))
}

fn rev_parse(cwd: &std::path::Path, rev: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", rev])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some(sha)
}

#[derive(Debug, serde::Deserialize)]
struct PrCheck {
    #[serde(default)]
    name: String,
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    workflow: String,
    #[serde(default)]
    link: String,
}

/// `None` when the body is not a list of checks.
pub(super) fn parse_pr_checks(stdout: &str) -> Option<Vec<GhRun>> {
    let checks = serde_json::from_str::<Vec<PrCheck>>(stdout.trim()).ok()?;
    Some(checks.into_iter().map(run_from_pr_check).collect())
}

/// Each check keeps its own name as its workflow, so one job of a workflow
/// does not hide another job of that workflow.
fn run_from_pr_check(check: PrCheck) -> GhRun {
    let (status, conclusion) = match check.bucket.as_str() {
        "pass" => ("completed", "success"),
        "fail" => ("completed", "failure"),
        "cancel" => ("completed", "cancelled"),
        "skipping" => ("completed", "skipped"),
        "pending" => ("in_progress", ""),
        other => ("completed", other),
    };
    let workflow_name = if check.workflow.is_empty() {
        check.name
    } else {
        format!("{} / {}", check.workflow, check.name)
    };
    GhRun {
        status: status.to_string(),
        conclusion: conclusion.to_string(),
        head_branch: None,
        workflow_name,
        database_id: run_id_from_link(&check.link),
    }
}

#[derive(Debug, serde::Deserialize)]
struct CheckRunsPage {
    #[serde(default)]
    total_count: usize,
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, serde::Deserialize)]
struct CheckRun {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
}

/// `None` when the body is not a check-runs page.
pub(super) fn parse_commit_checks(stdout: &str) -> Option<Vec<GhRun>> {
    let page = serde_json::from_str::<CheckRunsPage>(stdout.trim()).ok()?;
    if page.total_count > page.check_runs.len() {
        tracing::warn!(
            total = page.total_count,
            read = page.check_runs.len(),
            "ci: the commit has more check runs than one page holds; the rest are not read"
        );
    }
    Some(
        page.check_runs
            .into_iter()
            .map(|check| GhRun {
                status: check.status,
                conclusion: check.conclusion.unwrap_or_default(),
                head_branch: None,
                database_id: check.details_url.as_deref().and_then(run_id_from_link),
                workflow_name: check.name,
            })
            .collect(),
    )
}

/// The run id in an Actions link such as
/// `https://github.com/o/r/actions/runs/123/job/456`. A check from another CI
/// system has no such link.
fn run_id_from_link(link: &str) -> Option<u64> {
    let (_, rest) = link.split_once("/actions/runs/")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_sandbox::ci_state::{CiStatus, ci_from_runs};

    #[test]
    fn the_worker_accepts_both_fallback_queries() {
        let pr: Vec<String> = pr_checks_args("feat/x", Some("o/r"))
            .iter()
            .map(|arg| arg.to_string())
            .collect();
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&pr), "{pr:?}");
        let sha = "a".repeat(40);
        let api = vec!["api".to_string(), commit_checks_path("o/r", &sha)];
        assert!(xai_grok_sandbox::ci_host::gh_args_allowed(&api), "{api:?}");
    }

    /// The shape `gh pr checks --json name,bucket,workflow,link` emits.
    #[test]
    fn pull_request_checks_fold_like_runs() {
        let body = r#"[
            {"name":"build","bucket":"pass","workflow":"CI","link":"https://github.com/o/r/actions/runs/11/job/1"},
            {"name":"test","bucket":"fail","workflow":"CI","link":"https://github.com/o/r/actions/runs/11/job/2"},
            {"name":"lint","bucket":"pending","workflow":"CI","link":""}
        ]"#;
        let runs = parse_pr_checks(body).expect("a list of checks");
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].workflow_name, "CI / test");
        assert_eq!(runs[1].database_id, Some(11));
        assert_eq!(runs[2].database_id, None);
        // A failing job is not hidden by its passing sibling in the same workflow.
        assert_eq!(ci_from_runs(runs.clone()), CiStatus::Red);
        let green = parse_pr_checks(r#"[{"name":"build","bucket":"pass","workflow":"CI"}]"#)
            .expect("a list of checks");
        assert_eq!(ci_from_runs(green), CiStatus::Green);
        let pending =
            parse_pr_checks(r#"[{"name":"build","bucket":"pending"}]"#).expect("a list of checks");
        assert_eq!(ci_from_runs(pending), CiStatus::Yellow);
        assert!(parse_pr_checks("no pull requests found for branch").is_none());
    }

    /// The shape `GET /repos/o/r/commits/{sha}/check-runs` answers.
    #[test]
    fn commit_check_runs_fold_like_runs() {
        let body = r#"{"total_count":2,"check_runs":[
            {"name":"build","status":"completed","conclusion":"success","details_url":"https://github.com/o/r/actions/runs/40/job/9"},
            {"name":"test","status":"in_progress","conclusion":null}
        ]}"#;
        let runs = parse_commit_checks(body).expect("a check-runs page");
        assert_eq!(runs[0].database_id, Some(40));
        assert_eq!(runs[1].conclusion, "");
        assert_eq!(ci_from_runs(runs), CiStatus::Yellow);
        let empty = parse_commit_checks(r#"{"total_count":0,"check_runs":[]}"#).expect("a page");
        assert!(empty.is_empty());
        assert!(parse_commit_checks("[]").is_none());
    }

    #[test]
    fn a_run_id_comes_only_out_of_an_actions_link() {
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/123/job/456"),
            Some(123)
        );
        assert_eq!(
            run_id_from_link("https://github.com/o/r/actions/runs/77"),
            Some(77)
        );
        assert_eq!(run_id_from_link("https://ci.example.com/build/5"), None);
        assert_eq!(run_id_from_link(""), None);
    }
}
