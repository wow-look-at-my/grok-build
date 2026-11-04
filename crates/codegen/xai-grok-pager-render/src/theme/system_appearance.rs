//! System appearance detection for automatic day/night theming.
//!
//! Detection chain (each step only runs when the previous returns nothing):
//! 1. `dark-light` desktop APIs: macOS `AppleInterfaceStyle`, Linux XDG portal `org.freedesktop.appearance.color-scheme`, Windows registry
//! 2. Explicit env stamps, `GROK_APPEARANCE` / `LC_GROK_APPEARANCE` (SSH with tmux, wrap, headless).
//!    See [`super::env_appearance`].
//! 3. OSC 11 terminal background query, **startup-only**; see [`detect_with_osc11_fallback`].
//!    The result is cached so runtime `detect()` / `resolve_auto` cannot be overwritten by stale `COLORFGBG`.
//! 4. Inherited `COLORFGBG` guess, the last resort.
//!
//! Detection returns `None` when every step fails.

use super::ThemeKind;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::watch;

/// Startup OSC 11 result (`Some`/`None` after a probe); unset until [`detect_with_osc11_fallback`] runs.
/// Runtime `detect` reuses it so a live OSC 11 polarity is not replaced by inherited `COLORFGBG`.
static OSC11_STARTUP: OnceLock<Option<SystemAppearance>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    Dark,
}

impl SystemAppearance {
    /// Canonical env-var payload (`dark` / `light`), ASCII so it is safe in `LC_*` values forwarded by OpenSSH.
    #[must_use]
    pub const fn as_env_value(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Same chain as startup, but OSC 11 reuses the cached result instead of probing. Tests replace the whole chain via the mock.
#[must_use]
pub fn detect() -> Option<SystemAppearance> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(v) = mock_override() {
        return v;
    }

    detect_without_mock()
}

/// Startup-only: OSC 11 needs raw-mode stdin and must not run once `EventStream` is active.
/// The watcher uses [`detect`] and still prefers a cached OSC 11 hit over `COLORFGBG`.
#[must_use]
pub fn detect_with_osc11_fallback() -> Option<SystemAppearance> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(v) = mock_override() {
        return v;
    }

    if let Some(appearance) = detect_desktop() {
        return Some(appearance);
    }
    let env = crate::host::collect_unicode_env();
    let explicit = super::env_appearance::detect_explicit_from_env_map(&env);
    if explicit.is_some() {
        return explicit;
    }
    let osc11 = super::osc11::detect_via_osc11();
    let _ = OSC11_STARTUP.set(osc11);
    osc11.or_else(|| super::env_appearance::detect_colorfgbg_from_env_map(&env))
}

fn resolve_appearance_chain(
    desktop: Option<SystemAppearance>,
    explicit: Option<SystemAppearance>,
    osc11: Option<SystemAppearance>,
    colorfgbg: Option<SystemAppearance>,
) -> Option<SystemAppearance> {
    desktop.or(explicit).or(osc11).or(colorfgbg)
}

/// Desktop APIs only. `grok wrap` stamps the local OS theme before SSH; env hints may be a previous hop's snapshot.
#[must_use]
#[cfg(any(target_os = "macos", windows))]
pub fn detect_desktop() -> Option<SystemAppearance> {
    match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => Some(SystemAppearance::Dark),
        Ok(dark_light::Mode::Light) => Some(SystemAppearance::Light),
        // Mode::Unspecified or Err: no system preference detected
        _ => None,
    }
}

/// Desktop APIs only. `grok wrap` stamps the local OS theme before SSH; env hints may be a previous hop's snapshot.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[must_use]
pub fn detect_desktop() -> Option<SystemAppearance> {
    xdg_portal::detect()
}

/// Desktop APIs only. This platform has none that the pager reads.
#[cfg(not(any(
    target_os = "macos",
    windows,
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
#[must_use]
pub fn detect_desktop() -> Option<SystemAppearance> {
    None
}

