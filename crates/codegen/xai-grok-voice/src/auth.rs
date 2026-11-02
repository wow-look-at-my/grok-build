//! Bearer resolution for voice STT requests.
//! The voice clients are long-lived: a single voice session opens many STT WebSocket connections over its lifetime.
//! An OAuth/session bearer rotates (~15 min), so capturing a token once at startup would 401 mid-session.
//! Instead of a static `String`, the clients hold a [`SharedVoiceAuth`] and resolve a fresh bearer at the point of each connection.
//!
//! This crate stays dependency-light: it defines its own minimal async trait instead of the shell's `AuthManager` or tools' `ApiKeyProvider`.
//! The pager adapts the shell's refreshing provider onto this trait.

use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::Arc;

use xai_grok_net::{Auth, KeyScheme};

#[cfg(feature = "audio")]
use crate::error::VoiceError;

/// Why there is no bearer for `wss://api.x.ai/v1/stt`; neither variant is a cue to try another credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VoiceAuthError {
    #[error(
        "voice needs an xAI credential for this account: sign in with an xAI login or set XAI_API_KEY"
    )]
    ForeignSession,
    #[error("not signed in — run `grok login`, set XAI_API_KEY, or set a model api_key/env_key")]
    NotSignedIn,
}

pub trait VoiceAuthProvider: std::fmt::Debug + Send + Sync + 'static {
    /// The credential for the next STT connection.
    fn auth(&self) -> Pin<Box<dyn Future<Output = Result<Auth, VoiceAuthError>> + Send + '_>>;
}

/// Shared provider handed to the voice pipeline.
pub type SharedVoiceAuth = Arc<dyn VoiceAuthProvider>;

#[cfg(feature = "audio")]
pub(crate) async fn require_auth(auth: &SharedVoiceAuth) -> Result<Auth, VoiceError> {
    auth.auth()
        .await
        .map_err(|error| VoiceError::Auth(error.to_string()))
}

/// A fixed bearer that never refreshes.
/// Used by the standalone `voice-probe` binary and tests, where there is no `AuthManager`, only a raw `XAI_API_KEY`.
pub struct StaticVoiceAuth(pub String);

impl std::fmt::Debug for StaticVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("StaticVoiceAuth")
            .field(&"<redacted>")
            .finish()
    }
}

impl VoiceAuthProvider for StaticVoiceAuth {
    fn auth(&self) -> Pin<Box<dyn Future<Output = Result<Auth, VoiceAuthError>> + Send + '_>> {
        Box::pin(ready(Ok(Auth::ApiKey {
            key: self.0.clone(),
            scheme: KeyScheme::Bearer,
        })))
    }
}

impl StaticVoiceAuth {
    /// Build a [`SharedVoiceAuth`] from a static key, trimming whitespace and rejecting an empty value.
    pub fn shared(key: impl Into<String>) -> Option<SharedVoiceAuth> {
        let key = key.into().trim().to_string();
        if key.is_empty() {
            return None;
        }
        Some(Arc::new(Self(key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_resolves_as_an_api_key() {
        let provider = StaticVoiceAuth::shared("  sk-test  ").unwrap();
        match provider.auth().await {
            Ok(Auth::ApiKey { key, scheme }) => {
                assert_eq!(key, "sk-test");
                assert_eq!(scheme, KeyScheme::Bearer);
            }
            other => panic!("expected an API key, got {other:?}"),
        }
    }

    #[test]
    fn static_provider_rejects_empty() {
        assert!(StaticVoiceAuth::shared("   ").is_none());
    }
}
