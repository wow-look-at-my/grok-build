//! Bridge the shell's `AuthManager` onto the voice crate's STT routes.
//!
//! voice-api accepts an xAI API key or an xAI OAuth2 token at `api.example.invalid` and attributes per-user billing for OAuth.
//! The bearer comes from the shell's side-call resolver, the same one the Imagine tools use, so a login issued by a
//! foreign authority is refused here and no socket opens for it. A build that can transcribe a finished clip through
//! that login's own backend offers it as the second route, so such a login records and transcribes instead of failing.
//!
//! Resolved per request: the agent's refreshing manager in direct-spawn mode.
//! In leader mode, a non-refreshing one adopts the agent's rotated `auth.json` token under the file lock (see [`crate::acp`]).
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use xai_grok_tools::types::SharedApiKeyProvider;
use xai_grok_tools::types::api_key_provider::SideCallBearerError;
<<<<<<< HEAD
use xai_grok_voice::{Auth, KeyScheme, SharedVoiceAuth, VoiceAuthError, VoiceAuthProvider};

/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
///
/// Resolves a token per request (never a static snapshot), so a long session follows the `AuthManager` instead of pinning a token that 401s.
struct AuthManagerVoiceAuth {
    provider: SharedApiKeyProvider,
    auth_manager: Arc<xai_grok_login::AuthManager>,
}

=======
use xai_grok_voice::{SharedVoiceAuth, SttRoutes, VoiceAuthError, VoiceAuthProvider};
/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
///
/// Resolves a token per request (never a static snapshot), so a long session follows the `AuthManager` instead of pinning a token that 401s.
struct AuthManagerVoiceAuth(SharedApiKeyProvider);
>>>>>>> upstream/main
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
<<<<<<< HEAD

/// The side-call bearer is the login token only when it equals the manager's xAI session.
/// Any other bearer is a static xAI API key.
fn wire_auth(bearer: String, auth_manager: &xai_grok_login::AuthManager) -> Auth {
    let is_session = auth_manager
        .current_or_expired()
        .is_some_and(|login| login.is_session() && login.key == bearer);
    if is_session {
        Auth::XaiSession(bearer)
    } else {
        Auth::ApiKey {
            key: bearer,
            scheme: KeyScheme::Bearer,
        }
    }
}

=======
>>>>>>> upstream/main
impl VoiceAuthProvider for AuthManagerVoiceAuth {
    fn auth(&self) -> Pin<Box<dyn Future<Output = Result<Auth, VoiceAuthError>> + Send + '_>> {
        let provider = self.provider.clone();
        Box::pin(async move {
            let bearer = provider
                .side_call_bearer()
                .await
                .map_err(voice_auth_error)?;
            Ok(wire_auth(bearer, &self.auth_manager))
        })
    }
}
/// Build the voice bearer provider from the connection's `AuthManager`.
///
/// Serves xAI logins and `XAI_API_KEY` / per-model BYOK keys. A foreign-issuer login resolves to
/// [`VoiceAuthError::ForeignSession`] instead of a bearer.
<<<<<<< HEAD
pub fn build_voice_auth(auth_manager: Arc<xai_grok_login::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth {
        provider: xai_grok_login::shared_api_key_provider(auth_manager.clone()),
        auth_manager,
    })
=======
fn build_voice_auth(auth_manager: Arc<xai_grok_login::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth(
        xai_grok_login::shared_api_key_provider(auth_manager),
    ))
>>>>>>> upstream/main
}
/// Streaming bearer, plus the clip transcriber on builds that can transcribe for the signed-in login. The pipeline
/// consults it only after [`VoiceAuthError::ForeignSession`], so an xAI credential always keeps streaming.
pub fn build_stt_routes(auth_manager: Arc<xai_grok_login::AuthManager>) -> SttRoutes {
    let clip_transcriber = None;
    SttRoutes {
        auth: build_voice_auth(auth_manager),
        clip_transcriber,
    }
}
#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