/// Reads `org.freedesktop.appearance` `color-scheme` from the XDG desktop portal.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod xdg_portal {
    use super::SystemAppearance;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    /// The budget dark-light gave the same portal read.
    const TIMEOUT: Duration = Duration::from_millis(25);

    /// True while a probe thread runs. A portal that never answers then holds one thread, not one thread per call.
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

    pub(super) fn detect() -> Option<SystemAppearance> {
        if IN_FLIGHT.swap(true, Ordering::AcqRel) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("xdg-appearance".into())
            .spawn(move || {
                let _ = tx.send(read_color_scheme());
                IN_FLIGHT.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            IN_FLIGHT.store(false, Ordering::Release);
            return None;
        }
        rx.recv_timeout(TIMEOUT).ok().flatten()
    }

    fn read_color_scheme() -> Option<SystemAppearance> {
        static CONN: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
        let conn = CONN
            .get_or_init(|| {
                zbus::blocking::connection::Builder::session()
                    .ok()?
                    .method_timeout(TIMEOUT)
                    .build()
                    .ok()
            })
            .as_ref()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Settings"),
                "Read",
                &("org.freedesktop.appearance", "color-scheme"),
            )
            .ok()?;
        let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
        from_portal_value(&value)
    }

    /// `Read` wraps the value in one more variant on some portal versions.
    fn from_portal_value(value: &zbus::zvariant::Value<'_>) -> Option<SystemAppearance> {
        if let zbus::zvariant::Value::Value(inner) = value {
            return from_portal_value(inner);
        }
        match u32::try_from(value).ok()? {
            1 => Some(SystemAppearance::Dark),
            2 => Some(SystemAppearance::Light),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use zbus::zvariant::Value;

        #[test]
        fn portal_value_maps_like_the_spec() {
            assert_eq!(
                from_portal_value(&Value::U32(1)),
                Some(SystemAppearance::Dark)
            );
            assert_eq!(
                from_portal_value(&Value::U32(2)),
                Some(SystemAppearance::Light)
            );
            assert_eq!(from_portal_value(&Value::U32(0)), None);
            assert_eq!(
                from_portal_value(&Value::Value(Box::new(Value::U32(1)))),
                Some(SystemAppearance::Dark)
            );
            assert_eq!(from_portal_value(&Value::Str("dark".into())), None);
        }
    }
}

fn detect_without_mock() -> Option<SystemAppearance> {
    let env = crate::host::collect_unicode_env();
    resolve_appearance_chain(
        detect_desktop(),
        super::env_appearance::detect_explicit_from_env_map(&env),
        OSC11_STARTUP.get().copied().flatten(),
        super::env_appearance::detect_colorfgbg_from_env_map(&env),
    )
}

