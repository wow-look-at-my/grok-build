//! Free-to-paid subscription detection and gate imposition/lift.
//!
//! All gate transitions go through [`AppView::impose_gate`] / [`AppView::lift_gate`].
//! That keeps the defer-vs-show decision and the lift bookkeeping (focus, telemetry, JWT-refresh check) in one place.
//!
//! Design constraints that are not obvious from the code:
//! - Gates from cached auth meta, prefetched settings, or settings pushes can be stale: the user may have subscribed since the snapshot was computed.
//!   Painting such a gate directly flashes a paywall at a paying user, so it is held in `pending_gate_verification` while a live check runs.
//!   On check failure or timeout we err on blocking.
//! - Timer effects have no cancellation, so verifications are stamped with `gate_verify_gen`.
//!   Results and timeouts from superseded deferrals are ignored by generation mismatch.

use super::actions::Effect;
use super::app_view::{AppView, AuthState};

/// Default watch cadence.
/// Overridable via the remote settings `grok_build_settings.subscription_watch_interval_secs` field.
pub(crate) const SUBSCRIPTION_WATCH_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60);

/// Floor for the server-supplied cadence: a fat-fingered remote settings value must not make the whole fleet poll rapidly.
/// `0` means "disabled" and is special-cased before this clamp.
pub(crate) const SUBSCRIPTION_WATCH_MIN_INTERVAL_SECS: u64 = 30;

/// Floor for the `GROK_SUBSCRIPTION_WATCH_INTERVAL_SECS` env override (for tests and power users; deliberately below the server floor).
const SUBSCRIPTION_WATCH_ENV_MIN_SECS: u64 = 1;

/// Cap on the spacing between watch/focus-triggered checks.
pub(crate) const SUBSCRIPTION_CHECK_DEBOUNCE: std::time::Duration =
    std::time::Duration::from_secs(30);

/// How long a deferred gate is held before being shown anyway.
/// This is a safety net for a hung ACP round-trip only; a completed check (even a failed one) resolves the deferral immediately.
/// Generous on purpose: the check can chain a `/user` fetch, a JWT refresh, and a settings re-fetch.
pub(crate) const GATE_VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How many times a verifying check asks the shell before its answer without meta promotes the held gate.
pub(crate) const GATE_VERIFY_CHECK_ATTEMPTS: u32 = 4;

/// The pause between those asks.
pub(crate) const GATE_VERIFY_CHECK_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(250);

/// True when a verifying check must ask again instead of promoting the gate
/// it holds.
pub(crate) fn verify_check_retry(
    verify: Option<u64>,
    meta: Option<&serde_json::Value>,
    attempt: u32,
) -> bool {
    verify.is_some() && meta.is_none() && attempt < GATE_VERIFY_CHECK_ATTEMPTS
}

impl AppView {
    /// Consumer xAI session auth: not an API key, not an enterprise team.
    /// Subscription gates and the watch only apply to these sessions.
    fn is_consumer_session(&self) -> bool {
        matches!(self.auth_state, AuthState::Done)
            && !self.is_api_key_auth
            && self.team_name.is_none()
    }

    /// `None` tier counts as potentially free so detection works before the first auth meta lands.
    /// A `true` result does not confirm the tier is free.
    pub fn may_be_free_tier(&self) -> bool {
        match self.subscription_tier.as_deref() {
            Some(t) => t.trim().eq_ignore_ascii_case("free"),
            None => true,
        }
    }

