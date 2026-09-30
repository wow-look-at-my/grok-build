//! A process-wide cap on model requests in flight.

use std::future::Future;
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// The cap when nothing configures one.
pub const DEFAULT_MAX_PARALLEL_REQUESTS: u32 = 7;

/// The permit count that stands for "no cap". A limit of zero maps to it.
const UNCAPPED: usize = 1 << 20;

/// A FIFO gate with a limit that can change while slots are out.
pub struct RequestSlots {
    semaphore: Arc<Semaphore>,
    limits: Mutex<Limits>,
    waiting: AtomicUsize,
}

struct Limits {
    /// Permits the gate hands out, with [`UNCAPPED`] for no cap.
    capacity: usize,
    /// Permits a shrink could not take back because slots held them.
    owed: usize,
}

impl RequestSlots {
    /// A gate with `limit` slots. Zero means no cap.
    pub fn new(limit: u32) -> Self {
        let capacity = capacity_for(limit);
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            limits: Mutex::new(Limits { capacity, owed: 0 }),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Change the limit. Zero means no cap. A shrink below the slots in use
    /// takes effect as those slots come back. It never cuts a request off.
    pub fn set_limit(&self, limit: u32) {
        let new = capacity_for(limit);
        let mut limits = self.lock();
        let old = limits.capacity;
        if new > old {
            let grow = new - old;
            let repaid = grow.min(limits.owed);
            limits.owed -= repaid;
            self.semaphore.add_permits(grow - repaid);
        } else if new < old {
            let shrink = old - new;
            let taken = self.semaphore.forget_permits(shrink);
            limits.owed += shrink - taken;
        }
        limits.capacity = new;
    }

    /// The configured limit. Zero means no cap.
    pub fn limit(&self) -> u32 {
        let capacity = self.lock().capacity;
        if capacity == UNCAPPED {
            0
        } else {
            capacity as u32
        }
    }

    /// Requests that wait for a slot now.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// A slot if one is free now. It never jumps a queue that has waiters.
    pub fn try_acquire(self: &Arc<Self>) -> Option<RequestSlot> {
        let permit = Arc::clone(&self.semaphore).try_acquire_owned().ok()?;
        Some(RequestSlot {
            permit: Some(permit),
            slots: Arc::clone(self),
        })
    }

    /// Wait for a slot. The time in the queue is added to every
    /// [`timeout_excluding_queue`] that encloses this call.
    pub async fn acquire(self: &Arc<Self>) -> RequestSlot {
        if let Some(slot) = self.try_acquire() {
            return slot;
        }
        let queued_at = Instant::now();
        let clock = QUEUE_CLOCK.try_with(Arc::clone).ok();
        if let Some(clock) = &clock {
            clock.enter(queued_at);
        }
        let ahead = self.waiting.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            target: crate::sampling_log::TARGET,
            limit = self.limit(),
            ahead,
            event = "request_queued",
            "model request queued: every request slot is in use"
        );
        // The guard keeps the counts true when the caller drops this future.
        let _waiting = WaitGuard {
            slots: self,
            clock: clock.as_deref(),
        };
        let Ok(permit) = Arc::clone(&self.semaphore).acquire_owned().await else {
            unreachable!("nothing closes the request-slot semaphore");
        };
        tracing::info!(
            target: crate::sampling_log::TARGET,
            waited_ms = queued_at.elapsed().as_millis() as u64,
            event = "request_dequeued",
            "model request left the queue"
        );
        RequestSlot {
            permit: Some(permit),
            slots: Arc::clone(self),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Limits> {
        #[allow(clippy::disallowed_methods)]
        self.limits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn capacity_for(limit: u32) -> usize {
    if limit == 0 {
        UNCAPPED
    } else {
        (limit as usize).min(UNCAPPED - 1)
    }
}

struct WaitGuard<'a> {
    slots: &'a RequestSlots,
    clock: Option<&'a QueueClock>,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        self.slots.waiting.fetch_sub(1, Ordering::Relaxed);
        if let Some(clock) = self.clock {
            clock.leave(Instant::now());
        }
    }
}

/// One request's hold on the gate. Dropping it frees the slot.
pub struct RequestSlot {
    permit: Option<OwnedSemaphorePermit>,
    slots: Arc<RequestSlots>,
}

impl Drop for RequestSlot {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        let mut limits = self.slots.lock();
        if limits.owed > 0 {
            limits.owed -= 1;
            permit.forget();
        }
    }
}

static GLOBAL: LazyLock<Arc<RequestSlots>> =
    LazyLock::new(|| Arc::new(RequestSlots::new(DEFAULT_MAX_PARALLEL_REQUESTS)));

/// The gate every model request in this process goes through.
pub fn global() -> &'static Arc<RequestSlots> {
    &GLOBAL
}

