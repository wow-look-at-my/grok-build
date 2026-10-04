//! Checks the live subscription tier so the paywall gate can lift.
use std::sync::Arc;
use std::time::Duration;
use xai_grok_login::AuthManager;
use xai_grok_login::UserInfo;
use xai_grok_login::manager::{BEST_EFFORT_REFRESH_TIMEOUT, BoundedRefresh, RefreshReason};
use xai_grok_login::token_type::TokenType;
/// Any active subscription qualifies: the proxy only returns a tier when an
/// active subscription exists (`None` otherwise).
fn is_qualifying_tier(tier: &str) -> bool {
    !tier.is_empty() && tier != "Free"
}
/// Returned only when the check confirmed a qualifying tier.
pub(crate) struct UnblockResult {
    pub(crate) new_tier: String,
    /// The `userId` from the `/user` response that confirmed the tier.
    pub(crate) canonical_user_id: String,
    /// True when the best-effort refresh below hit its bounded deadline with the exchange still running.
    pub(crate) refresh_deadline_hit: bool,
}
async fn fetch_user_info(
    http_client: &reqwest::Client,
    url: &str,
    auth: &xai_grok_login::GrokAuth,
    auth_manager: &AuthManager,
    alpha_test_key: Option<&str>,
) -> Result<UserInfo, &'static str> {
    let request = http_client
        .get(url)
        .timeout(Duration::from_secs(10))
        .header("Authorization", format!("Bearer {}", auth.key))
        .header(
            "X-XAI-Token-Auth",
            auth_manager.grok_com_config().token_header.as_str(),
        )
        .header("x-grok-client-version", xai_grok_version::version())
        .header(
            crate::http::CLIENT_MODE_HEADER,
            crate::http::process_client_mode(),
        );
    let _ = alpha_test_key;
    match request.send().await {
        Ok(resp) if resp.status().is_success() => {
            resp.json::<UserInfo>().await.map_err(|_| "parse")
        }
        Ok(_resp) => Err("http_status"),
        Err(e) if e.is_timeout() => Err("timeout"),
        Err(_) => Err("transport"),
    }
}
/// Called by the pager every 5s while the paywall is shown (`x.ai/auth/check_subscription`).
/// Queries `/user?include=subscription` for the live tier.
/// On a qualifying tier it does a best-effort JWT refresh and returns `Some(UnblockResult)`.
#[tracing::instrument(name = "auth.paywall_check", skip_all, fields(user_id = %user_id))]
pub(crate) async fn single_check(
    auth_manager: Arc<AuthManager>,
    proxy_base_url: &str,
    alpha_test_key: Option<&str>,
    user_id: &str,
) -> Option<UnblockResult> {
    use xai_grok_login::backend::{ActiveAuthBackend, AuthBackend};
    if !ActiveAuthBackend::default().is_xai_authority() {
        return None;
    }
    let user_url = format!("{}/user?include=subscription", proxy_base_url);
    let http_client = crate::http::shared_client();
    let auth = auth_manager.current()?;
    let user_info = match fetch_user_info(
        &http_client,
        &user_url,
        &auth,
        &auth_manager,
        alpha_test_key,
    )
    .await
    {
        Ok(ui) => ui,
        Err(kind) => {
            xai_grok_telemetry::unified_log::warn(
                "paywall_check_error",
                None,
                Some(serde_json::json!({ "user_id": user_id, "kind": kind })),
            );
            return None;
        }
    };
    xai_grok_telemetry::unified_log::info(
        "paywall_check_result",
        None,
        Some(serde_json::json!({
            "user_id": user_id,
            "subscription_tier": user_info.subscription_tier,
        })),
    );
    let new_tier = match &user_info.subscription_tier {
        Some(tier) if !tier.is_empty() => tier.clone(),
        _ => return None,
    };
    if !is_qualifying_tier(&new_tier) {
        return None;
    }
    xai_grok_telemetry::unified_log::info(
        "paywall_check_subscription_detected",
        None,
        Some(serde_json::json!({
            "user_id": user_id,
            "new_tier": new_tier,
        })),
    );
    let refresh_deadline_hit = match auth_manager
        .refresh_chain_bounded_outcome(
            TokenType::OidcSession,
            RefreshReason::ServerRejected,
            BEST_EFFORT_REFRESH_TIMEOUT,
        )
        .await
    {
        BoundedRefresh::Resolved(result) => {
            if let Err(e) = *result {
                xai_grok_telemetry::unified_log::warn(
                    "paywall_check_error",
                    None,
                    Some(serde_json::json!({
                        "user_id": user_id,
                        "kind": "refresh_failed",
                        "detail": e.to_string(),
                    })),
                );
            }
            false
        }
        BoundedRefresh::DeadlineElapsed => {
            xai_grok_telemetry::unified_log::warn(
                "paywall_check_error",
                None,
                Some(serde_json::json!({
                    "user_id": user_id,
                    "kind": "refresh_deadline",
                    "detail": "bounded refresh deadline elapsed; mint continues in background",
                })),
            );
            true
        }
    };
    xai_grok_telemetry::unified_log::info(
        "paywall_check_unblocked",
        None,
        Some(serde_json::json!({ "user_id": user_id, "new_tier": new_tier })),
    );
    Some(UnblockResult {
        new_tier,
        canonical_user_id: user_info.user_id,
        refresh_deadline_hit,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_paid_tiers_qualify() {
        for tier in &[
            "SuperGrokPro",
            "SuperGrokPlus",
            "GrokPro",
            "SuperGrokLite",
            "XPremiumPlus",
            "XPremium",
            "XBasic",
        ] {
            assert!(is_qualifying_tier(tier), "{tier} must qualify");
        }
    }
    #[test]
    fn free_and_empty_tiers_are_not_qualifying() {
        assert!(!is_qualifying_tier("Free"));
        assert!(!is_qualifying_tier(""));
    }
}
