//! Realtime GitHub CI status for the current branch, driven by the `gh` CLI.
//!
//! Issue #40: show a red/yellow/green dot next to the branch whose color
//! reflects the branch's live CI/check status. We deliberately source this
//! from the `gh` binary (`gh run list`) rather than reimplementing a GitHub
//! REST/OAuth client. The [`gh` CLI][gh] discovers the owning repo from the
//! git remote at the process cwd, in a thread-safe, pure, dependency-free
//! unit that the TUI's render path and the unit tests share.
//!
//! The module keeps two pieces cleanly separated:
//!   1. [`map_ci_status`] / [`ci_from_runs`] — pure, dependency-free state →
//!      color mapping (failing/errored → red, in-progress/pending → yellow,
//!      success → green, nothing → off). These have no terminal or process
//!      dependencies and are exercised directly by the unit tests (and the
//!      headless load check).
//!   2. [`gh_ci_status_runs`] — the thin subprocess that runs the real `gh`
//!      command and returns the raw JSON plus the parsed [`CiStatus`], so the
//!      renderer's refresh path drives the *same* command shape a user gets
//!      from their terminal (`gh run list --branch <branch>`).
//!
//! "Realtime" is preserved via a per-(repo-root, branch) cache that mirrors
//! [`crate::git_info`]'s throttled refresh: reads return the last polled
//! value immediately and kick off an off-thread `gh` poll on a TTL, so the dot
//! tracks fresh CI state instead of a value captured once at startup.
//!
//! Renders alone cannot carry that promise, because the session watching its
//! own CI is precisely the one drawing no frames: the event loop parks until
//! something asks it to move. Three pieces close that gap, and all of them
//! must stay wired or the dot silently freezes at whatever it last showed:
//!   - [`CI_POLL_INTERVAL`] — the loop's own poll timer keeps calling
//!     [`refresh_ci_status`] with no frames in sight;
//!   - [`set_change_notifier`] — a poll that lands on a *different* color asks
//!     the loop for one repaint, so red/green arrives without a keypress;
//!   - [`ci_dot_animating`] — the app demands animation ticks while a run is
//!     in flight, which is what actually moves the yellow pulse.
//!
//! [gh]: https://cli.github.com/

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Minimum interval between off-thread `gh` refreshes for the same target, so
/// a per-frame caller can't spawn a storm of `gh` subprocesses.
const CI_REFRESH_TTL: Duration = Duration::from_secs(30);

/// How often the event loop re-arms its CI poll while an agent view is up.
/// The render path only refreshes the dot on frames it actually draws, and an
/// idle session draws none, so the poll timer is what keeps the color true
/// while the user sits and watches a run.
pub const CI_POLL_INTERVAL: Duration = CI_REFRESH_TTL;

/// How long after its last refresh a cache entry still counts as describing
/// the branch on screen. Entries for branches nobody renders any more stop
/// being refreshed and age out of [`ci_dot_animating`], so a checked-out-and-
/// abandoned branch can't keep an idle session animating.
const CI_ENTRY_FRESH_FOR: Duration = Duration::from_secs(90);

/// Only the `headBranch` the user cares about is ever fed into the cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CiCacheKey {
    repo_root: PathBuf,
    branch: String,
}

type CiCacheEntry = (Option<CiStatus>, Instant);
static CI_CACHE: LazyLock<Mutex<HashMap<CiCacheKey, CiCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Nudged by an off-thread poll that lands on a *different* color, so a
/// session with no input and no animation still repaints the moment CI goes
/// red or green. Registered once by the event loop; `None` in tests and in
/// any headless caller, where the send is simply skipped.
static CI_CHANGE_TX: LazyLock<Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>> =
    LazyLock::new(|| Mutex::new(None));

/// The tri-state CI color for a branch, plus the "no CI" absent state.
///
/// Rendered as a single colored dot (red / yellow / green) beside the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatus {
    /// No CI signal: `gh` unavailable, unauthenticated, no runs, or the
    /// branch has no workflow runs at all. The dot is simply not drawn (or
    /// drawn in a neutral dim style).
    Off,
    /// A run has failed or errored (failing/errored/cancelled/timed-out).
    Red,
    /// A run is currently in progress / queued / pending (non-terminal).
    Yellow,
    /// A run has concluded successfully.
    Green,
}

/// A single workflow run as reported by `gh run list --json`.
#[derive(Debug, Clone, Deserialize)]
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
    /// Branch that this run was triggered against. `gh run list` reports each
    /// run's owning branch; we filter/verify against the requested branch.
    #[serde(default)]
    pub head_branch: Option<String>,
}

