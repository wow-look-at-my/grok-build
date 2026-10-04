#![allow(clippy::cast_possible_wrap)]

//! Cross-platform system **sleep/wake** (suspend/resume) notifications.

#![deny(clippy::indexing_slicing)]

/// A system power transition.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// Because the idle-sleep negotiation can be vetoed (by any power client), a `WillSleep` is not a guarantee that sleep follows.
    WillSleep,
    /// The system resumed from sleep.
    DidWake,
}

/// Boxed user callback invoked on each [`PowerEvent`].
pub type PowerCallback = Box<dyn Fn(PowerEvent) + Send + Sync + 'static>;

/// A coarse, synchronously-queryable system power state (see
/// [`current_power_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// Full / user wake: display (graphics) capability present — a user is (or can be) present.
    FullWake,
    /// Dark wake: CPU (and usually network/disk) up for background or maintenance work, but display off and no user.
    DarkWake,
    /// State could not be determined: an unsupported OS, or the platform query failed / returned a transitional sample.
    Unknown,
}

/// Cheap, non-blocking, and never panics.
pub fn current_power_state() -> PowerState {
    imp::current_power_state()
}

/// Ask the OS not to sleep until the returned guard is dropped.
#[must_use = "the assertion is released as soon as the guard is dropped"]
pub fn hold_awake(reason: &str) -> Option<SleepAssertion> {
    imp::hold_awake(reason).map(|inner| SleepAssertion { _inner: inner })
}

/// RAII guard from [`hold_awake`]; releases the OS assertion on drop.
#[derive(Debug)]
pub struct SleepAssertion {
    _inner: imp::Assertion,
}

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod imp;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::PowerCallback;

    pub(crate) struct Listener;

    impl Listener {
        pub(crate) fn start(_callback: PowerCallback) -> Option<Self> {
            None
        }
    }

    pub(crate) fn current_power_state() -> super::PowerState {
        super::PowerState::Unknown
    }

    /// Never constructed (`hold_awake` always returns `None`).
    #[derive(Debug)]
    #[allow(dead_code)]
    pub(crate) struct Assertion;

    pub(crate) fn hold_awake(_reason: &str) -> Option<Assertion> {
        None
    }
}

/// On macOS/Windows, dropping it stops the listener and releases its OS
/// resources.
pub struct SystemPowerListener {
    // Kept for its `Drop`; the field is read on platforms with a real impl.
    #[allow(dead_code)]
    inner: imp::Listener,
}

impl SystemPowerListener {
    /// Callers should treat `None` as "no power notifications" and degrade
    /// gracefully (the dependent feature does not engage).
    pub fn start<F>(callback: F) -> Option<Self>
    where
        F: Fn(PowerEvent) + Send + Sync + 'static,
    {
        imp::Listener::start(Box::new(callback)).map(|inner| Self { inner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `start` + `drop` must be clean on every platform: no panic and no hang
    /// (the latter exercises the macOS run-loop teardown).
    #[test]
    fn start_and_drop_is_clean() {
        // Bind and let it drop at end of scope rather than calling `drop()`: on platforms where the listener owns no `Drop` type.
        let _listener = SystemPowerListener::start(|_event| {});
    }
}

#[cfg(all(test, target_os = "macos"))]
mod assertion_tests {
    /// Proves the FFI really registers with the OS, not merely that it links: take an assertion, look for it in `pmset -g
    /// assertions`, drop it, and confirm it went away. A wrong symbol or ABI would compile and silently protect nothing.
    #[test]
    fn hold_awake_registers_and_releases_a_real_assertion() {
        let name = format!("xai-system-power selftest {}", std::process::id());
        let listed = || -> String {
            std::process::Command::new("/usr/bin/pmset")
                .args(["-g", "assertions"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        if listed().is_empty() {
            return; // no pmset (sandboxed CI) — nothing to assert against
        }

        // The OS can refuse (sandboxed runners deny the powerd service);
        // refusal is a supported outcome, not a test failure.
        let Some(held) = super::hold_awake(&name) else {
            return;
        };
        assert!(
            listed().contains(&name),
            "assertion should be visible to the OS while held"
        );
        drop(held);
        assert!(
            !listed().contains(&name),
            "assertion must be released on drop, or the machine cannot sleep"
        );
    }
}