    /// Effective watch cadence; `None` means disabled.
    /// Precedence: env override (`0` disables), server override (`0` disables, floor-clamped), default.
    pub fn subscription_watch_interval(&self) -> Option<std::time::Duration> {
        if let Ok(v) = std::env::var("GROK_SUBSCRIPTION_WATCH_INTERVAL_SECS")
            && let Ok(secs) = v.trim().parse::<u64>()
        {
            return match secs {
                0 => None,
                s => Some(std::time::Duration::from_secs(
                    s.max(SUBSCRIPTION_WATCH_ENV_MIN_SECS),
                )),
            };
        }
        match self.subscription_watch_interval_secs {
            Some(0) => None,
            Some(secs) => Some(std::time::Duration::from_secs(
                secs.max(SUBSCRIPTION_WATCH_MIN_INTERVAL_SECS),
            )),
            None => Some(SUBSCRIPTION_WATCH_INTERVAL),
        }
    }

    /// Whether the watch (and the refocus check) should run: enabled, consumer session, and gated or possibly-free.
    pub fn subscription_watch_wanted(&self) -> bool {
        self.subscription_watch_interval().is_some()
            && self.is_consumer_session()
            && (self.gate.is_some() || self.may_be_free_tier())
    }

    /// The debounce is half the effective interval, capped at [`SUBSCRIPTION_CHECK_DEBOUNCE`].
    /// Scaling keeps the debounce from swallowing watch ticks when the cadence is tightened.
    fn subscription_check_allowed(&self) -> bool {
        let debounce = self
            .subscription_watch_interval()
            .map(|iv| (iv / 2).min(SUBSCRIPTION_CHECK_DEBOUNCE))
            .unwrap_or(SUBSCRIPTION_CHECK_DEBOUNCE);
        self.last_subscription_check_at
            .is_none_or(|t| t.elapsed() >= debounce)
    }

    fn note_subscription_check(&mut self) {
        self.last_subscription_check_at = Some(std::time::Instant::now());
    }

    /// The watch tick and the terminal-refocus trigger both fire their check through this one guard.
    /// Returns no effects when the watch is unwanted or the check is debounced.
    /// `trigger` tags the unified-log entry (`"watch"` / `"focus"`) so the check cadence is reconstructable from logs.
    #[must_use]
    pub fn fire_subscription_check(&mut self, trigger: &'static str) -> Vec<Effect> {
        if self.subscription_watch_wanted() && self.subscription_check_allowed() {
            self.note_subscription_check();
            crate::unified_log::info(
                "subscription.check.fired",
                None,
                Some(serde_json::json!({
                    "trigger": trigger,
                    "interval_secs": self
                        .subscription_watch_interval()
                        .map(|iv| iv.as_secs()),
                    "gated": self.gate.is_some(),
                    "tier": self.subscription_tier,
                })),
            );
            vec![Effect::CheckSubscription { verify: None }]
        } else {
            vec![]
        }
    }

    /// Chokepoint for a gate from settings. This build has no paywall, so a gate is never shown.
    /// It is logged and dropped.
    #[must_use]
    pub fn impose_gate(&mut self, gate: xai_grok_login::GateInfo) -> Vec<Effect> {
        crate::unified_log::info(
            "subscription.gate.ignored",
            None,
            Some(serde_json::json!({ "message": gate.message })),
        );
        vec![]
    }

    /// Chokepoint for lifting a gate once settings confirm the subscription.
    /// Clears the visible gate and any pending deferral; when either existed, runs the lift bookkeeping and returns the JWT-refresh check.
    /// The tier claim is baked into the JWT, so the shell must re-mint it.
    #[must_use]
    pub fn lift_gate(&mut self) -> Vec<Effect> {
        let was_blocked = self.gate.is_some() || self.pending_gate_verification.is_some();
        self.gate = None;
        self.pending_gate_verification = None;
        if !was_blocked {
            return vec![];
        }
        self.welcome_prompt_focused = true;
        self.paywall_check_started = None;
        crate::unified_log::info(
            "subscription.gate.lifted",
            None,
            Some(serde_json::json!({ "tier": self.subscription_tier })),
        );
        xai_grok_telemetry::session_ctx::log_event(
            xai_grok_telemetry::events::SubscriptionActivated {
                auth_method: self.login_method_id.as_ref().map(|id| id.0.to_string()),
                upsell_shown_this_session: self.access_gate_shown_logged,
            },
        );
        vec![Effect::CheckSubscription { verify: None }]
    }

