//! Realtime GitHub CI status for the current branch, driven by the `gh` CLI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

// The pure run→state reduction is shared with the agent's `ci` tool.
pub use xai_grok_sandbox::ci_state::{CiStatus, GhRun, ci_from_runs, map_ci_status, parse_gh_runs};

/// Minimum interval between off-thread `gh` refreshes for the same target.
const CI_REFRESH_TTL: Duration = Duration::from_secs(30);

/// How often the event loop re-arms its CI poll while an agent view is up.
pub const CI_POLL_INTERVAL: Duration = CI_REFRESH_TTL;

/// How long after its last refresh a cache entry still counts as describing the branch on screen.
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
/// session with no input.
static CI_CHANGE_TX: LazyLock<Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>> =
    LazyLock::new(|| Mutex::new(None));

/// How long one full breath of the "in progress" pulse takes, measured on the wall clock.
pub const CI_PULSE_PERIOD: Duration = Duration::from_secs(4);

/// The pulse's HSV value bounds: the dot dims to [`CI_PULSE_MIN_VALUE`] and brightens to [`CI_PULSE_MAX_VALUE`].
pub const CI_PULSE_MIN_VALUE: f32 = 0.25;
pub const CI_PULSE_MAX_VALUE: f32 = 0.80;

/// The pulse's factor in `[min, max]` (each in `0..=1`) after `elapsed` of
/// wall-clock time, `min` + `(max-min)·(1+sin)/2`.
pub fn pulse_value(elapsed: Duration, min: f32, max: f32) -> f32 {
    let phase = elapsed.as_secs_f32() * std::f32::consts::TAU / CI_PULSE_PERIOD.as_secs_f32();
    let unit = (phase.sin() + 1.0) / 2.0;
    min + unit * (max - min)
}

/// The process's pulse epoch.
static PULSE_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Wall-clock time since the pulse epoch, which is what the render path feeds
/// [`animate_value_at`].
pub fn pulse_elapsed() -> Duration {
    PULSE_EPOCH.elapsed()
}

/// The color the dot takes after `elapsed` of wall-clock time: `base`'s hue and
/// saturation with its HSV value pulsed across both bounds.
pub fn animate_value_at(elapsed: Duration, base: (u8, u8, u8), min: f32, max: f32) -> (u8, u8, u8) {
    let (h, s, _v) = rgb_to_hsv(base);
    hsv_to_rgb((h, s, pulse_value(elapsed, min, max)))
}

/// The in-progress dot's color right now: [`animate_value_at`] sampled at
/// [`pulse_elapsed`], at the shipped bounds.
pub fn in_progress_dot_color(base: (u8, u8, u8)) -> (u8, u8, u8) {
    animate_value_at(
        pulse_elapsed(),
        base,
        CI_PULSE_MIN_VALUE,
        CI_PULSE_MAX_VALUE,
    )
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

/// Run the real `gh` CI-status command for `branch` in `repo_root` and return
/// the parsed runs + tri-state color. Thin shell-out wrapper; all parsing is
/// delegated to the pure [`parse_gh_runs`] / [`ci_from_runs`].
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
    let mut runs = parse_gh_runs(&output.stdout)?;
    // `--branch` filters server-side; this is the belt to that suspenders.
    runs.retain(|run| match run.head_branch.as_deref() {
        Some(reported) => reported == branch,
        None => true,
    });
    (!runs.is_empty()).then_some(runs)
}

/// Talk to the unsandboxed CI-status host worker for a sandboxed session,
/// returning an [`Output`] shaped like a `gh` run's stdout.
///
/// It is authoritative: when the worker replies with the nothing-usable
/// sentinel, we hand the caller an `Output` whose body attenuates to "no
/// runs", so the dot degrades to the "off" state rather than falling through
/// to an in-jail `gh` spawn. Only a genuinely absent/unusable worker
/// connection returns `None`.
fn run_gh_via_ci_host(repo_root: &Path, args: &[&str], fd: i32) -> Option<std::process::Output> {
    #[cfg(unix)]
    {
        let _ = repo_root;
        let branch = args.windows(2).find_map(|pair| match pair {
            [flag, value] if *flag == "--branch" => Some(*value),
            _ => None,
        });
        let Some(branch) = branch else {
            return None;
        };
        let body = xai_grok_sandbox::ci_host::query_ci_host(fd, branch)?;
        // Carry the payload the same way a real `gh` stdout would, plus a
        // synthetic success status so the caller's parse path is unchanged.
        return Some(std::process::Output {
            status: success_exit_status(),
            stdout: body,
            stderr: Vec::new(),
        });
    }
    #[cfg(not(unix))]
    {
        let _ = (repo_root, args, fd);
        None
    }
}

