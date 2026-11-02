//! Telemetry identity: the subscription tier and the client re-init after an auth change.
use crate::agent::config::Config as AgentConfig;
use xai_grok_login::AuthManager;
/// Extract the numeric `tier` claim from a JWT access token (no signature verification).
/// Maps subscription-tier enum values to display strings the telemetry crate canonicalizes.
pub fn jwt_tier_claim(jwt: &str) -> Option<String> {
    use base64::Engine;
    let payload_b64 = jwt.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let tier = claims.get("tier")?.as_u64()?;
    Some(
        match tier {
            1 => "supergrok",
            2 => "x_basic",
            3 => "x_premium",
            4 => "x_premium_plus",
            5 => "supergrok_heavy",
            6 => "supergrok_lite",
            7 => "supergrok_plus",
            0 => "free",
            _ => return Some(tier.to_string()),
        }
        .to_string(),
    )
}
/// Resolve telemetry / AuthMeta `subscription_tier`.
/// Precedence: CCP `/settings` `subscription_tier_display` (when present and non-empty) [`AuthMode::ApiKey`] resolves to `"api_key"` (never free)
/// JWT `tier` claim via [`jwt_tier_claim`] (OAuth free resolves to `"free"`)
pub fn resolve_subscription_tier_for_telemetry(
    display: Option<String>,
    auth: Option<&xai_grok_login::GrokAuth>,
) -> Option<String> {
    if let Some(t) = display.filter(|s| !s.trim().is_empty()) {
        return Some(t);
    }
    let auth = auth?;
    if auth.auth_mode == xai_grok_login::AuthMode::ApiKey {
        return Some("api_key".into());
    }
    jwt_tier_claim(&auth.key)
}
/// Apply current telemetry config + auth identity. Tears down the client
/// when telemetry is disabled, so it's safe to call repeatedly.
pub fn update_telemetry_config(config: &AgentConfig, auth_manager: &AuthManager) {
    let user_agent = crate::http::process_user_agent_string();
    if xai_grok_net::header::HeaderValue::from_str(&user_agent).is_err() {
        tracing::warn!("telemetry init skipped: GROK_CLIENT_NAME yields an invalid user agent");
        return;
    }
    let grok_auth = auth_manager.current().filter(|a| a.is_xai_auth());
    let user_id = grok_auth.as_ref().map(|a| a.user_id.clone());
    let team_id = grok_auth.as_ref().and_then(|a| a.team_id.clone());
    let subscription_tier = resolve_subscription_tier_for_telemetry(
        config
            .remote_settings
            .as_ref()
            .and_then(|rs| rs.subscription_tier_display.clone()),
        auth_manager.current_or_expired().as_ref(),
    );
    xai_grok_telemetry::client::init(
        config.telemetry.clone(),
        config.resolve_telemetry_mode().value,
        user_id,
        team_id,
        crate::http::origin_client_info_from_env(),
        xai_grok_version::version().to_owned(),
        subscription_tier,
        crate::http::shared_client(),
    );
}