impl GhRun {
    /// A run that finished in a failing or errored state.
    fn is_terminal_failure(&self) -> bool {
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
    fn is_in_progress(&self) -> bool {
        let status_pending = !self.status.is_empty()
            && !self.status.eq_ignore_ascii_case("completed");
        status_pending || (self.conclusion.is_empty() && !self.status.is_empty())
    }

    /// A run that concluded successfully.
    fn is_success(&self) -> bool {
        self.conclusion.eq_ignore_ascii_case("success")
    }
}

/// Pure status→color mapping for a single run's `status`/`conclusion`.
///
/// This is the thin, dependency-free unit from criterion 2 and is exercised
/// directly by the unit tests against representative CI states:
///   - failing/errored conclusion → [`CiStatus::Red`]
///   - non-terminal status (in progress / pending / queued) → [`CiStatus::Yellow`]
///   - successful conclusion → [`CiStatus::Green`]
///   - no signal → [`CiStatus::Off`]
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
    };
    ci_from_runs(std::iter::once(run))
}

/// Fold a set of runs (as returned by `gh run list`) into one tri-state color.
///
/// Precedence (two passes, so a failing/errored run on the branch reports red
/// even while a parallel run is still in progress):
///   1. any failing/errored run → [`CiStatus::Red`]
///   2. else any in-progress/pending run → [`CiStatus::Yellow`]
///   3. else any successful run → [`CiStatus::Green`]
///   4. else → [`CiStatus::Off`]
pub fn ci_from_runs<I>(runs: I) -> CiStatus
where
    I: IntoIterator<Item = GhRun>,
{
    let runs: Vec<GhRun> = runs.into_iter().collect();
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

/// Parse the raw stdout of `gh run list --json` into runs + a tri-state color.
///
/// Pure and headless-safe. Returns `None` when `gh` produced no usable JSON
/// (or the output decodes to empty), so callers degrade to "no CI status"
/// instead of panicking.
pub fn parse_gh_runs(stdout: &[u8]) -> Option<Vec<GhRun>> {
    // `gh` can colourise piped JSON (e.g. `GH_FORCE_TTY`, `--color always`),
    // which would break serde parsing; strip ANSI CSI just as the PR status
    // extension does.
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
/// Mirrors the `gh` colourising workaround used by the PR-status extension.
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

/// Pure sine-wave factor used to animate the "CI running" dot's HS**V** value.
///
/// Returns a value in `[min, max]` (each in `0..=1`) that oscillates over the
/// render tick, `min` + `(max-min)·(1+sin)/2`. Kept dependency-free and
/// headless so the animation math is directly unit-testable.
pub fn sine_value_factor(tick: u64, min: f32, max: f32) -> f32 {
    // Full sine cycle (2π) every 48 ticks. A pulsing dot demands only slow
    // ticks (83ms, `app_view::SLOW_TICK_INTERVAL`), so that is one breath
    // per ~4s.
    let phase = (tick as f32) * std::f32::consts::TAU / 48.0;
    let unit = (phase.sin() + 1.0) / 2.0; // 0..=1
    min + unit * (max - min)
}

/// Pure RGB → HSV: returns `(hue 0..=360, saturation 0..=1, value 0..=1)`.
pub fn rgb_to_hsv((r, g, b): (u8, u8, u8)) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        let h = 60.0 * ((g - b) / delta % 6.0);
        if h < 0.0 { h + 360.0 } else { h }
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// Pure HSV → RGB with the **Value** scaled to `v` (`0..=1`).
pub fn hsv_to_rgb((h, s, v): (f32, f32, f32)) -> (u8, u8, u8) {
    if s <= 0.0 {
        let v = (v * 255.0).round() as u8;
        return (v, v, v);
    }
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// Animate the CI dot's color for a given `tick`: re-scale the HS**V** value of
/// `base` in a sine wave between `min` and `max` (percent as fractions), fully
/// preserving hue and saturation. Used to pulse the yellow "in progress" dot.
pub fn animate_value(tick: u64, base: (u8, u8, u8), min: f32, max: f32) -> (u8, u8, u8) {
    let (h, s, _v) = rgb_to_hsv(base);
    let v = sine_value_factor(tick, min, max);
    hsv_to_rgb((h, s, v))
}

/// Run the real `gh` CI-status command for `branch` in `repo_root` and return
/// the parsed runs + tri-state color. Thin shell-out wrapper; all parsing is
/// delegated to the pure [`parse_gh_runs`] / [`ci_from_runs`].
///
/// Returns `(Vec<GhRun>, CiStatus)`. When `gh` is missing, unauthenticated, or
/// there are no runs for the branch, the run list is empty and the status is
/// [`CiStatus::Off`] — never a panic.
pub fn gh_ci_status(repo_root: &Path, branch: &str) -> (Vec<GhRun>, CiStatus) {
    let runs = gh_run_list(repo_root, branch).unwrap_or_default();
    let status = ci_from_runs(runs.iter().cloned());
    (runs, status)
}

/// The exact `gh run list` invocation shape used both by the TUI's refresh
/// path and by the integration test's real run. The repository is discovered
/// by `gh` from the git remote at `repo_root` (no `-R` hand-authored against a
/// token/API client).
fn gh_run_list(repo_root: &Path, branch: &str) -> Option<Vec<GhRun>> {
    let output = run_gh(
        repo_root,
        &[
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "10",
            "--json",
            "status,conclusion,headBranch,workflowName",
        ],
    )?;
    parse_gh_runs(&output.stdout)
}

fn run_gh(repo_root: &Path, args: &[&str]) -> Option<std::process::Output> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(args)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    xai_grok_tools::util::detach_std_command(&mut cmd);
    cmd.envs(xai_grok_tools::util::pager_env());
    // `gh` colourises even piped `--json` output under CLICOLOR_FORCE or
    // GH_FORCE_TTY (inherited from terminal-launched dev environments) and
    // forcing beats NO_COLOR in gh's precedence; CLICOLOR_FORCE=0 is gh's
    // documented off-switch.
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env_remove("GH_FORCE_TTY");
    let output = cmd.output().ok()?;
    if !output.status.success() {
        let stderr_snippet: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(200)
            .collect();
        tracing::debug!(
            status = %output.status,
            stderr = %stderr_snippet,
            "gh run list failed"
        );
        return None;
    }
    Some(output)
}

/// Read the cached CI status for `(repo_root, branch)`, scheduling a
/// throttled off-thread `gh` refresh when the entry is missing or stale, so
/// the dot reflects fresh CI state rather than a startup capture. Never
/// blocks and never spawns `gh` synchronously on the render path — call this
/// from render code.
pub fn ci_status_lazy(repo_root: &Path, branch: &str) -> Option<CiStatus> {
    let cached = ci_status_peek(repo_root, branch);
    refresh_ci_status(repo_root, branch);
    cached
}

/// Read the cached CI status for `(repo_root, branch)` without scheduling
/// anything. Free of subprocesses, I/O, and cache mutation, so callers that
/// run outside the render path — [`ci_dot_animating`], the tick-demand check —
/// can ask what the dot currently shows without driving a poll.
pub fn ci_status_peek(repo_root: &Path, branch: &str) -> Option<CiStatus> {
    let key = CiCacheKey {
        repo_root: repo_root.to_path_buf(),
        branch: branch.to_string(),
    };
    let cache = CI_CACHE.lock().ok()?;
    cache.get(&key).and_then(|(status, _)| *status)
}

/// Whether a recently-polled branch under `repo_root` is mid-run, i.e. the dot
/// is in the one state that animates ([`CiStatus::Yellow`] pulses).
///
/// Drives the app's animation-tick demand: without it the pulse only moves
/// while something *else* is already redrawing the screen, which is never the
/// case for the session sitting idle watching its own CI.
pub fn ci_dot_animating(repo_root: &Path) -> bool {
    let Ok(cache) = CI_CACHE.lock() else {
        return false;
    };
    cache.iter().any(|(key, (status, polled_at))| {
        key.repo_root == repo_root
            && *status == Some(CiStatus::Yellow)
            && polled_at.elapsed() < CI_ENTRY_FRESH_FOR
    })
}

/// Schedule a throttled off-thread `gh` poll for `(repo_root, branch)`.
/// Returns without spawning when the last poll is younger than
/// [`CI_REFRESH_TTL`], so a per-frame caller can't start a subprocess storm.
pub fn refresh_ci_status(repo_root: &Path, branch: &str) {
    let key = CiCacheKey {
        repo_root: repo_root.to_path_buf(),
        branch: branch.to_string(),
    };
    let Ok(mut cache) = CI_CACHE.lock() else {
        return;
    };
    let (cached, needs_refresh) = match cache.get(&key) {
        Some((info, ts)) => (*info, ts.elapsed() >= CI_REFRESH_TTL),
        None => (None, true),
    };
    if !needs_refresh {
        return;
    }
    // Reserve the slot with a fresh timestamp BEFORE spawning so this frame's
    // other reads (and the next few frames) don't spawn duplicate refreshes
    // until this one lands or the TTL elapses.
    cache.insert(key.clone(), (cached, Instant::now()));
    drop(cache);
    spawn_ci_refresh(key, cached);
}

/// Register the channel an off-thread poll nudges when a branch's color
/// changes. The event loop repaints on it; replacing an existing sender is
/// harmless (one loop owns the terminal).
pub fn set_change_notifier(tx: tokio::sync::mpsc::UnboundedSender<()>) {
    if let Ok(mut slot) = CI_CHANGE_TX.lock() {
        *slot = Some(tx);
    }
}

fn spawn_ci_refresh(key: CiCacheKey, previous: Option<CiStatus>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn_blocking(move || {
        let (_, status) = gh_ci_status(&key.repo_root, &key.branch);
        if let Ok(mut cache) = CI_CACHE.lock() {
            cache.insert(key, (Some(status), Instant::now()));
        }
        notify_if_changed(previous, status);
    });
}

/// Ask the event loop for one repaint when this poll moved the dot. Silent
/// when the color is unchanged (an idle loop must stay parked), when no loop
/// registered a sender (unit tests, headless callers), and when the loop has
/// already exited.
fn notify_if_changed(previous: Option<CiStatus>, now: CiStatus) {
    if previous == Some(now) {
        return;
    }
    if let Ok(slot) = CI_CHANGE_TX.lock()
        && let Some(tx) = slot.as_ref()
    {
        let _ = tx.send(());
    }
}

/// Seed a polled result for `(repo_root, branch)` as if a `gh` poll had
/// landed `age` ago. Lets the render/tick tests exercise the dot without a
/// `gh` binary, a network, or a repo. Tests must use a repo path of their own:
/// the cache is process-global.
#[cfg(test)]
pub(crate) fn seed_for_test(repo_root: &Path, branch: &str, status: CiStatus, age: Duration) {
    let key = CiCacheKey {
        repo_root: repo_root.to_path_buf(),
        branch: branch.to_string(),
    };
    let polled_at = Instant::now()
        .checked_sub(age)
        .expect("test age is representable");
    if let Ok(mut cache) = CI_CACHE.lock() {
        cache.insert(key, (Some(status), polled_at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str, conclusion: &str) -> GhRun {
        GhRun {
            status: status.to_string(),
            conclusion: conclusion.to_string(),
            head_branch: Some("feature/x".into()),
        }
    }

    #[test]
    fn map_failure_conclusion_is_red() {
        // Representative failing/errored states → red.
        for (status, conclusion) in [
            ("completed", "failure"),
            ("completed", "cancelled"),
            ("completed", "timed_out"),
            ("completed", "action_required"),
        ] {
            assert_eq!(
                map_ci_status(Some(status), Some(conclusion)),
                CiStatus::Red,
                "{status}/{conclusion} should map to red"
            );
        }
    }

    #[test]
    fn map_in_progress_is_yellow() {
        // Representative non-terminal states → yellow.
        for (status, conclusion) in [
            ("queued", ""),
            ("in_progress", ""),
            ("pending", ""),
            ("requested", ""),
            ("waiting", ""),
        ] {
            assert_eq!(
                map_ci_status(Some(status), Some(conclusion)),
                CiStatus::Yellow,
                "{status} should map to yellow"
            );
        }
    }

    #[test]
    fn map_success_is_green() {
        assert_eq!(map_ci_status(Some("completed"), Some("success")), CiStatus::Green);
    }

    #[test]
    fn map_no_signal_is_off() {
        assert_eq!(map_ci_status(None, None), CiStatus::Off);
        assert_eq!(map_ci_status(Some("completed"), Some("neutral")), CiStatus::Off);
        assert_eq!(map_ci_status(Some("completed"), Some("skipped")), CiStatus::Off);
    }

    #[test]
    fn ci_from_runs_red_wins_over_later_in_progress() {
        // A branch with a failed run reports red even while other runs churn.
        let runs = vec![run("in_progress", ""), run("completed", "failure")];
        assert_eq!(ci_from_runs(runs), CiStatus::Red);
    }

    #[test]
    fn ci_from_runs_yellow_when_in_progress_only() {
        let runs = vec![run("in_progress", ""), run("queued", "")];
        assert_eq!(ci_from_runs(runs), CiStatus::Yellow);
    }

    #[test]
    fn ci_from_runs_green_when_all_success() {
        let runs = vec![run("completed", "success"), run("completed", "success")];
        assert_eq!(ci_from_runs(runs), CiStatus::Green);
    }

    #[test]
    fn ci_from_runs_off_with_no_runs() {
        assert_eq!(ci_from_runs(std::iter::empty::<GhRun>()), CiStatus::Off);
        // Neutral/skipped-only → off (nothing conclusive to report).
        let runs = vec![run("completed", "skipped"), run("completed", "neutral")];
        assert_eq!(ci_from_runs(runs), CiStatus::Off);
    }

    #[test]
    fn parse_gh_runs_real_json() {
        let json = br#"[{"conclusion":"failure","status":"completed","headBranch":"master"},{"conclusion":"","status":"in_progress","headBranch":"master"}]"#;
        let runs = parse_gh_runs(json).expect("parseable");
        assert_eq!(runs.len(), 2);
        // One terminal failure in the set → red.
        assert_eq!(ci_from_runs(runs.iter().cloned()), CiStatus::Red);
    }

    #[test]
    fn parse_gh_runs_strips_forced_ansi_color() {
        // gh colourises piped JSON under forced colour; parsing must survive.
        let json = b"\x1b[1;37m[{\x1b[m \x1b[1;34m\"conclusion\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"success\"\x1b[m\x1b[1;37m,\x1b[m \x1b[1;34m\"status\"\x1b[m\x1b[1;37m:\x1b[m \x1b[32m\"completed\"\x1b[m\x1b[1;37m}]\x1b[m\n";
        let runs = parse_gh_runs(json).expect("parseable even with colour");
        assert_eq!(runs.len(), 1);
        assert_eq!(ci_from_runs(runs.iter().cloned()), CiStatus::Green);
    }

    #[test]
    fn parse_gh_runs_empty_is_none() {
        assert!(parse_gh_runs(b"[]").is_none());
        assert!(parse_gh_runs(b"not json").is_none());
    }

    #[test]
    fn ci_cache_lazy_read_without_runtime_returns_none() {
        // With no tokio runtime (plain unit test) `ci_status_lazy` cannot
        // spawn a background poll, so a cache miss reads back `None` and the
        // cache is left in a well-defined state — no panic, graceful "no CI".
        // The path is this test's own: the cache is process-global, and
        // clearing it wholesale would race every other test that seeds one.
        assert_eq!(
            ci_status_lazy(std::path::Path::new("/lazy/no-runtime"), "master"),
            None
        );
    }

    fn cache_seed(repo_root: &str, branch: &str, status: CiStatus, age: Duration) {
        seed_for_test(Path::new(repo_root), branch, status, age);
    }

    /// Whether the cache holds an entry for this exact target. Tests key off
    /// their own unique repo paths rather than clearing the shared cache, so
    /// they stay correct when the suite runs threaded in one process.
    fn cache_has(repo_root: &str, branch: &str) -> bool {
        CI_CACHE.lock().expect("cache").contains_key(&CiCacheKey {
            repo_root: PathBuf::from(repo_root),
            branch: branch.to_string(),
        })
    }

    #[test]
    fn peek_reads_the_cache_without_scheduling_a_poll() {
        let repo = "/peek/repo";
        assert_eq!(ci_status_peek(Path::new(repo), "master"), None);
        // A miss must not leave a reservation behind: peek is for callers off
        // the render path, and reserving here would suppress the next real
        // refresh for a whole TTL.
        assert!(!cache_has(repo, "master"));
        cache_seed(repo, "master", CiStatus::Green, Duration::ZERO);
        assert_eq!(
            ci_status_peek(Path::new(repo), "master"),
            Some(CiStatus::Green)
        );
    }

    #[test]
    fn only_a_fresh_in_progress_entry_demands_animation() {
        let repo = "/anim/repo";
        assert!(
            !ci_dot_animating(Path::new(repo)),
            "empty cache never ticks"
        );

        for settled in [CiStatus::Green, CiStatus::Red, CiStatus::Off] {
            cache_seed(repo, "master", settled, Duration::ZERO);
            assert!(
                !ci_dot_animating(Path::new(repo)),
                "{settled:?} is a static dot"
            );
        }

        cache_seed(repo, "master", CiStatus::Yellow, Duration::ZERO);
        assert!(ci_dot_animating(Path::new(repo)), "a live run pulses");
        // Another repo's run must not animate this one.
        assert!(!ci_dot_animating(Path::new("/anim/other")));

        // An entry nobody refreshes any more ages out, so an abandoned branch
        // cannot keep an idle session redrawing forever.
        cache_seed(repo, "master", CiStatus::Yellow, CI_ENTRY_FRESH_FOR);
        assert!(
            !ci_dot_animating(Path::new(repo)),
            "stale entry stops ticks"
        );
    }

    #[test]
    fn change_notifier_fires_only_when_the_color_actually_changes() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        set_change_notifier(tx);
        // The poll landed on the same color the dot already shows: repainting
        // would be pure churn on an otherwise-parked loop.
        notify_if_changed(Some(CiStatus::Green), CiStatus::Green);
        assert!(rx.try_recv().is_err());
        notify_if_changed(Some(CiStatus::Green), CiStatus::Red);
        assert!(rx.try_recv().is_ok(), "green -> red must repaint");
        // First poll of a session: nothing was on screen, the dot appears.
        notify_if_changed(None, CiStatus::Yellow);
        assert!(rx.try_recv().is_ok(), "first result must repaint");
        if let Ok(mut slot) = CI_CHANGE_TX.lock() {
            *slot = None;
        }
    }

    #[test]
    fn refresh_without_runtime_leaves_a_reservation_and_never_panics() {
        // No tokio runtime here, so nothing can poll `gh`; the call must still
        // be infallible, and it must reserve the slot so the render path does
        // not re-spawn on every frame.
        let repo = "/refresh/no-runtime";
        refresh_ci_status(Path::new(repo), "master");
        assert!(cache_has(repo, "master"));
        assert_eq!(ci_status_peek(Path::new(repo), "master"), None);
    }

    #[test]
    fn sine_value_factor_stays_between_min_and_max() {
        // The yellow pulse must never leave [0.25, 0.80].
        for tick in 0..200 {
            let v = sine_value_factor(tick, 0.25, 0.80);
            assert!((0.25..=0.80).contains(&v), "tick {tick} -> {v}");
        }
        // Phase extremes hit the bounds: sin peaks at tick 12 (max) and
        // bottoms out at tick 36 (min) for a 48-tick cycle.
        assert!((sine_value_factor(12, 0.25, 0.80) - 0.80).abs() < 1e-2);
        assert!((sine_value_factor(36, 0.25, 0.80) - 0.25).abs() < 1e-2);
        // And it is periodic.
        assert!((sine_value_factor(0, 0.25, 0.80) - sine_value_factor(48, 0.25, 0.80)).abs() < 1e-2);
    }

    #[test]
    fn animated_value_preserves_hue_and_oscillates_brightness() {
        // A mid-yellow base: hue ~60°, saturation ~1, value ~0.5.
        let base = (224, 175, 104); // theme.warning-ish
        let (h, s, _) = rgb_to_hsv(base);
        assert!(h > 30.0 && h < 90.0, "expected a yellow hue, got {h}");
        assert!(s > 0.5);

        // One full cycle apart: tick 12 is the brightest (value 0.80), tick 36
        // the dimmest (value 0.25).
        let dim = animate_value(36, base, 0.25, 0.80);
        let bright = animate_value(12, base, 0.25, 0.80);
        // Preserves hue and saturation; only value changes.
        let (h1, s1, _) = rgb_to_hsv(dim);
        let (h2, s2, _) = rgb_to_hsv(bright);
        assert!((h1 - h2).abs() < 1.0, "hue must be preserved");
        assert!((s1 - s2).abs() < 0.05, "saturation must be preserved");
        // The bright frame is strictly lightened (higher luminance).
        let lum = |(r, g, b): (u8, u8, u8)| 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        assert!(lum(bright) > lum(dim), "brighter frame must be lighter");
    }

    #[test]
    fn hsv_roundtrip_approximates_input() {
        // Round-tripping a known RGB through HSV→RGB(V=orig) is lossy but close.
        let (r, g, b) = (224, 175, 104);
        let hsv = rgb_to_hsv((r, g, b));
        let back = hsv_to_rgb((hsv.0, hsv.1, hsv.2));
        assert!((r as i16 - back.0 as i16).abs() <= 2);
        assert!((g as i16 - back.1 as i16).abs() <= 2);
        assert!((b as i16 - back.2 as i16).abs() <= 2);
    }
}