fn success_exit_status() -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(0)
    }
    #[cfg(not(unix))]
    {
        std::process::ExitStatus::default()
    }
}

/// Run the real `gh` CI-status command.
fn run_gh(repo_root: &Path, args: &[&str]) -> Option<std::process::Output> {
    if let Some(fd) = ci_host_fd() {
        // Sandboxed: the host worker is the only way to reach `gh`.
        return run_gh_via_ci_host(repo_root, args, fd);
    }
    run_gh_direct(repo_root, args)
}

use xai_grok_sandbox::ci_host::ci_host_fd;

/// The direct, unsandboxed `gh` invocation used when no host worker was
/// handed to us (a normal session).
fn run_gh_direct(repo_root: &Path, args: &[&str]) -> Option<std::process::Output> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(args)
        .current_dir(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    xai_grok_tools::util::detach_std_command(&mut cmd);
    cmd.envs(xai_grok_tools::util::pager_env());
    // `gh` colourises even piped `--json` output under CLICOLOR_FORCE or GH_FORCE_TTY (inherited from terminal-launched dev environments).
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
/// throttled off-thread `gh` refresh when the entry is missing or stale.
pub fn ci_status_lazy(repo_root: &Path, branch: &str) -> Option<CiStatus> {
    let cached = ci_status_peek(repo_root, branch);
    refresh_ci_status(repo_root, branch);
    cached
}

/// Read the cached CI status for `(repo_root, branch)` without scheduling
/// anything.
pub fn ci_status_peek(repo_root: &Path, branch: &str) -> Option<CiStatus> {
    let key = CiCacheKey {
        repo_root: repo_root.to_path_buf(),
        branch: branch.to_string(),
    };
    let cache = CI_CACHE.lock().ok()?;
    cache.get(&key).and_then(|(status, _)| *status)
}

/// Whether a recently-polled branch under `repo_root` is mid-run, i.e. the
/// dot is in the state that animates ([`CiStatus::Yellow`] pulses).
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
    // Reserve the slot with a fresh timestamp BEFORE spawning so this frame's other reads (and the next few frames).
    cache.insert(key.clone(), (cached, Instant::now()));
    drop(cache);
    spawn_ci_refresh(key, cached);
}

/// Register the channel an off-thread poll nudges when a branch's color
/// changes.
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

