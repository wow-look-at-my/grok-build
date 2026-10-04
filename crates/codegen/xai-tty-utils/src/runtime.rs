//! Worker-thread and blocking-pool policy for tokio runtimes.

use std::io;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Maximum runtime worker threads for any grok process.
#[allow(
    clippy::unwrap_used,
    reason = "const unwrap is evaluated at compile time and cannot panic at runtime"
)]
pub const MAX_WORKER_THREADS: NonZeroUsize = NonZeroUsize::new(8).unwrap();

/// Maximum Tokio blocking threads per runtime.
pub const MAX_BLOCKING_THREADS: usize = 16;

pub const BLOCKING_THREAD_KEEP_ALIVE: Duration = Duration::MAX;

const PREWARM_THREAD_WAIT: Duration = Duration::from_secs(5);

/// Pure, testable: `min(cores, MAX_WORKER_THREADS)`.
pub fn cap_worker_threads(cores: NonZeroUsize) -> NonZeroUsize {
    cores.min(MAX_WORKER_THREADS)
}

/// Reads the host: `min(available_parallelism, MAX_WORKER_THREADS)`.
pub fn capped_worker_threads() -> NonZeroUsize {
    cap_worker_threads(std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
}

/// Cap the blocking pool and keep idle workers for the process lifetime.
pub fn apply_blocking_pool(builder: &mut tokio::runtime::Builder) -> &mut tokio::runtime::Builder {
    builder
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_keep_alive(BLOCKING_THREAD_KEEP_ALIVE)
}

/// Apply the blocking-pool policy, build, and pre-warm the full pool.
/// Process-lifetime runtimes only.
pub fn build_with_blocking_pool(
    builder: &mut tokio::runtime::Builder,
) -> io::Result<tokio::runtime::Runtime> {
    let rt = apply_blocking_pool(builder).build()?;
    prewarm_blocking_pool(rt.handle())?;
    Ok(rt)
}

/// Create [`MAX_BLOCKING_THREADS`] overlapping blocking workers.
pub fn prewarm_blocking_pool(handle: &tokio::runtime::Handle) -> io::Result<()> {
    prewarm_blocking_pool_n(handle, MAX_BLOCKING_THREADS, PREWARM_THREAD_WAIT)
}

/// Create `n` overlapping blocking workers, waiting at most `wait` in total. Timed out before `n` workers started.
/// Already-started workers are released so `Runtime` drop does not hang.
pub fn prewarm_blocking_pool_n(
    handle: &tokio::runtime::Handle,
    n: usize,
    wait: Duration,
) -> io::Result<()> {
    let release = Arc::new(AtomicBool::new(false));
    let workers = park_blocking_workers(handle, n, &release, wait)?;
    release_parked_workers(&release, &workers);
    Ok(())
}

/// Park `n` overlapping `spawn_blocking` workers until [`release_parked_workers`]. `wait` overflow (`checked_add` is
/// `None`) waits forever. On timeout, already-started workers are released so `Runtime` drop does not hang.
fn park_blocking_workers(
    handle: &tokio::runtime::Handle,
    n: usize,
    release: &Arc<AtomicBool>,
    wait: Duration,
) -> io::Result<Vec<std::thread::Thread>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    for _ in 0..n {
        let ready_tx = ready_tx.clone();
        let release = Arc::clone(release);
        handle.spawn_blocking(move || {
            let _ = ready_tx.send(std::thread::current());
            while !release.load(Ordering::Acquire) {
                std::thread::park();
            }
        });
    }
    drop(ready_tx);

    let deadline = Instant::now().checked_add(wait);
    let mut workers = Vec::with_capacity(n);
    for started in 0..n {
        let got = match deadline {
            Some(d) => ready_rx
                .recv_timeout(d.saturating_duration_since(Instant::now()))
                .ok(),
            None => ready_rx.recv().ok(),
        };
        match got {
            Some(thread) => workers.push(thread),
            None => {
                release_parked_workers(release, &workers);
                while let Ok(thread) = ready_rx.try_recv() {
                    thread.unpark();
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("blocking pool pre-warm stalled after {started} of {n} threads"),
                ));
            }
        }
    }
    Ok(workers)
}

fn release_parked_workers(release: &AtomicBool, workers: &[std::thread::Thread]) {
    release.store(true, Ordering::Release);
    for thread in workers {
        thread.unpark();
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "runtime_eagain_tests.rs"]
mod eagain_tests;
