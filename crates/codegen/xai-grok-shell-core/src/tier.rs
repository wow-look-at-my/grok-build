//! Subscription-tier classification shared across the shell and the pager.

/// Whether a **known** subscription-tier display name is a gated tier: the
/// free tier or X Basic.
pub fn is_restricted_tier_name(tier: &str) -> bool {
    let t = tier.trim().to_ascii_lowercase();
    t.is_empty() || t == "free" || t == "x basic" || t == "x_basic"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_names() {
        assert!(is_restricted_tier_name(""));
        assert!(is_restricted_tier_name("   "));
        assert!(is_restricted_tier_name("Free"));
        assert!(is_restricted_tier_name("free"));
        assert!(is_restricted_tier_name("X Basic"));
        assert!(is_restricted_tier_name("x_basic"));
        assert!(is_restricted_tier_name("  X BASIC  "));
    }

    #[test]
    fn unrestricted_names() {
        assert!(!is_restricted_tier_name("SuperGrok"));
        assert!(!is_restricted_tier_name("SuperGrok Heavy"));
        assert!(!is_restricted_tier_name("supergrok_lite"));
        assert!(!is_restricted_tier_name("X Premium"));
        assert!(!is_restricted_tier_name("x_premium_plus"));
        // API keys are not free-tier gated.
        assert!(!is_restricted_tier_name("api_key"));
        assert!(!is_restricted_tier_name("API Key"));
        // Unknown future tiers fail open.
        assert!(!is_restricted_tier_name("some_new_plan"));
    }
}
