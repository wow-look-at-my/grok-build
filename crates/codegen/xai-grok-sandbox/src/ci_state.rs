//! Pure `gh run list` → CI-state reduction, free of processes and terminals.
//!
//! Both readers of CI state fold the same run list the same way: the pager's
//! status dot and the agent's `ci` tool. The reduction lives here, beside the
//! [`crate::ci_host`] transport that fetches the runs, so the dot and the tool
//! can never disagree about what red means.

use serde::Deserialize;

/// The tri-state CI colour for a branch, plus the "no CI" absent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatus {
    /// No CI signal: `gh` unavailable, unauthenticated, no runs, or the
    /// branch has no workflow runs at all.
    Off,
    /// A run has failed or errored (failing/errored/cancelled/timed-out).
    Red,
    /// A run is currently in progress / queued / pending (non-terminal).
    Yellow,
    /// A run has concluded successfully.
    Green,
}

impl CiStatus {
    /// The lowercase word a model or a log reads this state as.
    pub fn as_str(self) -> &'static str {
        match self {
            CiStatus::Off => "none",
            CiStatus::Red => "failing",
            CiStatus::Yellow => "in_progress",
            CiStatus::Green => "passing",
        }
    }

    /// Whether this state can still change on its own. A branch mid-run is
    /// worth waiting on; a settled one is not.
    pub fn is_terminal(self) -> bool {
        !matches!(self, CiStatus::Yellow)
    }
}

/// A single workflow run as reported by `gh run list --json`.
///
/// `gh` emits camelCase keys (`headBranch`, `workflowName`); without the
/// rename every non-single-word field silently deserialized to its default.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GhRun {
    /// GitHub `status` of the run: `queued`, `in_progress`, `completed`,
    /// `requested`, `waiting`, `pending` … — `""` when unknown.
    #[serde(default)]
    pub status: String,
    /// GitHub `conclusion` of a completed run: `success`, `failure`,
    /// `cancelled`, `neutral`, `skipped`, `timed_out`, `action_required`,
    /// `stale`, `startup_failure` … — empty (`""`) or null while the run is
    /// still in progress.
    #[serde(default)]
    pub conclusion: String,
    /// Branch this run was triggered against. `--branch` already filters
    /// server-side; callers re-check it so a run for another branch can never
    /// colour this branch's state.
    #[serde(default)]
    pub head_branch: Option<String>,
    /// Workflow this run belongs to (`""` when unknown). A branch normally
    /// has several — CI, release, previews — and they must be folded
    /// separately: see [`newest_run_per_workflow`].
    #[serde(default)]
    pub workflow_name: String,
    /// The run's numeric id, when the caller asked `gh` for it. This is what
    /// addresses a run's logs (`gh run view <id> --log-failed`).
    #[serde(default)]
    pub database_id: Option<u64>,
}

impl GhRun {
    /// A run that finished in a failing or errored state.
    pub fn is_terminal_failure(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "failure" | "cancelled" | "timed_out" | "action_required" | "stale" | "startup_failure"
        )
    }

    /// A run that is still running / queued (non-terminal).
    ///
    /// Anything not yet `completed` (queued/in_progress/pending/requested/
    /// waiting) is a live, moving CI signal → yellow. A `completed` run that
    /// is still missing a final conclusion is also treated as in-flight.
    pub fn is_in_progress(&self) -> bool {
        let status_pending =
            !self.status.is_empty() && !self.status.eq_ignore_ascii_case("completed");
        status_pending || (self.conclusion.is_empty() && !self.status.is_empty())
    }

    /// A run that concluded successfully.
    pub fn is_success(&self) -> bool {
        self.conclusion.eq_ignore_ascii_case("success")
    }
}

/// The `--json` field list that populates every [`GhRun`] field.
pub const RUN_JSON_FIELDS: &str = "status,conclusion,headBranch,workflowName,databaseId";

/// Pure status→state mapping for a single run's `status`/`conclusion`.
pub fn map_ci_status(status: Option<&str>, conclusion: Option<&str>) -> CiStatus {
    let conclusion = conclusion.unwrap_or("");
    let status = status.unwrap_or("");
    // No status *and* no conclusion → unknown/no signal.
    if status.is_empty() && conclusion.is_empty() {
        return CiStatus::Off;
    }
    let run = GhRun {
        status: status.to_string(),
        conclusion: conclusion.to_string(),
        head_branch: None,
        workflow_name: String::new(),
        database_id: None,
    };
    ci_from_runs(std::iter::once(run))
}

