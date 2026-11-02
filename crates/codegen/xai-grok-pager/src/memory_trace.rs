//! Process memory tracing: durable JSONL evidence for memory investigations.
//!
//! This module records the process memory over time, so a footprint problem can be read after the process is gone:
//!
//! - **Samples**: footprint/RSS and the thread count every `GROK_MEMTRACE_INTERVAL_SECS` (default 30s) from a detached thread. - **Thresholds**.
//! - **Thresholds**: the physical footprint crossing a bucket (`GROK_MEMTRACE_THRESHOLD_MB`, default 1 GiB, doubling) fires the *threshold hook*.
//!   Buckets re-arm once the footprint halves, so a long-lived process can evidence repeated growth cycles.
//!
//! ## Files
//!
//! `$GROK_HOME/memtrace/<start-ts>-<pid>.jsonl`, and a `.1` after 4 MiB rotation.
//! Files are created lazily on the first event so short-lived CLI invocations leave no debris.
//! Traces contain **process memory numbers only** (no user content), so they are safe to ship for analysis.
//!
//! ## Threshold hook
//!
//! Absent a hook, crossing is still recorded locally and logged via `tracing::warn!`.
//!
//! Everything is inert until [`start`] runs (tests use scoped sinks).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[path = "memory_trace_wait.rs"]
mod memory_trace_wait;
use memory_trace_wait::wait_full_interval;

pub use xai_tty_utils::{ProcessResources, sample_process_memory};

/// One JSONL trace line. `kind` discriminates which optional fields apply.
#[derive(serde::Serialize)]
struct TraceEvent<'a> {
    /// Unix millis.
    ts_ms: u64,
    /// "start" | "sample" | "threshold" | "crash".
    kind: &'a str,
    /// macOS `phys_footprint` (resident dirty + compressed + swapped).
    /// This is the number Activity Monitor "Memory" and `vmmap`'s "Physical footprint" show.
    /// `None` where unavailable (Linux).
    #[serde(skip_serializing_if = "Option::is_none")]
    footprint_bytes: Option<u64>,
    /// Resident set size.
    #[serde(skip_serializing_if = "Option::is_none")]
    rss_bytes: Option<u64>,
    /// Live OS thread count, recorded for offline analysis (the threshold buckets key on footprint only).
    /// A count that scales with work done (background tasks, subagents) instead of holding a flat baseline is a leak.
    #[serde(skip_serializing_if = "Option::is_none")]
    threads: Option<u64>,
    /// Threshold: the bucket that fired.
    #[serde(skip_serializing_if = "Option::is_none")]
    threshold_bytes: Option<u64>,
    /// Start: pid and binary version, for joining traces to sessions/hosts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
}

static THRESHOLD_HOOK: OnceLock<fn(&Path, u64)> = OnceLock::new();

/// It fires at most once per bucket per growth cycle (buckets re-arm after
/// the footprint halves). Idempotent; first caller wins.
pub fn install_threshold_hook(hook: fn(&Path, u64)) {
    let _ = THRESHOLD_HOOK.set(hook);
}

/// Threshold buckets that fire exactly once per growth cycle.
/// A bucket fires when the footprint reaches it while armed, then stays disarmed until the footprint drops below half the bucket.
/// The hysteresis keeps purge/regrow cycles near a boundary from spamming.
struct Thresholds {
    buckets: Vec<u64>,
    armed: Vec<bool>,
}

impl Thresholds {
    fn new(first_bytes: u64, count: usize) -> Self {
        let mut buckets = Vec::with_capacity(count);
        let mut b = first_bytes.max(64 << 20); // floor: 64 MiB
        for _ in 0..count {
            buckets.push(b);
            b = b.saturating_mul(2);
        }
        let armed = vec![true; buckets.len()];
        Self { buckets, armed }
    }

    /// Feed a footprint observation; returns the buckets that fire on it.
    fn observe(&mut self, footprint: u64) -> Vec<u64> {
        let mut fired = Vec::new();
        for (armed, &bucket) in self.armed.iter_mut().zip(&self.buckets) {
            if *armed && footprint >= bucket {
                *armed = false;
                fired.push(bucket);
            } else if !*armed && footprint < bucket / 2 {
                *armed = true;
            }
        }
        fired
    }
}

const ROTATE_BYTES_DEFAULT: u64 = 4 << 20; // 4 MiB, then one .1 rotation.

struct Sink {
    /// Lazily-created JSONL file; `None` until the first event.
    file: Mutex<Option<std::fs::File>>,
    path: PathBuf,
    bytes_written: AtomicU64,
    rotate_bytes: u64,
    thresholds: Mutex<Thresholds>,
}

