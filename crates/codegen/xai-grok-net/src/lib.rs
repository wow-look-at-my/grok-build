//! The HTTP path of the grok CLI.

#![deny(clippy::indexing_slicing)]

mod auth;
mod blocking;
mod client;
mod error;
mod session;
pub mod ws;

pub use auth::{
    Auth, CredentialSource, KeyScheme, SentBearerSuffix, is_credential_header, is_xai_host,
};
pub use blocking::{BlockingHttpClient, BlockingOptions, BlockingRequest};
pub use client::{ClientOptions, HttpClient, Request};
pub use error::{AuthRefusal, Error};
pub use session::register_session_token;

/// The transport error a response body read returns.
pub type TransportError = request::Error;

pub use request::{
    Body, Certificate, Identity, Method, Proxy, Response, StatusCode, Url, Version, header,
    multipart, redirect,
};

/// The blocking response type.
pub mod blocking_types {
    pub use request::blocking::{Body, Response, multipart};
}
