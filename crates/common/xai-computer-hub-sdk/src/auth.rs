//! Auth credentials and pool-dedup principal keys.

use std::collections::BTreeMap;
use std::fmt;

use http::HeaderName;
use http::header::AUTHORIZATION;

use crate::error::ClientError;

/// Credential carried into the WebSocket upgrade. Clones are cheap (the
/// secret material is at most a small number of owned strings).
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AuthCredential {
    /// Bearer token attached as the `Authorization: Bearer …` header.
    Bearer { token: String },
    /// Pre-built header bundle.
    Headers { headers: BTreeMap<String, String> },
}

impl AuthCredential {
    /// Convenience constructor for the bearer-token shape.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer {
            token: token.into(),
        }
    }

    /// Convenience constructor for the raw-header bundle shape.
    ///
    /// Names are canonicalised to lowercase and validated as
    /// [`HeaderName`] at construction so an invalid header (e.g. one
    /// containing a newline injection attempt) returns
    /// [`ClientError::InvalidConfig`] rather than being silently
    /// filtered out at upgrade time.
    pub fn headers<I, K, V>(headers: I) -> Result<Self, ClientError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for (raw_name, raw_value) in headers {
            let name = raw_name.as_ref().to_ascii_lowercase();
            HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                ClientError::InvalidConfig(format!("invalid header name {name:?}: {err}"))
            })?;
            map.insert(name, raw_value.into());
        }
        Ok(Self::Headers { headers: map })
    }

    /// Stable hashable projection used as the pool dedup key.
    ///
    /// Distinct credentials hash equal iff they carry the same secret
    /// material. See the module-level "Pool dedup and credential
    /// refresh" section for the implications when bearer tokens are
    /// rotated.
    pub fn principal_key(&self) -> PrincipalKey {
        match self {
            Self::Bearer { token } => PrincipalKey {
                fingerprint: format!("bearer:{token}"),
            },
            Self::Headers { headers } => {
                // Concatenate canonicalised name=value pairs so the fingerprint is order-independent.
                let mut joined = String::with_capacity(headers.len() * 32);
                for (name, value) in headers {
                    joined.push_str(name);
                    joined.push('=');
                    joined.push_str(value);
                    joined.push('\n');
                }
                PrincipalKey {
                    fingerprint: format!("headers:{joined}"),
                }
            }
        }
    }

    /// Headers to attach to the WebSocket upgrade request.
    ///
    /// `Headers` variant entries are infallible at this point — names
    /// were validated by [`Self::headers`].
    pub fn upgrade_headers(&self) -> Vec<(HeaderName, String)> {
        match self {
            Self::Bearer { token, .. } => {
                vec![(AUTHORIZATION, format!("Bearer {token}"))]
            }
            Self::Headers { headers, .. } => headers
                .iter()
                .filter_map(|(name, value)| {
                    HeaderName::from_bytes(name.as_bytes())
                        .ok()
                        .map(|n| (n, value.clone()))
                })
                .collect(),
        }
    }
}

impl fmt::Debug for AuthCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never log the secret; surface only the variant.
        match self {
            Self::Bearer { .. } => f
                .debug_struct("AuthCredential::Bearer")
                .finish_non_exhaustive(),
            Self::Headers { headers } => f
                .debug_struct("AuthCredential::Headers")
                .field("header_count", &headers.len())
                .finish_non_exhaustive(),
        }
    }
}

/// Stable hashable projection of an [`AuthCredential`] used as the pool dedup
/// key alongside the connect URL.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PrincipalKey {
    fingerprint: String,
}

impl PrincipalKey {
    /// Stable non-secret fingerprint (e.g. OIDC issuer+client); never tokens.
    pub fn opaque(fingerprint: impl Into<String>) -> Self {
        Self {
            fingerprint: fingerprint.into(),
        }
    }
}

impl fmt::Debug for PrincipalKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrincipalKey").finish_non_exhaustive()
    }
}

/// Owner identity surfaced by an [`AuthProvider`] alongside its credential.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthIdentity {
    /// Stable user identifier (owner of the bearer token).
    pub user_id: String,
    /// OAuth `principal_type` wire string (`"User"` / `"Team"`), when known.
    pub principal_type: Option<String>,
    /// Team id when `principal_type == "Team"`; otherwise `None`.
    pub principal_id: Option<String>,
}

/// Credential provider called on every connect/reconnect.
pub trait AuthProvider: Send + Sync + std::fmt::Debug {
    /// The credential to present now.
    fn current(&self) -> AuthCredential;

    /// Stable pool-dedup key, decoupled from the per-connect credential.
    /// Defaults to the current credential's key (existing behavior).
    fn principal_key(&self) -> PrincipalKey {
        self.current().principal_key()
    }

    /// Owner identity behind the credential, when the provider can surface
    /// it.
    fn identity(&self) -> Option<AuthIdentity> {
        None
    }
}

pub type SharedAuthProvider = std::sync::Arc<dyn AuthProvider>;

impl AuthProvider for AuthCredential {
    fn current(&self) -> AuthCredential {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_header_name_rejected_at_construction() {
        let cred = AuthCredential::headers([("authorization\nx-injected", "value")]);
        match cred {
            Err(ClientError::InvalidConfig(msg)) => {
                assert!(msg.contains("invalid header name"), "got {msg}")
            }
            other => panic!("expected InvalidConfig; got {other:?}"),
        }
    }

    #[test]
    fn valid_headers_accepted() {
        let cred = AuthCredential::headers([("authorization", "Bearer token")]).expect("valid");
        assert_eq!(cred.upgrade_headers().len(), 1);
    }
}