/// Process-global sink.
/// `RwLock` (not `OnceLock`) so tests can install scoped sinks; production installs exactly once via [`start`].
static SINK: RwLock<Option<std::sync::Arc<Sink>>> = RwLock::new(None);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Sink {
    fn new(path: PathBuf, rotate_bytes: u64, first_threshold: u64) -> Self {
        Self {
            file: Mutex::new(None),
            path,
            bytes_written: AtomicU64::new(0),
            rotate_bytes,
            thresholds: Mutex::new(Thresholds::new(first_threshold, 6)),
        }
    }

    fn write_line(&self, line: &str) {
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_none() {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => *guard = Some(f),
                Err(_) => return, // Tracing must never break the pager.
            }
        }
        if let Some(f) = guard.as_mut()
            && f.write_all(line.as_bytes()).is_ok()
        {
            let _ = f.write_all(b"\n");
            let total = self
                .bytes_written
                .fetch_add(line.len() as u64 + 1, Ordering::Relaxed);
            if total > self.rotate_bytes {
                // Rotate: rename the current file to .1, replacing any previous .1
                // Reopen the live file eagerly so a reader between events never observes a missing trace
                let mut rotated = self.path.clone().into_os_string();
                rotated.push(".1");
                let _ = std::fs::rename(&self.path, PathBuf::from(rotated));
                self.bytes_written.store(0, Ordering::Relaxed);
                *guard = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .ok();
            }
        }
    }

    fn emit(&self, event: &TraceEvent<'_>) {
        if let Ok(line) = serde_json::to_string(event) {
            self.write_line(&line);
        }
    }

    /// Record a sample and run the threshold logic.
    fn record_sample(&self) {
        let mem = sample_process_memory();
        self.emit(&TraceEvent {
            ts_ms: now_ms(),
            kind: "sample",
            footprint_bytes: mem.footprint_bytes,
            rss_bytes: mem.rss_bytes,
            threads: mem.threads,
            threshold_bytes: None,
            pid: None,
            version: None,
        });
        // Threshold gauge: physical footprint where available (macOS), otherwise RSS (Linux)
        // Without the RSS fallback, Linux thresholds would never fire
        if let Some(gauge) = mem.footprint_bytes.or(mem.rss_bytes) {
            let fired = match self.thresholds.lock() {
                Ok(mut t) => t.observe(gauge),
                Err(p) => p.into_inner().observe(gauge),
            };
            for bucket in fired {
                self.fire_threshold(bucket, gauge);
            }
        }
    }

    fn fire_threshold(&self, bucket: u64, footprint: u64) {
        self.emit(&TraceEvent {
            ts_ms: now_ms(),
            kind: "threshold",
            footprint_bytes: Some(footprint),
            rss_bytes: None,
            threads: None,
            threshold_bytes: Some(bucket),
            pid: None,
            version: None,
        });
        tracing::warn!(
            target: "memtrace",
            footprint_bytes = footprint,
            threshold_bytes = bucket,
            trace = %self.path.display(),
            "process footprint crossed memory threshold"
        );
        if let Some(hook) = THRESHOLD_HOOK.get() {
            hook(&self.path, bucket);
        }
    }
}

fn with_sink(f: impl FnOnce(&Sink)) {
    let guard = match SINK.read() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(sink) = guard.as_ref() {
        f(sink);
    }
}

