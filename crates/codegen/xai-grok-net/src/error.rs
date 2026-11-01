/// Why this crate would not send a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRefusal {
    /// The xAI session was named for a host xAI does not operate.
    SessionOffXaiHost(String),
    /// A header to a host xAI does not operate carries an xAI session token.
    SessionTokenInHeader { header: String, host: String },
    /// A caller set a credential header directly instead of naming an [`crate::Auth`].
    CredentialHeaderOutsideAuth(String),
    /// The credential is not a valid header value.
    InvalidValue,
}

impl std::fmt::Display for AuthRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionOffXaiHost(host) => {
                write!(
                    f,
                    "refused to send the xAI login token to {host}, which xAI does not operate"
                )
            }
            Self::SessionTokenInHeader { header, host } => write!(
                f,
                "refused to send the xAI login token in `{header}` to {host}, which xAI does not operate"
            ),
            Self::CredentialHeaderOutsideAuth(header) => write!(
                f,
                "`{header}` carries a credential; name it as an Auth instead of setting the header"
            ),
            Self::InvalidValue => f.write_str("the credential is not a valid HTTP header value"),
        }
    }
}

impl std::error::Error for AuthRefusal {}

/// A request that did not complete.
#[derive(Debug)]
pub enum Error {
    /// This crate refused the credential. Nothing was sent.
    Auth(AuthRefusal),
    /// The transport, or the response, failed.
    Http(reqwest::Error),
}

impl Error {
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_timeout())
    }
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_connect())
    }
    pub fn is_request(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_request())
    }
    pub fn is_body(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_body())
    }
    pub fn is_decode(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_decode())
    }
    pub fn is_status(&self) -> bool {
        matches!(self, Self::Http(e) if e.is_status())
    }
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            Self::Http(e) => e.status(),
            Self::Auth(_) => None,
        }
    }
    pub fn url(&self) -> Option<&reqwest::Url> {
        match self {
            Self::Http(e) => e.url(),
            Self::Auth(_) => None,
        }
    }
    /// The transport error, for code that classifies one.
    pub fn as_http(&self) -> Option<&reqwest::Error> {
        match self {
            Self::Http(e) => Some(e),
            Self::Auth(_) => None,
        }
    }
    pub fn auth_refusal(&self) -> Option<&AuthRefusal> {
        match self {
            Self::Auth(r) => Some(r),
            Self::Http(_) => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(r) => r.fmt(f),
            Self::Http(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Auth(r) => Some(r),
            Self::Http(e) => Some(e),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<AuthRefusal> for Error {
    fn from(r: AuthRefusal) -> Self {
        Self::Auth(r)
    }
}
