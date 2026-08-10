pub(crate) mod api_key_probe;
pub(crate) mod attribution;
mod auth_provider;
mod config;
pub mod credential_provider;
#[path = "devbox_login_stub.rs"]
pub(crate) mod devbox_login;
pub(crate) mod device_code;
pub mod error;
mod external_auth;
mod flow;
mod jwt;
pub(crate) mod manager;
mod model;
pub mod oidc;
pub(crate) mod recovery;
pub(crate) mod refresh;
pub(crate) mod single_flight;
mod storage;
mod token_output;
pub(crate) mod token_type;
/// Pin jsonwebtoken to this crate's declared provider before the first JWT
/// operation in the process.
///
/// Auto-detection cannot be relied on here: `gcloud-storage` pulls
/// `jsonwebtoken/aws_lc_rs` while this crate asks for `rust_crypto`, and
/// cargo's feature unification leaves both on, which jsonwebtoken answers with
/// a provider whose every operation panics. The process default latches on
/// first use (`OnceLock::get_or_init`), so a test that reaches crypto without
/// installing first poisons every other test in the process — call this before
/// any JWT work. Production installs on the same grounds at login.
#[cfg(test)]
pub(crate) fn ensure_crypto_provider() {
    let _ = jsonwebtoken::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

pub(crate) use api_key_probe::{
    DEFAULT_PROBE_TIMEOUT, first_party_env_key_allows_advertise, should_probe_first_party_env_key,
};
pub use auth_provider::{AuthProviderConfig, AuthProviderRef};
pub(crate) use auth_provider::{
    PROVIDER_TIMEOUT_CEILING_SECS, PROVIDER_TOKEN_EXPIRY_SKEW_SECS, ProviderRefreshOutcome,
};
#[cfg(test)]
pub(crate) use auth_provider::{test_backdate_provider_mint, test_counting_provider};
pub(crate) use config::LEGACY_AUTH_SCOPE;
pub use config::{
    ForceLoginTeam, GrokComConfig, OAuth2ProviderConfig, OidcAuthConfig, PreferredAuthMethod,
    XAI_OAUTH2_ISSUER, is_xai_oauth2_issuer, xai_oauth2_issuer,
};
pub(crate) use external_auth::{parse_output, refresh_with_command};
pub(crate) use flow::{
    AuthChannels, mint_session_noninteractive, run_auth_flow, run_auth_flow_with_stderr_bridge,
    try_noninteractive_auth_no_mint,
};
pub use flow::{
    AuthUrlInfo, AuthUrlMode, LoginTransportOverride, LogoutResult, ensure_authenticated,
    ensure_authenticated_or_noninteractive, ensure_authenticated_with_override, perform_logout,
    run_cli_login, run_cli_logout, try_ensure_fresh_auth,
};
pub use jwt::{is_jwt_expired_or_near, parse_jwt_expiration};
mod meta;
pub use error::{AuthError, RefreshTokenError, RefreshTokenFailedReason};
pub use manager::{AuthManager, shared_api_key_provider};
pub(crate) use manager::{AuthRemedy, SilentRefresh};
pub use meta::{AuthMeta, GateInfo};
pub use model::{AuthMode, GrokAuth, lookup_auth};
pub(crate) use model::{TOKEN_TTL, UserInfo, default_coding_data_retention_opt_out, is_expired};
pub(crate) use refresh::DiagnosticUploader;
pub use storage::{
    clear_api_key, clear_openai_compatible_api_key, read_api_key, read_auth_json,
    read_openai_compatible_api_key, read_token_by_scope, store_api_key,
    store_openai_compatible_api_key,
};
