//! Origin/client identification used by the telemetry engine.

pub use xai_grok_sampler::OriginClientInfo;

/// Construct an [`OriginClientInfo`] from the `GROK_CLIENT_NAME` /
/// `GROK_CLIENT_VERSION` env vars. Returns `None` when `GROK_CLIENT_NAME` is
/// unset.
pub fn origin_client_info_from_env() -> Option<OriginClientInfo> {
    std::env::var("GROK_CLIENT_NAME")
        .ok()
        .map(|product| OriginClientInfo {
            product,
            version: std::env::var("GROK_CLIENT_VERSION").ok(),
        })
}