    /// Ends the pending verification of `generation`. This build has no paywall, so nothing is shown.
    pub(crate) fn promote_deferred_gate(&mut self, generation: u64, _reason: &'static str) {
        if generation == self.gate_verify_gen {
            self.pending_gate_verification = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::app_view::tests::test_app;

    fn watch_gate() -> xai_grok_login::GateInfo {
        xai_grok_login::GateInfo {
            message: "Subscribe".into(),
            url: None,
            label: None,
        }
    }

    #[test]
    fn may_be_free_tier_matrix() {
        let mut app = test_app();
        app.subscription_tier = None;
        assert!(app.may_be_free_tier(), "unknown tier is potentially free");
        app.subscription_tier = Some("Free".into());
        assert!(app.may_be_free_tier());
        app.subscription_tier = Some(" FREE ".into());
        assert!(app.may_be_free_tier(), "case/whitespace-insensitive");
        app.subscription_tier = Some("SuperGrok Heavy".into());
        assert!(!app.may_be_free_tier());
        app.subscription_tier = Some("X Premium".into());
        assert!(!app.may_be_free_tier());
    }

    #[test]
    fn subscription_watch_wanted_matrix() {
        let mut app = test_app(); // AuthState::Done, consumer, tier unknown
        assert!(
            app.subscription_watch_wanted(),
            "unknown-tier consumer session watches"
        );

        app.subscription_tier = Some("Free".into());
        assert!(app.subscription_watch_wanted(), "free tier watches");

        app.subscription_tier = Some("SuperGrok".into());
        assert!(!app.subscription_watch_wanted(), "paid tier is dormant");

        // Gated: watches regardless of the (stale) tier string
        app.gate = Some(watch_gate());
        assert!(app.subscription_watch_wanted(), "gated session watches");
        app.gate = None;

        app.subscription_tier = Some("Free".into());
        app.is_api_key_auth = true;
        assert!(
            !app.subscription_watch_wanted(),
            "API-key auth never watches"
        );
        app.is_api_key_auth = false;
        app.team_name = Some("Acme Corp".into());
        assert!(
            !app.subscription_watch_wanted(),
            "team session never watches"
        );
        app.team_name = None;

        app.auth_state = AuthState::Pending { error: None };
        assert!(!app.subscription_watch_wanted(), "pre-auth never watches");
    }

    #[test]
    fn subscription_watch_interval_override_clamp_and_disable() {
        let mut app = test_app();
        assert_eq!(
            app.subscription_watch_interval(),
            Some(SUBSCRIPTION_WATCH_INTERVAL)
        );
        app.subscription_watch_interval_secs = Some(120);
        assert_eq!(
            app.subscription_watch_interval(),
            Some(std::time::Duration::from_secs(120))
        );
        app.subscription_watch_interval_secs = Some(1);
        assert_eq!(
            app.subscription_watch_interval(),
            Some(std::time::Duration::from_secs(
                SUBSCRIPTION_WATCH_MIN_INTERVAL_SECS
            )),
            "sub-floor values are clamped"
        );
        app.subscription_watch_interval_secs = Some(0);
        assert_eq!(app.subscription_watch_interval(), None);
        app.subscription_tier = Some("Free".into());
        assert!(
            !app.subscription_watch_wanted(),
            "interval 0 must disable the watch even on the free tier"
        );
    }

    #[test]
    fn subscription_check_debounce() {
        let mut app = test_app();
        assert!(app.subscription_check_allowed(), "no prior check — allowed");

        app.note_subscription_check();
        assert!(
            !app.subscription_check_allowed(),
            "right after a check — debounced"
        );

        app.last_subscription_check_at =
            Some(std::time::Instant::now() - SUBSCRIPTION_CHECK_DEBOUNCE);
        assert!(app.subscription_check_allowed());
    }

    #[test]
    fn fire_subscription_check_guards_and_debounces() {
        let mut app = test_app();
        let effs = app.fire_subscription_check("watch");
        assert!(matches!(
            effs.as_slice(),
            [Effect::CheckSubscription { verify: None }]
        ));
        assert!(
            app.fire_subscription_check("watch").is_empty(),
            "second fire inside the debounce window must be empty"
        );

        let mut paid = test_app();
        paid.subscription_tier = Some("SuperGrok".into());
        assert!(
            paid.fire_subscription_check("watch").is_empty(),
            "paid tier never fires"
        );
    }

    /// This build has no paywall: a gate is dropped for every kind of session.
    #[test]
    fn impose_gate_never_shows_a_gate() {
        let mut consumer = test_app();
        assert!(consumer.impose_gate(watch_gate()).is_empty());
        assert!(consumer.has_access());
        assert!(consumer.pending_gate_verification.is_none());

        let mut team = test_app();
        team.team_name = Some("Acme Corp".into());
        assert!(team.impose_gate(watch_gate()).is_empty());
        assert!(team.has_access());

        let mut api_key = test_app();
        api_key.is_api_key_auth = true;
        assert!(api_key.impose_gate(watch_gate()).is_empty());
        assert!(api_key.has_access());
    }

    #[test]
    fn a_gate_in_the_auth_meta_is_dropped() {
        let mut app = test_app();
        app.apply_auth_meta(&xai_grok_login::AuthMeta {
            gate: Some(watch_gate()),
            ..xai_grok_login::AuthMeta::default()
        });
        assert!(app.has_access());
    }

    #[test]
    fn lift_gate_runs_bookkeeping_once() {
        let mut app = test_app();
        app.gate = Some(watch_gate());
        app.paywall_check_started = Some(std::time::Instant::now());

        let effs = app.lift_gate();
        assert!(app.has_access());
        assert!(app.welcome_prompt_focused);
        assert!(app.paywall_check_started.is_none());
        assert!(matches!(
            effs.as_slice(),
            [Effect::CheckSubscription { verify: None }]
        ));

        assert!(
            app.lift_gate().is_empty(),
            "lift without a gate or deferral is a no-op"
        );
    }

    #[test]
    fn promote_deferred_gate_shows_nothing() {
        let mut app = test_app();
        app.pending_gate_verification = Some(watch_gate());
        app.promote_deferred_gate(app.gate_verify_gen, "verify_timeout");
        assert!(app.has_access());
        assert!(app.pending_gate_verification.is_none());
    }

    #[test]
    fn apply_auth_meta_drops_pending_gate_verification() {
        let mut app = test_app();
        let _effs = app.impose_gate(watch_gate());

        app.apply_auth_meta(&xai_grok_login::AuthMeta::default());

        assert!(app.pending_gate_verification.is_none());
        assert!(app.has_access());
    }

    #[test]
    fn a_verifying_check_retries_an_answer_without_meta() {
        let meta = serde_json::json!({ "email": "user@test.invalid" });
        // A verification that got meta is answered: it never retries.
        assert!(!verify_check_retry(Some(3), Some(&meta), 1));
        // Neither does one that ran out of attempts: the gate is promoted then.
        assert!(!verify_check_retry(
            Some(3),
            None,
            GATE_VERIFY_CHECK_ATTEMPTS
        ));
        // A check that holds no gate (watch, focus, paywall chain) asks once.
        assert!(!verify_check_retry(None, None, 1));
        // An answer without meta from a verification is no evidence, so it asks again.
        for attempt in 1..GATE_VERIFY_CHECK_ATTEMPTS {
            assert!(
                verify_check_retry(Some(3), None, attempt),
                "attempt {attempt}"
            );
        }
    }
}
