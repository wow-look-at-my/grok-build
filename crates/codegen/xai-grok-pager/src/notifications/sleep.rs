//! Holds the system awake as `[ui.notifications].keep_awake` asks.
//! A child process holds the lock: `caffeinate` on macOS, `systemd-inhibit` on Linux.
//!
//! Threading: this lives on `AppView`, which is single-threaded and `!Send`, so fields use `Cell` and `RefCell` rather than atomics or mutexes.

use std::cell::{Cell, RefCell};
use std::process::{Child, Command, Stdio};

use super::config::KeepAwake;

/// Holds the system awake while [`KeepAwake::wants_awake`] is true. Every call is idempotent.
/// On `Drop`, the lock holder is stopped.
pub struct SleepInhibitor {
    mode: Cell<KeepAwake>,
    child: RefCell<Option<Child>>,
    /// Set on the first failure, so a host without the tool does not spawn it again on every update.
    platform_unavailable: Cell<bool>,
    make_command: fn() -> Option<Command>,
}

impl SleepInhibitor {
    /// `Always` takes the lock here, before any session starts.
    pub fn new(mode: KeepAwake) -> Self {
        Self::with_command(mode, platform_command)
    }

    fn with_command(mode: KeepAwake, make_command: fn() -> Option<Command>) -> Self {
        let inhibitor = Self {
            mode: Cell::new(mode),
            child: RefCell::new(None),
            platform_unavailable: Cell::new(false),
            make_command,
        };
        inhibitor.sync(false);
        inhibitor
    }

    pub fn mode(&self) -> KeepAwake {
        self.mode.get()
    }

    /// Change the mode and apply it at once.
    pub fn set_mode(&self, mode: KeepAwake, running: bool) {
        self.mode.set(mode);
        self.sync(running);
    }

    /// Take or drop the lock. `running` is true while any session does work.
    pub fn sync(&self, running: bool) {
        if self.mode.get().wants_awake(running) {
            self.inhibit();
        } else {
            self.release();
        }
    }

    /// Whether a lock holder runs now.
    pub fn is_active(&self) -> bool {
        self.child.borrow().is_some()
    }

    fn inhibit(&self) {
        if self.platform_unavailable.get() {
            return;
        }
        let mut slot = self.child.borrow_mut();
        if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => {
                    tracing::warn!(%status, "keep-awake lock holder exited; the system can sleep");
                }
                Err(error) => {
                    tracing::warn!(%error, "keep-awake lock holder cannot be polled; the system can sleep");
                }
            }
            *slot = None;
            self.platform_unavailable.set(true);
            return;
        }
        let Some(mut cmd) = (self.make_command)() else {
            self.platform_unavailable.set(true);
            return;
        };
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        xai_tty_utils::detach_std_command(&mut cmd);
        // A crashed grok runs no Drop. On Linux this binds the holder to our death; `caffeinate -w` does that on macOS.
        xai_tty_utils::kill_on_parent_death_std(&mut cmd);
        #[allow(clippy::disallowed_methods)] // bound to our lifetime; stopped on release
        match cmd.spawn() {
            Ok(child) => *slot = Some(child),
            Err(error) => {
                tracing::warn!(
                    %error,
                    program = ?cmd.get_program(),
                    "keep-awake tool did not start; the system can sleep"
                );
                self.platform_unavailable.set(true);
            }
        }
    }

    fn release(&self) {
        let Some(mut child) = self.child.borrow_mut().take() else {
            return;
        };
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        #[cfg(not(unix))]
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for SleepInhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

/// `-i` stops idle system sleep. `-w` makes caffeinate exit when this process exits.
#[cfg(target_os = "macos")]
fn platform_command() -> Option<Command> {
    let mut cmd = Command::new("/usr/bin/caffeinate");
    cmd.args(["-i", "-w", &std::process::id().to_string()]);
    Some(cmd)
}

/// `systemd-inhibit` keeps the idle-inhibit fd and runs `sleep infinity` as its child.
#[cfg(target_os = "linux")]
fn platform_command() -> Option<Command> {
    let mut cmd = Command::new("systemd-inhibit");
    cmd.args([
        "--what=idle",
        "--who=grok",
        "--why=keep_awake",
        "sleep",
        "infinity",
    ]);
    Some(cmd)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_command() -> Option<Command> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A stand-in lock holder that every unix CI host has.
    fn sleeper() -> Option<Command> {
        let mut cmd = Command::new("sleep");
        cmd.arg("1000");
        Some(cmd)
    }

    fn missing_tool() -> Option<Command> {
        Some(Command::new("/nonexistent/keep-awake-tool"))
    }

    fn holder_pid(inhibitor: &SleepInhibitor) -> Option<u32> {
        inhibitor.child.borrow().as_ref().map(Child::id)
    }

    /// A reaped pid is gone; release waits on the child, so no zombie stays behind.
    fn process_alive(pid: u32) -> bool {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits i32"));
        nix::sys::signal::kill(pid, None).is_ok()
    }

    #[test]
    fn never_holds_nothing() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::Never, sleeper);
        inhibitor.sync(true);
        assert!(!inhibitor.is_active());
    }

    #[test]
    fn while_running_follows_running() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::WhileRunning, sleeper);
        assert!(!inhibitor.is_active(), "idle at start");
        inhibitor.sync(true);
        let pid = holder_pid(&inhibitor).expect("work runs, so a holder runs");
        inhibitor.sync(true);
        assert_eq!(
            holder_pid(&inhibitor),
            Some(pid),
            "a second sync spawns nothing"
        );
        inhibitor.sync(false);
        assert!(!inhibitor.is_active());
        assert!(!process_alive(pid), "release stops the holder");
    }

    #[test]
    fn always_holds_from_construction_and_while_idle() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::Always, sleeper);
        assert!(inhibitor.is_active(), "always takes the lock at start");
        inhibitor.sync(false);
        assert!(inhibitor.is_active(), "idle does not release it");
    }

    #[test]
    fn set_mode_applies_at_once() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::WhileRunning, sleeper);
        inhibitor.set_mode(KeepAwake::Always, false);
        assert!(inhibitor.is_active());
        inhibitor.set_mode(KeepAwake::Never, true);
        assert!(!inhibitor.is_active());
        assert_eq!(inhibitor.mode(), KeepAwake::Never);
    }

    #[test]
    fn drop_stops_the_holder() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::Always, sleeper);
        let pid = holder_pid(&inhibitor).expect("holder runs");
        drop(inhibitor);
        assert!(!process_alive(pid));
    }

    #[test]
    fn a_missing_tool_latches_and_is_not_spawned_again() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::Always, missing_tool);
        assert!(!inhibitor.is_active());
        assert!(inhibitor.platform_unavailable.get());
        inhibitor.sync(true);
        assert!(!inhibitor.is_active());
    }

    #[test]
    fn a_holder_that_exits_is_not_reported_as_held() {
        let inhibitor = SleepInhibitor::with_command(KeepAwake::Always, sleeper);
        let pid = holder_pid(&inhibitor).expect("holder runs");
        let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits i32"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL).expect("kill holder");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while inhibitor.is_active() && std::time::Instant::now() < deadline {
            inhibitor.sync(true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!inhibitor.is_active());
        assert!(inhibitor.platform_unavailable.get());
    }
}
