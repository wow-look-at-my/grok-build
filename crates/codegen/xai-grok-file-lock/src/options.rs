use std::path::PathBuf;
use std::time::Duration;

/// How long a caller waits behind a held acquire slot before `AcquireInProgress`.
pub const DEFAULT_SLOT_GRACE: Duration = Duration::from_secs(2);

/// How long to wait for a contended target lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// One open+flock attempt; contention is `LockError::Contended`.
    NoWait,
    Poll {
        timeout: Duration,
        interval: Duration,
    },
}

/// Whether an attempt runs behind the machine-local acquire slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotPolicy {
    /// Default.
    Guarded { grace: Duration },
    /// As `Guarded`, rooted at an explicit directory (tests, diagnostics).
    GuardedIn { dir: PathBuf, grace: Duration },
    /// No slot. Windows always behaves as if this were set.
    Unguarded,
}

/// Options for `lock_file`. Fields are private so later additions stay non-breaking.
#[derive(Debug, Clone)]
pub struct LockOptions {
    wait: Wait,
    slot: SlotPolicy,
}

impl LockOptions {
    /// `Wait::NoWait` and `SlotPolicy::Guarded` with [`DEFAULT_SLOT_GRACE`].
    pub fn new() -> Self {
        LockOptions {
            wait: Wait::NoWait,
            slot: SlotPolicy::Guarded {
                grace: DEFAULT_SLOT_GRACE,
            },
        }
    }

    pub fn with_wait(mut self, wait: Wait) -> Self {
        self.wait = wait;
        self
    }

    /// Shorthand for `with_wait(Wait::Poll { timeout, interval })`.
    pub fn with_poll(self, timeout: Duration, interval: Duration) -> Self {
        self.with_wait(Wait::Poll { timeout, interval })
    }

    pub fn with_slot(mut self, slot: SlotPolicy) -> Self {
        self.slot = slot;
        self
    }

    pub fn wait(&self) -> Wait {
        self.wait
    }

    pub fn slot(&self) -> &SlotPolicy {
        &self.slot
    }
}

impl Default for LockOptions {
    fn default() -> Self {
        LockOptions::new()
    }
}
