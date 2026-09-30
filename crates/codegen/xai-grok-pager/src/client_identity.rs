pub const PAGER_CLIENT_TYPE: &str = "grok-pager";
pub const HEADLESS_CLIENT_TYPE: &str = "grok-shell";

/// A function, not a constant: the release number is written into the binary
/// after it links, so it is not a value the compiler holds.
pub fn pager_client_version() -> &'static str {
    xai_grok_version::version()
}

/// `User-Agent` for the pager's own HTTP clients that call `api.x.ai` directly (voice STT).
///
/// Matches the sampler's `grok-shell/<version> (os; arch)` shape so server-side dashboards bucket voice traffic alongside chat / imagine requests.
pub fn client_user_agent() -> String {
    format!(
        "{}/{} ({}; {})",
        HEADLESS_CLIENT_TYPE,
        pager_client_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_user_agent_has_expected_shape() {
        // e.g. "grok-shell/1.2.3 (macos; aarch64)".
        // Servers parse this UA string, so pin the exact shape
        let ua = client_user_agent();
        assert_eq!(
            ua,
            format!(
                "grok-shell/{} ({}; {})",
                pager_client_version(),
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
    }
}
