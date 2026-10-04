//! `std::thread::sleep` asserts when `nanosleep` returns a non-`EINTR` errno.

use std::time::{Duration, Instant};

pub(super) fn wait_full_interval(interval: Duration) {
    let started = Instant::now();
    loop {
        let remaining = interval.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return;
        }
        std::thread::park_timeout(remaining);
    }
}