/// Env: disable with `GROK_MEMTRACE=0|false|off`.
fn enabled_by_env() -> bool {
    !matches!(
        std::env::var("GROK_MEMTRACE").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn interval_from_env() -> Duration {
    let secs = std::env::var("GROK_MEMTRACE_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    // Production floors at 5s so accidental tiny values don't spam samples.
    // Test builds allow 1s so isolated regressions can finish quickly.
    let min_secs = if cfg!(test) { 1 } else { 5 };
    Duration::from_secs(secs.max(min_secs))
}

fn first_threshold_from_env() -> u64 {
    std::env::var("GROK_MEMTRACE_THRESHOLD_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1024)
        .saturating_mul(1 << 20)
}

/// Start memory tracing: install the process-global sink under `dir` and spawn the detached sampler thread. Call
/// once from the composition-root binary, AFTER the intercepts for short-lived children (the mermaid render
/// worker), so helpers don't trace.
pub fn start(dir: PathBuf) {
    if !enabled_by_env() {
        return;
    }
    {
        let mut guard = match SINK.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_some() {
            return; // Already started.
        }
        let start_ts = now_ms() / 1000;
        let path = dir.join(format!("{start_ts}-{}.jsonl", std::process::id()));
        *guard = Some(std::sync::Arc::new(Sink::new(
            path,
            ROTATE_BYTES_DEFAULT,
            first_threshold_from_env(),
        )));
    }
    let interval = interval_from_env();
    // Detached sampler: holds no locks across waits, and the JoinHandle is dropped so nothing else can unpark this thread (see memory_trace_wait)
    // The thread is named so `sample` and Instruments can show it; it dies with the process
    let _ = std::thread::Builder::new()
        .name("grok-memtrace".into())
        .spawn(move || {
            let mut wrote_start = false;
            loop {
                wait_full_interval(interval);
                with_sink(|s| {
                    if !wrote_start {
                        wrote_start = true;
                        s.emit(&TraceEvent {
                            ts_ms: now_ms(),
                            kind: "start",
                            footprint_bytes: None,
                            rss_bytes: None,
                            threads: None,
                            threshold_bytes: None,
                            pid: Some(std::process::id()),
                            version: Some(env!("CARGO_PKG_VERSION")),
                        });
                    }
                    s.record_sample();
                });
            }
        });
}

/// Appends without taking the sink's write lock.
/// A panic raised while that lock is held therefore cannot hang the hook before the process reports the panic.
pub fn record_crash_sample() {
    let Ok(sink) = SINK.try_read() else {
        return;
    };
    let Some(path) = sink.as_ref().map(|s| s.path.clone()) else {
        return;
    };
    drop(sink);

    let mem = sample_process_memory();
    let Ok(line) = serde_json::to_string(&TraceEvent {
        ts_ms: now_ms(),
        kind: "crash",
        footprint_bytes: mem.footprint_bytes,
        rss_bytes: mem.rss_bytes,
        threads: mem.threads,
        threshold_bytes: None,
        pid: None,
        version: None,
    }) else {
        return;
    };

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

pub fn default_dir() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("memtrace")
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExportedTrace {
    pub name: String,
    pub data: Vec<u8>,
}

pub struct ExportLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
}

impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_files: 32,
            max_total_bytes: 16 << 20,
        }
    }
}

struct Candidate {
    stem: String,
    name: String,
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

fn parse_trace_name(name: &str) -> Option<String> {
    let stem = name
        .strip_suffix(".jsonl")
        .or_else(|| name.strip_suffix(".jsonl.1"))?;
    let (start_ts, pid) = stem.split_once('-')?;
    start_ts.parse::<u64>().ok()?;
    pid.parse::<u32>().ok()?;
    Some(stem.to_owned())
}

pub fn collect_for_export(dir: &Path, limits: ExportLimits) -> Vec<ExportedTrace> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut candidates: Vec<Candidate> = entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = parse_trace_name(&name)?;
            let meta = entry.metadata().ok()?;
            Some(Candidate {
                stem,
                name,
                path: entry.path(),
                len: meta.len(),
                modified: meta.modified().unwrap_or(UNIX_EPOCH),
            })
        })
        .collect();

    let mut last_activity: std::collections::HashMap<String, SystemTime> =
        std::collections::HashMap::new();
    for candidate in &candidates {
        let seen = last_activity
            .entry(candidate.stem.clone())
            .or_insert(candidate.modified);
        *seen = (*seen).max(candidate.modified);
    }

    candidates.sort_by_cached_key(|c| {
        (
            std::cmp::Reverse(last_activity.get(&c.stem).copied().unwrap_or(c.modified)),
            c.stem.clone(),
            c.name.clone(),
        )
    });

    let mut exported = Vec::new();
    let mut total_bytes: u64 = 0;
    for candidate in candidates {
        if exported.len() >= limits.max_files {
            break;
        }
        if total_bytes.saturating_add(candidate.len) > limits.max_total_bytes {
            continue;
        }
        let Ok(data) = std::fs::read(&candidate.path) else {
            continue;
        };
        total_bytes = total_bytes.saturating_add(data.len() as u64);
        exported.push(ExportedTrace {
            name: candidate.name,
            data,
        });
    }
    exported
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Install a scoped sink writing to `path` (tiny rotation cap, high thresholds).
    /// Returns a guard restoring the previous sink on drop.
    /// Tests using this must serialize on the `MEMTRACE_SINK` serial key; the sink is process-global.
    pub(crate) struct SinkGuard(Option<std::sync::Arc<Sink>>);

    impl Drop for SinkGuard {
        fn drop(&mut self) {
            let mut guard = match SINK.write() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            *guard = self.0.take();
        }
    }

    pub(crate) fn install_test_sink(path: PathBuf, rotate_bytes: u64) -> SinkGuard {
        let mut guard = match SINK.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let prev = guard.take();
        *guard = Some(std::sync::Arc::new(Sink::new(
            path,
            rotate_bytes,
            u64::MAX >> 1, // never fire thresholds unless a test asks
        )));
        SinkGuard(prev)
    }

    pub(crate) fn record_sample_for_tests() {
        with_sink(|s| s.record_sample());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_fire_once_and_rearm_after_halving() {
        let mut t = Thresholds::new(1 << 30, 3); // 1 GiB, 2 GiB, 4 GiB
        assert!(t.observe(512 << 20).is_empty(), "below first bucket");
        assert_eq!(t.observe(1 << 30), vec![1 << 30], "first crossing fires");
        assert!(
            t.observe(1200 << 20).is_empty(),
            "staying above must not re-fire"
        );
        assert_eq!(
            t.observe(5 << 30),
            vec![2 << 30, 4 << 30],
            "one observation can cross several buckets"
        );
        // Dropping below half of 1 GiB re-arms only that bucket
        assert!(t.observe(400 << 20).is_empty());
        assert_eq!(
            t.observe(1 << 30),
            vec![1 << 30],
            "re-armed bucket fires again on the next growth cycle"
        );
    }

    #[test]
    fn thresholds_floor_prevents_degenerate_buckets() {
        let mut t = Thresholds::new(0, 2);
        assert!(
            t.observe(1 << 20).is_empty(),
            "buckets are floored at 64 MiB, tiny footprints never fire"
        );
        assert_eq!(t.observe(64 << 20), vec![64 << 20]);
    }

    #[test]
    #[serial_test::serial(MEMTRACE_SINK)]
    fn sample_events_are_valid_jsonl_and_rotate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let _guard = test_support::install_test_sink(path.clone(), 256);

        // Enough samples to exceed the 256-byte cap at least twice
        // The post-rotation file is created lazily by the NEXT write, so the final sample guarantees both files exist
        for _ in 0..16 {
            test_support::record_sample_for_tests();
        }
        // Rotation happened at the tiny cap: a `.1` exists and both files hold only valid JSON lines with the expected shape
        let rotated = dir.path().join("t.jsonl.1");
        assert!(rotated.exists(), "rotation must produce a .1 file");
        assert!(path.exists(), "rotation reopens the live file eagerly");
        for p in [&path, &rotated] {
            let body = std::fs::read_to_string(p).unwrap();
            for line in body.lines() {
                let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
                assert_eq!(v.get("kind").and_then(|k| k.as_str()), Some("sample"));
                assert!(
                    v.get("ts_ms")
                        .and_then(|t| t.as_u64())
                        .is_some_and(|n| n > 0)
                );
            }
        }
    }

    fn write_aged(dir: &Path, name: &str, len: usize, age_secs: u64) {
        let path = dir.join(name);
        std::fs::write(&path, vec![b'x'; len]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
    }

    fn names(exported: &[ExportedTrace]) -> Vec<&str> {
        exported.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    #[serial_test::serial(MEMTRACE_SINK)]
    fn crash_sample_lands_even_when_the_trace_directory_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent").join("t.jsonl");
        let _guard = test_support::install_test_sink(path.clone(), 1 << 20);

        record_crash_sample();

        let body = std::fs::read_to_string(&path).unwrap();
        let event: serde_json::Value = serde_json::from_str(body.trim()).unwrap();
        assert_eq!(event.get("kind").and_then(|k| k.as_str()), Some("crash"));
    }

    #[test]
    fn export_groups_each_process_and_leads_with_its_timeline() {
        let dir = tempfile::tempdir().unwrap();
        write_aged(dir.path(), "100-1.jsonl", 8, 60);
        write_aged(dir.path(), "100-1.jsonl.1", 8, 300);
        write_aged(dir.path(), "100-2.jsonl", 8, 5);
        write_aged(dir.path(), "100-2.jsonl.1", 8, 30);

        let exported = collect_for_export(dir.path(), ExportLimits::default());

        assert_eq!(
            names(&exported),
            [
                "100-2.jsonl",
                "100-2.jsonl.1",
                "100-1.jsonl",
                "100-1.jsonl.1"
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn export_ignores_files_it_did_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("credentials");
        std::fs::write(&elsewhere, "secret").unwrap();
        write_aged(dir.path(), "100-1.jsonl", 8, 0);
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join("200-2.jsonl")).unwrap();

        let exported = collect_for_export(dir.path(), ExportLimits::default());

        assert_eq!(names(&exported), ["100-1.jsonl"]);
    }

    #[test]
    fn export_skips_an_over_budget_trace_and_keeps_reading() {
        let dir = tempfile::tempdir().unwrap();
        write_aged(dir.path(), "200-1.jsonl", 128, 0);
        write_aged(dir.path(), "100-1.jsonl", 32, 60);

        let exported = collect_for_export(
            dir.path(),
            ExportLimits {
                max_files: usize::MAX,
                max_total_bytes: 64,
            },
        );

        assert_eq!(
            exported,
            vec![ExportedTrace {
                name: "100-1.jsonl".into(),
                data: vec![b'x'; 32],
            }]
        );
    }
}
