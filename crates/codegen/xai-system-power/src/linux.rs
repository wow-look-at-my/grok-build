//! Linux system sleep/wake via systemd-logind's `PrepareForSleep` D-Bus signal, with a `delay` inhibitor lock so we get a short window.

use std::thread;

use super::{PowerCallback, PowerEvent};

const DEST: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const IFACE: &str = "org.freedesktop.login1.Manager";

/// There is intentionally no clean stop.
pub(crate) struct Listener;

impl Listener {
    pub(crate) fn start(callback: PowerCallback) -> Option<Self> {
        // Probe synchronously so registration failures return `None` to the caller rather than dying silently.
        let conn = zbus::blocking::Connection::system().ok()?;
        let proxy = zbus::blocking::Proxy::new(&conn, DEST, PATH, IFACE).ok()?;
        let signals = proxy.receive_signal("PrepareForSleep").ok()?;

        thread::Builder::new()
            .name("xai-power-listener".into())
            .spawn(move || run_thread(proxy, signals, callback))
            .ok()?;

        Some(Self)
    }
}

/// Take a `delay` sleep inhibitor: logind holds off sleep until the returned fd
/// drops. `None` if unavailable — we then react without a pre-sleep window.
fn take_inhibitor(proxy: &zbus::blocking::Proxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
    proxy
        .call(
            "Inhibit",
            &("sleep", "grok", "Pause token refresh across sleep", "delay"),
        )
        .ok()
}

fn run_thread(
    proxy: zbus::blocking::Proxy<'static>,
    signals: zbus::blocking::proxy::SignalIterator<'static>,
    callback: PowerCallback,
) {
    // Hold the delay lock so the first PrepareForSleep(true) gives us a window.
    let mut inhibitor = take_inhibitor(&proxy);

    for msg in signals {
        let Ok(about_to_sleep) = msg.body().deserialize::<bool>() else {
            continue;
        };
        if about_to_sleep {
            // The callback may block (bounded) waiting for an in-flight token refresh to finish; the `delay` inhibitor is still held across it.
            callback(PowerEvent::WillSleep);
            inhibitor = None;
        } else {
            callback(PowerEvent::DidWake);
            // Re-acquire the delay lock for the next sleep cycle.
            inhibitor = take_inhibitor(&proxy);
        }
    }

    drop(inhibitor);
}

pub(crate) fn current_power_state() -> crate::PowerState {
    // Linux has no "dark wake" equivalent to query (the system is either suspended or fully awake).
    crate::PowerState::Unknown
}

/// No power-assertion support on this platform: callers carry on unprotected.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct Assertion;

pub(crate) fn hold_awake(_reason: &str) -> Option<Assertion> {
    None
}
