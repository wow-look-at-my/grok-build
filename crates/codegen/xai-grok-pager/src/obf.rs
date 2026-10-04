//! Compile-time obfuscated string constants for binary hardening.

/// Authentication mechanism identifiers.
pub mod auth {
    /// The internal `cached_token` auth method ID used for reconnection.
    macro_rules! CACHED_TOKEN {
        () => {
            obfstr::obfstr!("cached_token")
        };
    }
    pub(crate) use CACHED_TOKEN;
}
