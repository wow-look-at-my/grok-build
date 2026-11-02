//! Auth traits shared between `xai-file-utils` (the holder) and `xai-grok-shell` (the implementer).
//! Keeps shell types out of data-collector's import graph while still letting refresh-aware token resolution drive HTTP requests.

#![deny(clippy::indexing_slicing)]

pub mod auth_provider;
pub mod bearer_fragment;
#[cfg(test)]
mod source_tests;
pub mod visibility;

pub use auth_provider::{
    AuthCredentialProvider, CredentialSnapshot, StaticAuthCredentialProvider, credential_source,
    retrying_auth, sent_bearer_suffix,
};
pub use bearer_fragment::{BEARER_SUFFIX_LEN, bearer_suffix};
pub use visibility::{HttpAuth, RequestAuth};
pub use xai_grok_net::SentBearerSuffix;