/// Ask the event loop for one repaint when this poll moved the dot.
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
    use std::io::{Read, Write};

    fn run(status: &str, conclusion: &str) -> GhRun {
        run_in("CI", status, conclusion)
    }

    fn run_in(workflow: &str, status: &str, conclusion: &str) -> GhRun {
        GhRun {
            status: status.to_string(),
            conclusion: conclusion.to_string(),
            head_branch: Some("feature/x".into()),
            workflow_name: workflow.to_string(),
            // The dot folds runs to a colour and never addresses one, so it asks `gh` for no id.
            database_id: None,
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
        assert_eq!(
            map_ci_status(Some("completed"), Some("success")),
            CiStatus::Green
        );
    }

    #[test]
    fn map_no_signal_is_off() {
        assert_eq!(map_ci_status(None, None), CiStatus::Off);
        assert_eq!(
            map_ci_status(Some("completed"), Some("neutral")),
            CiStatus::Off
        );
        assert_eq!(
            map_ci_status(Some("completed"), Some("skipped")),
            CiStatus::Off
        );
    }

    #[test]
    fn ci_from_runs_red_wins_over_another_workflow_in_progress() {
        // A failing workflow reports red even while a different one churns.
        let runs = vec![
            run_in("Release", "in_progress", ""),
            run_in("CI", "completed", "failure"),
        ];
        assert_eq!(ci_from_runs(runs), CiStatus::Red);
    }

    #[test]
    fn ci_from_runs_ignores_a_run_a_newer_push_superseded() {
        // gh lists newest first.
        let runs = vec![
            run_in("CI", "in_progress", ""),
            run_in("CI", "completed", "cancelled"),
        ];
        assert_eq!(ci_from_runs(runs), CiStatus::Yellow);

        let settled = vec![
            run_in("CI", "completed", "success"),
            run_in("CI", "completed", "cancelled"),
            run_in("CI", "completed", "failure"),
        ];
        assert_eq!(ci_from_runs(settled), CiStatus::Green);
    }

    #[test]
    fn ci_from_runs_yellow_when_in_progress_only() {
        let runs = vec![
            run_in("CI", "in_progress", ""),
            run_in("Release", "queued", ""),
        ];
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
        // Exactly the shape `gh run list --json status,conclusion,headBranch, workflowName` emits: camelCase keys, newest run first.
        let json = br#"[{"conclusion":"","status":"in_progress","headBranch":"master","workflowName":"CI"},{"conclusion":"failure","status":"completed","headBranch":"master","workflowName":"Release"}]"#;
        let runs = parse_gh_runs(json).expect("parseable");
        assert_eq!(runs.len(), 2);
        // The camelCase keys must reach their snake_case fields — defaulting them away is invisible.
        assert_eq!(runs[0].head_branch.as_deref(), Some("master"));
        assert_eq!(runs[0].workflow_name, "CI");
        assert_eq!(runs[1].workflow_name, "Release");
        // A failed workflow in the set → red.
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
        // spawn a background poll.
        assert_eq!(
            ci_status_lazy(std::path::Path::new("/lazy/no-runtime"), "master"),
            None
        );
    }

    fn cache_seed(repo_root: &str, branch: &str, status: CiStatus, age: Duration) {
        seed_for_test(Path::new(repo_root), branch, status, age);
    }

    /// Whether the cache holds an entry for this exact target.
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
        // A miss must not leave a reservation behind: peek is for callers off the render path.
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
        // Another repo's run must not animate this.
        assert!(!ci_dot_animating(Path::new("/anim/other")));

        // An entry nobody refreshes any more ages out.
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
        // The poll landed on the same color the dot already shows.
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

    /// Serialises the tests that publish a host-worker fd through the process environment.
    static CI_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn ci_env_lock() -> std::sync::MutexGuard<'static, ()> {
        CI_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish an in-process peer speaking the worker's protocol over
    /// `CI_HOST_FD_ENV`, exactly the way the jail boundary hands the fd to the
    /// jailed pager. Answer one `gh-status <branch>` request with `json`.
    ///
    /// The peer asserts the request shape, so a caller that reached `gh` some
    /// other way, or asked for the wrong thing, fails here rather than silently
    /// reading whatever the peer felt like sending. Callers hold
    /// [`ci_env_lock`] for as long as the variable must stay set.
    #[cfg(unix)]
    fn publish_ci_host_peer(json: &'static [u8]) -> i32 {
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let raw = std::os::unix::io::AsRawFd::as_raw_fd(&ours);
        std::thread::spawn(move || {
            let mut peer = theirs;
            let mut buf = [0u8; 8192];
            let n = peer.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(
                request.starts_with("gh-status "),
                "the jailed side must ask in the worker's fixed shape, got: {request}"
            );
            peer.write_all(json).unwrap();
            peer.write_all(b"\n").unwrap();
            peer.flush().unwrap();
        });
        // SAFETY: the tests that mutate this variable serialise on
        // `CI_ENV_LOCK`, and nothing else in the process reads it.
        unsafe {
            std::env::set_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV, raw.to_string());
        }
        // Leak `ours` so the fd stays open and unique for this process.
        std::mem::forget(ours);
        raw
    }

    /// Drive the SHIPPED CI-status path exactly as a `--sandbox` session does.
    /// `gh_ci_status` → `run_gh` → the `GROK_CI_HOST_FD` env read → the host
    /// worker, with no fd passed by hand.
    ///
    /// `repo_root` does not exist. A real in-jail `gh` spawn could only fail:
    /// reading a color back at all proves the answer came over the inherited
    /// worker connection.
    #[test]
    #[cfg(unix)]
    fn the_shipped_ci_status_reads_the_host_worker_the_jail_hands_it() {
        let _env = ci_env_lock();
        let _fd = publish_ci_host_peer(
            br#"[{"status":"completed","conclusion":"success","headBranch":"master","workflowName":"CI"}]"#,
        );
        let (runs, status) = gh_ci_status(Path::new("/no/such/repo"), "master");
        assert_eq!(
            status,
            CiStatus::Green,
            "a green host answer must reach the dot ({runs:?})"
        );
        unsafe { std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV) };
    }

    #[test]
    #[cfg(unix)]
    fn the_shipped_ci_status_shows_an_in_progress_host_answer_as_yellow() {
        let _env = ci_env_lock();
        let _fd = publish_ci_host_peer(
            br#"[{"status":"in_progress","conclusion":"","headBranch":"master","workflowName":"CI"}]"#,
        );
        let (_runs, status) = gh_ci_status(Path::new("/no/such/repo"), "master");
        assert_eq!(
            status,
            CiStatus::Yellow,
            "a live run must pulse, not go dark"
        );
        unsafe { std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV) };
    }

    #[test]
    #[cfg(unix)]
    fn the_shipped_ci_status_degrades_to_off_on_the_worker_sentinel() {
        // The worker's nothing-usable sentinel (`gh` failed, or the branch has no runs).
        let _env = ci_env_lock();
        let _fd = publish_ci_host_peer(b".");
        let (runs, status) = gh_ci_status(Path::new("/no/such/repo"), "master");
        assert_eq!(status, CiStatus::Off);
        assert!(runs.is_empty());
        unsafe { std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV) };
    }

    /// Drive `run_gh_via_ci_host` against an in-process peer speaking the real
    /// host-worker protocol, returning the `Output` it would hand a caller.
    /// The peer consumes a request, replies with one JSON line, and verifies
    /// it was handed the exact `--branch` we asked for (confinement).
    fn host_peer_reply(json: &'static [u8]) -> std::process::Output {
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let ours_raw = std::os::unix::io::AsRawFd::as_raw_fd(&ours);
        std::thread::spawn(move || {
            let mut peer = theirs;
            let mut buf = [0u8; 8192];
            let n = peer.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            // The request must be the fixed `gh-status <branch>` shape —
            // the confined worker never accepts anything else.
            let branch = req
                .strip_prefix("gh-status ")
                .expect("request must use the gh-status prefix")
                .trim_end();
            assert_eq!(branch, "master", "branch token must survive intact");
            peer.write_all(json).unwrap();
            peer.write_all(b"\n").unwrap();
            peer.flush().unwrap();
        });
        // Present the fd to the transport exactly the way the jail boundary
        // does.
        unsafe {
            std::env::set_var(
                xai_grok_sandbox::ci_host::CI_HOST_FD_ENV,
                ours_raw.to_string(),
            );
        }
        let output = run_gh_via_ci_host(
            Path::new("/repo"),
            &["run", "list", "--branch", "master"],
            ours_raw,
        )
        .expect("host result");
        unsafe {
            std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV);
        }
        // Leak `ours` so its fd stays open and unique for this test process.
        std::mem::forget(ours);
        output
    }

    #[test]
    fn sandboxed_query_reduces_a_host_result_to_a_real_status() {
        let _env = ci_env_lock();
        // A host worker answering exactly what `gh run list --json` emits.
        let json = br#"[{"status":"completed","conclusion":"success","headBranch":"master","workflowName":"CI"}]"#;
        let output = host_peer_reply(json);
        // The transport produced a synthetic success `Output` shaped like a real `gh` run.
        let runs = parse_gh_runs(&output.stdout).expect("parse host JSON");
        assert_eq!(ci_from_runs(runs.iter().cloned()), CiStatus::Green);
        assert!(output.status.success(), "host result must read as success");
    }

    #[test]
    fn sandboxed_query_yellow_on_an_in_progress_host_result() {
        let _env = ci_env_lock();
        let json = br#"[{"status":"in_progress","conclusion":"","headBranch":"master","workflowName":"CI"}]"#;
        let output = host_peer_reply(json);
        let runs = parse_gh_runs(&output.stdout).expect("parse host JSON");
        assert_eq!(ci_from_runs(runs.iter().cloned()), CiStatus::Yellow);
    }

    #[test]
    fn sandboxed_query_degrades_to_off_on_a_malformed_host_answer() {
        let _env = ci_env_lock();
        // A worker replying with the "." sentinel (its `gh` failed / the branch had no runs) must read back as no status at all.
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let ours_raw = std::os::unix::io::AsRawFd::as_raw_fd(&ours);
        std::thread::spawn(move || {
            let mut peer = theirs;
            let mut buf = [0u8; 64];
            let _ = peer.read(&mut buf);
            peer.write_all(b".\n").unwrap();
            peer.flush().unwrap();
        });
        unsafe {
            std::env::set_var(
                xai_grok_sandbox::ci_host::CI_HOST_FD_ENV,
                ours_raw.to_string(),
            );
        }
        let got = run_gh_via_ci_host(
            Path::new("/repo"),
            &["run", "list", "--branch", "master"],
            ours_raw,
        );
        unsafe {
            std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV);
        }
        std::mem::forget(ours);
        assert_eq!(got, None, "sentinel must degrade to no status (off)");
    }

    #[test]
    fn repeated_polls_reuse_one_host_connection_and_stay_correct() {
        let _env = ci_env_lock();
        // The session must poll more than once (continuous refresh), and each poll must get a fresh.
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().expect("pair");
        let ours_raw = std::os::unix::io::AsRawFd::as_raw_fd(&ours);
        std::thread::spawn(move || {
            let mut peer = theirs;
            let ok: &[u8] =
                b"[{\"status\":\"completed\",\"conclusion\":\"success\",\"headBranch\":\"feat/x\",\"workflowName\":\"CI\"}]";
            let run: &[u8] =
                b"[{\"status\":\"in_progress\",\"conclusion\":\"\",\"headBranch\":\"feat/x\",\"workflowName\":\"CI\"}]";
            for answer in [ok, run, ok] {
                let mut buf = [0u8; 64];
                let _ = peer.read(&mut buf);
                peer.write_all(answer).unwrap();
                peer.write_all(b"\n").unwrap();
                peer.flush().unwrap();
            }
        });
        unsafe {
            std::env::set_var(
                xai_grok_sandbox::ci_host::CI_HOST_FD_ENV,
                ours_raw.to_string(),
            );
        }
        // Successive polls over the single inherited connection.
        let first = run_gh_via_ci_host(
            Path::new("/repo"),
            &["run", "list", "--branch", "feat/x"],
            ours_raw,
        )
        .expect("first");
        let second = run_gh_via_ci_host(
            Path::new("/repo"),
            &["run", "list", "--branch", "feat/x"],
            ours_raw,
        )
        .expect("second");
        let third = run_gh_via_ci_host(
            Path::new("/repo"),
            &["run", "list", "--branch", "feat/x"],
            ours_raw,
        )
        .expect("third");
        unsafe {
            std::env::remove_var(xai_grok_sandbox::ci_host::CI_HOST_FD_ENV);
        }
        // Keep this test's worker fd unique across the process; see the sibling `host_peer_reply` for why.
        std::mem::forget(ours);

        let s1 = ci_from_runs(parse_gh_runs(&first.stdout).expect("a").iter().cloned());
        let s2 = ci_from_runs(parse_gh_runs(&second.stdout).expect("b").iter().cloned());
        let s3 = ci_from_runs(parse_gh_runs(&third.stdout).expect("c").iter().cloned());
        assert_eq!(s1, CiStatus::Green);
        assert_eq!(s2, CiStatus::Yellow);
        assert_eq!(s3, CiStatus::Green);
    }

    #[test]
    fn sandboxed_query_with_no_branch_is_degraded_off() {
        // Without a `--branch` in the args there is nothing to ask the worker
        // for; the transport must refuse rather than query garbage.
        assert_eq!(
            run_gh_via_ci_host(Path::new("/repo"), &["run", "list"], 0),
            None
        );
    }

    #[test]
    fn refresh_without_runtime_leaves_a_reservation_and_never_panics() {
        // No tokio runtime here, so nothing can poll `gh`; the call must still be infallible.
        let repo = "/refresh/no-runtime";
        refresh_ci_status(Path::new(repo), "master");
        assert!(cache_has(repo, "master"));
        assert_eq!(ci_status_peek(Path::new(repo), "master"), None);
    }

    /// Walk one render cadence over `span` of wall-clock time, sampling the
    /// SHIPPED pulse once per frame the way the render loop does. Report the
    /// wall-clock duration of one full breath. The elapsed time between the
    /// first peak samples.
    fn measured_period(step: Duration, span: Duration) -> Duration {
        let mut samples: Vec<(Duration, f32)> = Vec::new();
        let mut elapsed = Duration::ZERO;
        while elapsed <= span {
            samples.push((
                elapsed,
                pulse_value(elapsed, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE),
            ));
            elapsed += step;
        }
        let peaks: Vec<Duration> = samples
            .windows(3)
            .filter(|w| w[1].1 >= w[0].1 && w[1].1 >= w[2].1)
            .map(|w| w[1].0)
            .collect();
        assert!(
            peaks.len() >= 2,
            "a {span:?} span at {step:?} per frame must contain two peaks"
        );
        peaks[1] - peaks[0]
    }

    #[test]
    fn the_pulse_period_is_wall_clock_time_not_a_frame_count() {
        // Both cadences the event loop uses.
        let slow_step = Duration::from_millis(83);
        let fast_step = Duration::from_millis(33);
        let span = CI_PULSE_PERIOD * 3;

        let slow = measured_period(slow_step, span);
        let fast = measured_period(fast_step, span);

        // Each cadence measures the shipped period, within one frame of its own
        // sampling resolution.
        for (measured, step) in [(slow, slow_step), (fast, fast_step)] {
            let error = measured.abs_diff(CI_PULSE_PERIOD);
            assert!(
                error <= step * 2,
                "one breath must be {CI_PULSE_PERIOD:?} at a {step:?} cadence, \
                 measured {measured:?}"
            );
        }
        // And both cadences agree with each other: a frame-counted pulse
        // cannot do this, because its period scales with the frame rate.
        assert!(
            slow.abs_diff(fast) <= slow_step * 2,
            "the breath must take the same wall-clock time at either cadence: \
             slow {slow:?} vs fast {fast:?}"
        );
    }

    #[test]
    fn pulse_value_stays_between_min_and_max_and_is_periodic() {
        for ms in 0..(CI_PULSE_PERIOD.as_millis() as u64 * 2) {
            let v = pulse_value(
                Duration::from_millis(ms),
                CI_PULSE_MIN_VALUE,
                CI_PULSE_MAX_VALUE,
            );
            assert!(
                (CI_PULSE_MIN_VALUE..=CI_PULSE_MAX_VALUE).contains(&v),
                "{ms}ms -> {v}"
            );
        }
        // Phase extremes: sin peaks a quarter period in (max) and bottoms out at quarters (min).
        let quarter = CI_PULSE_PERIOD / 4;
        assert!(
            (pulse_value(quarter, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE) - CI_PULSE_MAX_VALUE)
                .abs()
                < 1e-2
        );
        assert!(
            (pulse_value(quarter * 3, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE) - CI_PULSE_MIN_VALUE)
                .abs()
                < 1e-2
        );
        // And the period is the period.
        assert!(
            (pulse_value(Duration::ZERO, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE)
                - pulse_value(CI_PULSE_PERIOD, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE))
            .abs()
                < 1e-2
        );
    }

    #[test]
    fn animated_value_preserves_hue_and_oscillates_brightness() {
        let base = (224, 175, 104); // theme.warning-ish
        let (h, s, _) = rgb_to_hsv(base);
        assert!(h > 30.0 && h < 90.0, "expected a yellow hue, got {h}");
        assert!(s > 0.5);

        // A quarter period apart: the brightest sample, then the dimmest.
        let quarter = CI_PULSE_PERIOD / 4;
        let bright = animate_value_at(quarter, base, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE);
        let dim = animate_value_at(quarter * 3, base, CI_PULSE_MIN_VALUE, CI_PULSE_MAX_VALUE);
        // Preserves hue and saturation; only value changes.
        let (h1, s1, _) = rgb_to_hsv(dim);
        let (h2, s2, _) = rgb_to_hsv(bright);
        assert!((h1 - h2).abs() < 1.0, "hue must be preserved");
        assert!((s1 - s2).abs() < 0.05, "saturation must be preserved");
        // The bright frame is strictly lightened (higher luminance).
        let lum =
            |(r, g, b): (u8, u8, u8)| 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        assert!(lum(bright) > lum(dim), "brighter frame must be lighter");
    }

    /// The shipped status-bar call site must feed the pulse ELAPSED WALL TIME.
    /// The pulse math above is only half the fix. A render path that still
    /// passes `scrollback.animation_tick()` would keep the old frame-counted
    /// behavior with a `Duration`-shaped cast.
    ///
    /// Structural, because the call site lives inside a ratatui render pass that
    /// no unit test can run. It reads the real file and asserts the CI dot's
    /// in-progress arm goes through `in_progress_dot_color` (the wall-clock
    /// entry point) and never through the tick counter.
    #[test]
    fn the_status_bar_dot_pulses_from_wall_clock_time() {
        const RENDER_SRC: &str = include_str!("app/agent_view/render.rs");
        let arm = RENDER_SRC
            .split("CiStatus::Yellow => {")
            .nth(1)
            .expect("the status bar must have an in-progress arm for the dot");
        let arm = &arm[..arm.find("CiStatus::Green").unwrap_or(arm.len())];
        assert!(
            arm.contains("in_progress_dot_color("),
            "the in-progress dot must pulse through the wall-clock entry point: {arm}"
        );
        assert!(
            !arm.contains("animation_tick"),
            "the pulse phase must not come from the frame tick: {arm}"
        );
        assert!(
            !arm.contains("animate_value("),
            "the tick-counter entry point must be gone from the render path: {arm}"
        );
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