/// Returns `Some(value)` when a mock is active, `None` when real detection should proceed.
#[cfg(any(test, feature = "test-support"))]
fn mock_override() -> Option<Option<SystemAppearance>> {
    *MOCK_APPEARANCE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `[ui].auto_dark_theme` / `auto_light_theme`; `None` defaults to `GrokNight` / `GrokDay`.
#[must_use]
pub fn to_theme_kind(
    appearance: SystemAppearance,
    dark_theme: Option<ThemeKind>,
    light_theme: Option<ThemeKind>,
) -> ThemeKind {
    match appearance {
        SystemAppearance::Light => light_theme.unwrap_or(ThemeKind::GrokDay),
        SystemAppearance::Dark => dark_theme.unwrap_or(ThemeKind::GrokNight),
    }
}

/// Polling interval for system appearance detection.
/// Test builds shorten it so polling tests complete quickly.
#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Polls via [`detect()`] only (no OSC 11) and never mutates `theme_cache::CURRENT` or `AUTO_MODE`.
pub struct SystemAppearanceWatcher {
    rx: watch::Receiver<Option<SystemAppearance>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl SystemAppearanceWatcher {
    /// Returns `None` when `is_auto` is false; the event loop then uses `std::future::pending()` so the `select!` branch never fires.
    pub fn start_if_auto(is_auto: bool) -> Option<Self> {
        if !is_auto {
            return None;
        }

        let initial = detect();
        let (tx, rx) = watch::channel(initial);
        let interval = POLL_INTERVAL;

        // The handle is not dropped: it is held as `_handle` and aborted in the
        // watcher's `Drop`. The poll loop's lifetime is owned by the watcher.
        #[allow(clippy::disallowed_methods)]
        let handle = xai_spawn::spawn(async move {
            let mut current = initial;
            loop {
                tokio::time::sleep(interval).await;
                let detected = detect();
                if detected != current {
                    current = detected;
                    let _ = tx.send(current);
                }
            }
        });

        Some(Self {
            rx,
            _handle: handle,
        })
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.rx.changed().await
    }

    #[must_use]
    pub fn current(&self) -> Option<SystemAppearance> {
        *self.rx.borrow()
    }
}

impl Drop for SystemAppearanceWatcher {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

// -- Test support ----------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

/// Mock override for `detect()` / `detect_with_osc11_fallback`.
/// When set, both skip desktop, env, and OSC 11 so tests can control the watcher loop.
#[cfg(any(test, feature = "test-support"))]
static MOCK_APPEARANCE: Mutex<Option<Option<SystemAppearance>>> = Mutex::new(None);

/// Override `detect()` for tests.
/// Set to `Some(value)` to mock a specific appearance, or `None` to mock detection failure.
#[cfg(any(test, feature = "test-support"))]
pub fn set_mock(value: Option<SystemAppearance>) {
    *MOCK_APPEARANCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
}

#[cfg(any(test, feature = "test-support"))]
pub fn clear_mock() {
    *MOCK_APPEARANCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::super::cache as theme_cache;
    use super::*;

    /// Caller must hold `theme_cache::test_lock()`.
    /// Parallel tests in `cache::tests` and `slash::commands::theme::tests` also mutate `MOCK_APPEARANCE` via `set_mock`/`clear_mock`.
    fn assert_mock_roundtrip(value: Option<SystemAppearance>) {
        set_mock(value);
        assert_eq!(detect(), value);
        clear_mock();
    }

    #[test]
    fn appearance_chain_osc11_wins_over_conflicting_colorfgbg() {
        assert_eq!(
            resolve_appearance_chain(
                None,
                None,
                Some(SystemAppearance::Light),
                Some(SystemAppearance::Dark),
            ),
            Some(SystemAppearance::Light)
        );
        assert_eq!(
            resolve_appearance_chain(
                None,
                None,
                Some(SystemAppearance::Dark),
                Some(SystemAppearance::Light),
            ),
            Some(SystemAppearance::Dark)
        );
    }

    #[test]
    fn appearance_chain_colorfgbg_used_when_osc11_absent() {
        assert_eq!(
            resolve_appearance_chain(None, None, None, Some(SystemAppearance::Dark)),
            Some(SystemAppearance::Dark)
        );
    }

    #[test]
    fn appearance_chain_explicit_and_desktop_beat_osc11() {
        assert_eq!(
            resolve_appearance_chain(
                Some(SystemAppearance::Dark),
                Some(SystemAppearance::Light),
                Some(SystemAppearance::Light),
                Some(SystemAppearance::Light),
            ),
            Some(SystemAppearance::Dark)
        );
        assert_eq!(
            resolve_appearance_chain(
                None,
                Some(SystemAppearance::Dark),
                Some(SystemAppearance::Light),
                Some(SystemAppearance::Light),
            ),
            Some(SystemAppearance::Dark)
        );
    }

    #[test]
    fn to_theme_kind_dark_defaults_to_groknight() {
        let result = to_theme_kind(SystemAppearance::Dark, None, None);
        assert_eq!(result, ThemeKind::GrokNight);
    }

    #[test]
    fn to_theme_kind_light_defaults_to_grokday() {
        let result = to_theme_kind(SystemAppearance::Light, None, None);
        assert_eq!(result, ThemeKind::GrokDay);
    }

    #[test]
    fn to_theme_kind_custom_dark_theme() {
        let result = to_theme_kind(SystemAppearance::Dark, Some(ThemeKind::TokyoNight), None);
        assert_eq!(result, ThemeKind::TokyoNight);
    }

    #[test]
    fn to_theme_kind_custom_light_theme() {
        let result = to_theme_kind(SystemAppearance::Light, None, Some(ThemeKind::RosePineMoon));
        assert_eq!(result, ThemeKind::RosePineMoon);
    }

    #[test]
    fn to_theme_kind_custom_both() {
        let result = to_theme_kind(
            SystemAppearance::Dark,
            Some(ThemeKind::RosePineMoon),
            Some(ThemeKind::GrokNight),
        );
        assert_eq!(result, ThemeKind::RosePineMoon);

        let result = to_theme_kind(
            SystemAppearance::Light,
            Some(ThemeKind::RosePineMoon),
            Some(ThemeKind::GrokNight),
        );
        assert_eq!(result, ThemeKind::GrokNight);
    }

    #[test]
    fn to_theme_kind_dark_ignores_light_override() {
        let result = to_theme_kind(SystemAppearance::Dark, None, Some(ThemeKind::TokyoNight));
        assert_eq!(result, ThemeKind::GrokNight);
    }

    #[test]
    fn to_theme_kind_light_ignores_dark_override() {
        let result = to_theme_kind(SystemAppearance::Light, Some(ThemeKind::TokyoNight), None);
        assert_eq!(result, ThemeKind::GrokDay);
    }

    #[test]
    fn mock_dark_appearance() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_mock_roundtrip(Some(SystemAppearance::Dark));
    }

    #[test]
    fn mock_light_appearance() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_mock_roundtrip(Some(SystemAppearance::Light));
    }

    #[test]
    fn mock_detection_failure() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_mock_roundtrip(None);
    }
    // -- SystemAppearanceWatcher -----------------------------------------

    #[tokio::test]
    async fn start_if_auto_returns_none_when_not_auto() {
        assert!(SystemAppearanceWatcher::start_if_auto(false).is_none());
    }

    #[tokio::test]
    async fn start_if_auto_returns_some_when_auto() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(Some(SystemAppearance::Dark));
        let watcher = SystemAppearanceWatcher::start_if_auto(true);
        assert!(watcher.is_some());
        clear_mock();
    }

    #[tokio::test]
    async fn watcher_reports_initial_appearance() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(Some(SystemAppearance::Light));
        let watcher = SystemAppearanceWatcher::start_if_auto(true).unwrap();
        assert_eq!(watcher.current(), Some(SystemAppearance::Light));
        clear_mock();
    }

    #[tokio::test]
    async fn watcher_reports_none_on_detection_failure() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(None);
        let watcher = SystemAppearanceWatcher::start_if_auto(true).unwrap();
        assert_eq!(watcher.current(), None);
        clear_mock();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Deliberate: theme_cache::test_lock() serializes mock access.
    async fn watcher_detects_appearance_change() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(Some(SystemAppearance::Dark));
        let mut watcher = SystemAppearanceWatcher::start_if_auto(true).unwrap();
        assert_eq!(watcher.current(), Some(SystemAppearance::Dark));

        set_mock(Some(SystemAppearance::Light));

        // Wait for the watcher to detect the change (polls every 50ms in tests).
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("timed out waiting for change")
            .expect("watcher channel closed");

        assert_eq!(watcher.current(), Some(SystemAppearance::Light));
        clear_mock();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Deliberate: theme_cache::test_lock() serializes mock access.
    async fn watcher_does_not_send_when_unchanged() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(Some(SystemAppearance::Dark));
        let mut watcher = SystemAppearanceWatcher::start_if_auto(true).unwrap();

        // Wait longer than the poll interval
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(200), watcher.changed()).await;

        assert!(
            result.is_err(),
            "expected timeout — no change should be emitted"
        );
        assert_eq!(watcher.current(), Some(SystemAppearance::Dark));
        clear_mock();
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Deliberate: theme_cache::test_lock() serializes mock access.
    async fn watcher_detects_recovery_from_failure() {
        let _guard = theme_cache::test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_mock(None); // Initially detection fails
        let mut watcher = SystemAppearanceWatcher::start_if_auto(true).unwrap();
        assert_eq!(watcher.current(), None);

        // Now detection succeeds.
        set_mock(Some(SystemAppearance::Dark));

        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("timed out waiting for recovery")
            .expect("watcher channel closed");

        assert_eq!(watcher.current(), Some(SystemAppearance::Dark));
        clear_mock();
    }
}