/// Set the process-wide cap. Zero means no cap.
pub fn set_max_parallel_requests(limit: u32) {
    let previous = GLOBAL.limit();
    if previous != limit {
        tracing::info!(
            target: crate::sampling_log::TARGET,
            previous,
            limit,
            "max parallel model requests changed"
        );
    }
    GLOBAL.set_limit(limit);
}

tokio::task_local! {
    /// Set while a caller already holds a slot for the request it is about to send.
    static SLOT_HELD: ();
    /// The innermost [`timeout_excluding_queue`] around this task.
    static QUEUE_CLOCK: Arc<QueueClock>;
}

/// Run `fut`, whose request already holds a slot, without taking another.
pub(crate) async fn with_slot_held<F: Future>(fut: F) -> F::Output {
    SLOT_HELD.scope((), fut).await
}

/// Take a slot from the global gate, unless the caller holds one already.
pub(crate) async fn acquire_unless_held() -> Option<RequestSlot> {
    if SLOT_HELD.try_with(|_| ()).is_ok() {
        return None;
    }
    Some(global().acquire().await)
}

/// How long the requests under one timeout waited for a slot.
#[derive(Default)]
pub struct QueueClock {
    state: Mutex<ClockState>,
    parent: Option<Arc<QueueClock>>,
}

#[derive(Default)]
struct ClockState {
    queued: Duration,
    /// When the first of the current waiters joined the queue.
    since: Option<Instant>,
    waiters: usize,
}

impl QueueClock {
    fn nested() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
            parent: QUEUE_CLOCK.try_with(Arc::clone).ok(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ClockState> {
        #[allow(clippy::disallowed_methods)]
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn enter(&self, now: Instant) {
        {
            let mut state = self.lock();
            if state.waiters == 0 {
                state.since = Some(now);
            }
            state.waiters += 1;
        }
        if let Some(parent) = &self.parent {
            parent.enter(now);
        }
    }

    fn leave(&self, now: Instant) {
        {
            let mut state = self.lock();
            state.waiters = state.waiters.saturating_sub(1);
            if state.waiters == 0
                && let Some(since) = state.since.take()
            {
                state.queued += now.saturating_duration_since(since);
            }
        }
        if let Some(parent) = &self.parent {
            parent.leave(now);
        }
    }

    /// Time spent with at least one request in the queue, up to `now`.
    fn queued(&self, now: Instant) -> Duration {
        let state = self.lock();
        state.queued
            + state
                .since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }
}

/// The clock of the innermost [`timeout_excluding_queue`] around this task.
pub fn current_queue_clock() -> Option<Arc<QueueClock>> {
    QUEUE_CLOCK.try_with(Arc::clone).ok()
}

/// Run `fut` so that its slot waits record into `clock`.
pub async fn in_queue_clock<F: Future>(clock: Option<Arc<QueueClock>>, fut: F) -> F::Output {
    match clock {
        Some(clock) => QUEUE_CLOCK.scope(clock, fut).await,
        None => fut.await,
    }
}

/// A [`timeout_excluding_queue`] ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueAwareElapsed {
    /// The limit, which excludes the time in the queue.
    pub limit: Duration,
    /// The time the requests under the limit spent in the queue.
    pub queued: Duration,
}

impl std::fmt::Display for QueueAwareElapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "deadline of {:?} has elapsed ({:?} in the request queue not counted)",
            self.limit, self.queued
        )
    }
}

impl std::error::Error for QueueAwareElapsed {}

/// [`tokio::time::timeout`], except that time a model request under `fut`
/// spends waiting for a slot does not count against `limit`.
pub async fn timeout_excluding_queue<F: Future>(
    limit: Duration,
    fut: F,
) -> Result<F::Output, QueueAwareElapsed> {
    let clock = QueueClock::nested();
    let start = Instant::now();
    let fut = QUEUE_CLOCK.scope(Arc::clone(&clock), fut);
    tokio::pin!(fut);
    loop {
        let deadline = start + limit + clock.queued(Instant::now());
        tokio::select! {
            biased;
            out = &mut fut => return Ok(out),
            () = tokio::time::sleep_until(deadline) => {
                let now = Instant::now();
                let queued = clock.queued(now);
                if now >= start + limit + queued {
                    return Err(QueueAwareElapsed { limit, queued });
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "request_slots_tests.rs"]
mod tests;