/// Fold a set of runs (as returned by `gh run list`) into one tri-state.
///
/// Only the newest run of each workflow counts — see
/// [`newest_run_per_workflow`]. Across those, precedence (two passes, so a
/// failing workflow reports red even while another is still in progress):
///   1. any failing/errored run → [`CiStatus::Red`]
///   2. else any in-progress/pending run → [`CiStatus::Yellow`]
///   3. else any successful run → [`CiStatus::Green`]
///   4. else → [`CiStatus::Off`]
pub fn ci_from_runs<I>(runs: I) -> CiStatus
where
    I: IntoIterator<Item = GhRun>,
{
    let runs = newest_run_per_workflow(runs);
    if runs.is_empty() {
        return CiStatus::Off;
    }
    // Pass 1 — a branch's CI is red while any run has failed/errored.
    if runs.iter().any(GhRun::is_terminal_failure) {
        return CiStatus::Red;
    }
    // Pass 2 — otherwise the branch is yellow while any run is still moving.
    if runs.iter().any(GhRun::is_in_progress) {
        return CiStatus::Yellow;
    }
    // Pass 3 — otherwise green when a run concluded successfully.
    if runs.iter().any(GhRun::is_success) {
        return CiStatus::Green;
    }
    // Only neutral/skipped/no-op runs on this branch → nothing conclusive.
    CiStatus::Off
}

/// Keep the newest run of each workflow, dropping the ones it superseded.
///
/// `gh run list` returns newest first and reaches back ten runs, so a branch
/// that has been pushed twice reports both. Pushing cancels the run in flight
/// (`concurrency.cancel-in-progress`), and a cancelled run is a failure — so
/// folding over the raw list paints the state red off a run the newer push
/// already replaced, and it stays red however green the branch gets.
///
/// Runs are grouped by workflow rather than collapsed to one, because a
/// branch's workflows are independent: a failing test workflow must still
/// report red while a release workflow is mid-upload.
pub fn newest_run_per_workflow<I>(runs: I) -> Vec<GhRun>
where
    I: IntoIterator<Item = GhRun>,
{
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    runs.into_iter()
        .filter(|run| seen.insert(run.workflow_name.clone()))
        .collect()
}

/// Parse the raw stdout of `gh run list --json` into runs.
///
/// Pure and headless-safe. Returns `None` when `gh` produced no usable JSON
/// (or the output decodes to empty), so callers degrade to "no CI status"
/// instead of panicking.
pub fn parse_gh_runs(stdout: &[u8]) -> Option<Vec<GhRun>> {
    // `gh` can colourise piped JSON (e.g. `GH_FORCE_TTY`, `--color always`),
    // which would break serde parsing; strip ANSI CSI first.
    let runs = strip_ansi_csi(stdout);
    let parsed = match serde_json::from_slice::<Vec<GhRun>>(&runs) {
        Ok(runs) => runs,
        Err(error) => {
            tracing::debug!(error = %error, "gh run list output did not parse as JSON");
            return None;
        }
    };
    if parsed.is_empty() {
        return None;
    }
    Some(parsed)
}

/// Best-effort decode of a raw ANSI-encoded byte buffer to plain bytes.
pub fn strip_ansi_csi(bytes: &[u8]) -> Vec<u8> {
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

    fn run_in(workflow: &str, status: &str, conclusion: &str) -> GhRun {
        GhRun {
            status: status.to_string(),
            conclusion: conclusion.to_string(),
            head_branch: Some("feature/x".into()),
            workflow_name: workflow.to_string(),
            database_id: None,
        }
    }

    #[test]
    fn a_failing_workflow_is_red_even_beside_a_live_one() {
        let runs = vec![
            run_in("Release", "in_progress", ""),
            run_in("CI", "completed", "failure"),
        ];
        assert_eq!(ci_from_runs(runs), CiStatus::Red);
    }

    #[test]
    fn a_run_a_newer_push_superseded_does_not_colour_the_branch() {
        // gh lists newest first. Pushing cancels the run in flight, so a
        // branch pushed twice reports [live, cancelled] for the SAME workflow.
        let runs = vec![
            run_in("CI", "in_progress", ""),
            run_in("CI", "completed", "cancelled"),
        ];
        assert_eq!(ci_from_runs(runs), CiStatus::Yellow);
    }

    #[test]
    fn terminal_states_are_the_ones_worth_no_more_waiting() {
        assert!(!CiStatus::Yellow.is_terminal());
        for settled in [CiStatus::Red, CiStatus::Green, CiStatus::Off] {
            assert!(settled.is_terminal(), "{settled:?} cannot change on its own");
        }
    }

    #[test]
    fn parse_reads_the_camel_case_keys_gh_emits() {
        let json = br#"[{"conclusion":"","status":"in_progress","headBranch":"master","workflowName":"CI","databaseId":42}]"#;
        let runs = parse_gh_runs(json).expect("parseable");
        assert_eq!(runs[0].head_branch.as_deref(), Some("master"));
        assert_eq!(runs[0].workflow_name, "CI");
        assert_eq!(runs[0].database_id, Some(42));
        assert_eq!(ci_from_runs(runs), CiStatus::Yellow);
    }

    #[test]
    fn parse_survives_forced_colour_and_refuses_junk() {
        let json = b"\x1b[1;37m[{\x1b[m \x1b[1;34m\"conclusion\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"success\"\x1b[m\x1b[1;37m,\x1b[m \x1b[1;34m\"status\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"completed\"\x1b[m\x1b[1;37m}]\x1b[m\n";
        let runs = parse_gh_runs(json).expect("parseable even with colour");
        assert_eq!(ci_from_runs(runs), CiStatus::Green);
        assert!(parse_gh_runs(b"[]").is_none());
        assert!(parse_gh_runs(b"not json").is_none());
    }
}
