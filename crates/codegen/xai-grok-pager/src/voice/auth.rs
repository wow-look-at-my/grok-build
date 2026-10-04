//! Bridge the shell's `AuthManager` onto the voice crate's bearer provider.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use xai_grok_tools::types::SharedApiKeyProvider;
use xai_grok_tools::types::api_key_provider::SideCallBearerError;
use xai_grok_voice::{SharedVoiceAuth, VoiceAuthError, VoiceAuthProvider};

/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
struct AuthManagerVoiceAuth(SharedApiKeyProvider);

impl std::fmt::Debug for AuthManagerVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthManagerVoiceAuth")
    }
}

fn voice_auth_error(error: SideCallBearerError) -> VoiceAuthError {
    match error {
        SideCallBearerError::ForeignSession => VoiceAuthError::ForeignSession,
        SideCallBearerError::Missing => VoiceAuthError::NotSignedIn,
    }
}

impl VoiceAuthProvider for AuthManagerVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>> {
        let provider = self.0.clone();
        Box::pin(async move { provider.side_call_bearer().await.map_err(voice_auth_error) })
    }
}

/// Build the voice bearer provider from the connection's `AuthManager`.
/// Serves xAI logins and `XAI_API_KEY` / per-model BYOK keys.
pub fn build_voice_auth(auth_manager: Arc<xai_grok_login::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth(
        xai_grok_login::shared_api_key_provider(auth_manager),
    ))
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
