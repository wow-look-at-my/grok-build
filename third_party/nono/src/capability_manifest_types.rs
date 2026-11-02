/// Error types.
pub mod error {
    /// Error from a `TryFrom` or `FromStr` implementation.
    pub struct ConversionError(::std::borrow::Cow<'static, str>);
    impl ::std::error::Error for ConversionError {}
    impl ::std::fmt::Display for ConversionError {
        fn fmt(
            &self,
            f: &mut ::std::fmt::Formatter<'_>,
        ) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Display::fmt(&self.0, f)
        }
    }
    impl ::std::fmt::Debug for ConversionError {
        fn fmt(
            &self,
            f: &mut ::std::fmt::Formatter<'_>,
        ) -> Result<(), ::std::fmt::Error> {
            ::std::fmt::Debug::fmt(&self.0, f)
        }
    }
    impl From<&'static str> for ConversionError {
        fn from(value: &'static str) -> Self {
            Self(value.into())
        }
    }
    impl From<String> for ConversionError {
        fn from(value: String) -> Self {
            Self(value.into())
        }
    }
}
///Filesystem access mode. 'read' grants read-only access, 'write' grants write-only access, 'readwrite' grants both.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Filesystem access mode. 'read' grants read-only access, 'write' grants write-only access, 'readwrite' grants both.",
///  "type": "string",
///  "enum": [
///    "read",
///    "write",
///    "readwrite"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum AccessMode {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "write")]
    Write,
    #[serde(rename = "readwrite")]
    Readwrite,
}
impl ::std::fmt::Display for AccessMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Read => f.write_str("read"),
            Self::Write => f.write_str("write"),
            Self::Readwrite => f.write_str("readwrite"),
        }
    }
}
impl ::std::str::FromStr for AccessMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "readwrite" => Ok(Self::Readwrite),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for AccessMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for AccessMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for AccessMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///A credential route definition for the reverse proxy. Defines how to route requests to an upstream service and inject credentials the sandboxed process never sees.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "A credential route definition for the reverse proxy. Defines how to route requests to an upstream service and inject credentials the sandboxed process never sees.",
///  "type": "object",
///  "required": [
///    "name",
///    "source",
///    "upstream"
///  ],
///  "properties": {
///    "endpoint_rules": {
///      "description": "L7 endpoint rules for this credential route. When non-empty, only requests matching at least one rule are forwarded (default-deny). When empty or absent, all endpoints are permitted.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/EndpointRule"
///      }
///    },
///    "env_var": {
///      "description": "SDK API key environment variable name for phantom token injection (e.g., 'OPENAI_API_KEY'). Required when source is a URI manager reference (op://, apple-password://, file://) because those URIs cannot be meaningfully uppercased into an environment variable name. When absent for keystore-backed credentials, derived from credential_key.to_uppercase().",
///      "type": "string",
///      "minLength": 1
///    },
///    "inject": {
///      "description": "How the credential is injected into outbound requests. Defaults to Bearer token in Authorization header.",
///      "$ref": "#/$defs/CredentialInject"
///    },
///    "name": {
///      "description": "Service name for this credential (e.g., 'github', 'internal_api'). Used to reference this credential from endpoint rules.",
///      "type": "string",
///      "minLength": 1
///    },
///    "source": {
///      "description": "Credential source URI. Supported schemes: 'env://VAR_NAME' (environment variable), 'file:///path' (file contents), 'op://vault/item/field' (1Password), 'apple-password://server/account' (macOS Keychain), or a bare account name for the system keystore.",
///      "type": "string",
///      "minLength": 1
///    },
///    "upstream": {
///      "description": "Upstream URL to proxy requests to (e.g., 'https://api.github.com'). Must use HTTPS, or HTTP only for loopback addresses.",
///      "type": "string",
///      "minLength": 1
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct Credential {
    ///L7 endpoint rules for this credential route. When non-empty, only requests matching at least one rule are forwarded (default-deny). When empty or absent, all endpoints are permitted.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub endpoint_rules: ::std::vec::Vec<EndpointRule>,
    ///SDK API key environment variable name for phantom token injection (e.g., 'OPENAI_API_KEY'). Required when source is a URI manager reference (op://, apple-password://, file://) because those URIs cannot be meaningfully uppercased into an environment variable name. When absent for keystore-backed credentials, derived from credential_key.to_uppercase().
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub env_var: ::std::option::Option<CredentialEnvVar>,
    ///How the credential is injected into outbound requests. Defaults to Bearer token in Authorization header.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub inject: ::std::option::Option<CredentialInject>,
    ///Service name for this credential (e.g., 'github', 'internal_api'). Used to reference this credential from endpoint rules.
    pub name: CredentialName,
    ///Credential source URI. Supported schemes: 'env://VAR_NAME' (environment variable), 'file:///path' (file contents), 'op://vault/item/field' (1Password), 'apple-password://server/account' (macOS Keychain), or a bare account name for the system keystore.
    pub source: CredentialSource,
    ///Upstream URL to proxy requests to (e.g., 'https://api.github.com'). Must use HTTPS, or HTTP only for loopback addresses.
    pub upstream: CredentialUpstream,
}
impl Credential {
    pub fn builder() -> builder::Credential {
        Default::default()
    }
}
///SDK API key environment variable name for phantom token injection (e.g., 'OPENAI_API_KEY'). Required when source is a URI manager reference (op://, apple-password://, file://) because those URIs cannot be meaningfully uppercased into an environment variable name. When absent for keystore-backed credentials, derived from credential_key.to_uppercase().
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "SDK API key environment variable name for phantom token injection (e.g., 'OPENAI_API_KEY'). Required when source is a URI manager reference (op://, apple-password://, file://) because those URIs cannot be meaningfully uppercased into an environment variable name. When absent for keystore-backed credentials, derived from credential_key.to_uppercase().",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CredentialEnvVar(::std::string::String);
impl ::std::ops::Deref for CredentialEnvVar {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CredentialEnvVar> for ::std::string::String {
    fn from(value: CredentialEnvVar) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CredentialEnvVar {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CredentialEnvVar {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CredentialEnvVar {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CredentialEnvVar {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CredentialEnvVar {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Configuration for how a credential is injected into proxied requests.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Configuration for how a credential is injected into proxied requests.",
///  "type": "object",
///  "properties": {
///    "format": {
///      "description": "Format string for the credential value, using {} as placeholder. Only used when mode is 'header'.",
///      "default": "Bearer {}",
///      "type": "string"
///    },
///    "header": {
///      "description": "HTTP header name for injection. Only used when mode is 'header'.",
///      "default": "Authorization",
///      "type": "string"
///    },
///    "mode": {
///      "description": "Injection mode. Defaults to 'header'.",
///      "default": "header",
///      "$ref": "#/$defs/InjectMode"
///    },
///    "path_pattern": {
///      "description": "Pattern to match in incoming URL path, using {} as placeholder for the phantom token. Required when mode is 'url_path'.",
///      "type": "string"
///    },
///    "path_replacement": {
///      "description": "Pattern for outgoing URL path, using {} as placeholder for the real credential. Defaults to path_pattern if not specified. Only used when mode is 'url_path'.",
///      "type": "string"
///    },
///    "query_param_name": {
///      "description": "Name of the query parameter to inject the credential into. Required when mode is 'query_param'.",
///      "type": "string"
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct CredentialInject {
    ///Format string for the credential value, using {} as placeholder. Only used when mode is 'header'.
    #[serde(default = "defaults::credential_inject_format")]
    pub format: ::std::string::String,
    ///HTTP header name for injection. Only used when mode is 'header'.
    #[serde(default = "defaults::credential_inject_header")]
    pub header: ::std::string::String,
    ///Injection mode. Defaults to 'header'.
    #[serde(default = "defaults::credential_inject_mode")]
    pub mode: InjectMode,
    ///Pattern to match in incoming URL path, using {} as placeholder for the phantom token. Required when mode is 'url_path'.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub path_pattern: ::std::option::Option<::std::string::String>,
    ///Pattern for outgoing URL path, using {} as placeholder for the real credential. Defaults to path_pattern if not specified. Only used when mode is 'url_path'.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub path_replacement: ::std::option::Option<::std::string::String>,
    ///Name of the query parameter to inject the credential into. Required when mode is 'query_param'.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub query_param_name: ::std::option::Option<::std::string::String>,
}
impl ::std::default::Default for CredentialInject {
    fn default() -> Self {
        Self {
            format: defaults::credential_inject_format(),
            header: defaults::credential_inject_header(),
            mode: defaults::credential_inject_mode(),
            path_pattern: Default::default(),
            path_replacement: Default::default(),
            query_param_name: Default::default(),
        }
    }
}
impl CredentialInject {
    pub fn builder() -> builder::CredentialInject {
        Default::default()
    }
}
///Service name for this credential (e.g., 'github', 'internal_api'). Used to reference this credential from endpoint rules.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Service name for this credential (e.g., 'github', 'internal_api'). Used to reference this credential from endpoint rules.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CredentialName(::std::string::String);
impl ::std::ops::Deref for CredentialName {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CredentialName> for ::std::string::String {
    fn from(value: CredentialName) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CredentialName {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CredentialName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CredentialName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CredentialName {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CredentialName {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Credential source URI. Supported schemes: 'env://VAR_NAME' (environment variable), 'file:///path' (file contents), 'op://vault/item/field' (1Password), 'apple-password://server/account' (macOS Keychain), or a bare account name for the system keystore.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Credential source URI. Supported schemes: 'env://VAR_NAME' (environment variable), 'file:///path' (file contents), 'op://vault/item/field' (1Password), 'apple-password://server/account' (macOS Keychain), or a bare account name for the system keystore.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CredentialSource(::std::string::String);
impl ::std::ops::Deref for CredentialSource {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CredentialSource> for ::std::string::String {
    fn from(value: CredentialSource) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CredentialSource {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CredentialSource {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CredentialSource {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CredentialSource {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CredentialSource {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Upstream URL to proxy requests to (e.g., 'https://api.github.com'). Must use HTTPS, or HTTP only for loopback addresses.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Upstream URL to proxy requests to (e.g., 'https://api.github.com'). Must use HTTPS, or HTTP only for loopback addresses.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct CredentialUpstream(::std::string::String);
impl ::std::ops::Deref for CredentialUpstream {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<CredentialUpstream> for ::std::string::String {
    fn from(value: CredentialUpstream) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for CredentialUpstream {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for CredentialUpstream {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for CredentialUpstream {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for CredentialUpstream {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for CredentialUpstream {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///HTTP method+path access rule for restricting which API endpoints are accessible.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "HTTP method+path access rule for restricting which API endpoints are accessible.",
///  "type": "object",
///  "required": [
///    "method",
///    "path"
///  ],
///  "properties": {
///    "method": {
///      "description": "HTTP method to match (e.g., 'GET', 'POST') or '*' for any method.",
///      "type": "string",
///      "minLength": 1
///    },
///    "path": {
///      "description": "URL path glob pattern. Uses standard glob syntax: '*' matches one path segment, '**' matches zero or more segments.",
///      "type": "string",
///      "minLength": 1
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct EndpointRule {
    ///HTTP method to match (e.g., 'GET', 'POST') or '*' for any method.
    pub method: EndpointRuleMethod,
    ///URL path glob pattern. Uses standard glob syntax: '*' matches one path segment, '**' matches zero or more segments.
    pub path: EndpointRulePath,
}
impl EndpointRule {
    pub fn builder() -> builder::EndpointRule {
        Default::default()
    }
}
///HTTP method to match (e.g., 'GET', 'POST') or '*' for any method.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "HTTP method to match (e.g., 'GET', 'POST') or '*' for any method.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct EndpointRuleMethod(::std::string::String);
impl ::std::ops::Deref for EndpointRuleMethod {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<EndpointRuleMethod> for ::std::string::String {
    fn from(value: EndpointRuleMethod) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for EndpointRuleMethod {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for EndpointRuleMethod {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for EndpointRuleMethod {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for EndpointRuleMethod {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for EndpointRuleMethod {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///URL path glob pattern. Uses standard glob syntax: '*' matches one path segment, '**' matches zero or more segments.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "URL path glob pattern. Uses standard glob syntax: '*' matches one path segment, '**' matches zero or more segments.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct EndpointRulePath(::std::string::String);
impl ::std::ops::Deref for EndpointRulePath {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<EndpointRulePath> for ::std::string::String {
    fn from(value: EndpointRulePath) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for EndpointRulePath {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for EndpointRulePath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for EndpointRulePath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for EndpointRulePath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for EndpointRulePath {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Execution strategy. 'direct' execs into the command (nono disappears). 'monitor' sandboxes then forks (default). 'supervised' forks then sandboxes (supports rollback and capability elevation).
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Execution strategy. 'direct' execs into the command (nono disappears). 'monitor' sandboxes then forks (default). 'supervised' forks then sandboxes (supports rollback and capability elevation).",
///  "type": "string",
///  "enum": [
///    "direct",
///    "monitor",
///    "supervised"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum ExecStrategy {
    #[serde(rename = "direct")]
    Direct,
    #[serde(rename = "monitor")]
    Monitor,
    #[serde(rename = "supervised")]
    Supervised,
}
impl ::std::fmt::Display for ExecStrategy {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Direct => f.write_str("direct"),
            Self::Monitor => f.write_str("monitor"),
            Self::Supervised => f.write_str("supervised"),
        }
    }
}
impl ::std::str::FromStr for ExecStrategy {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "direct" => Ok(Self::Direct),
            "monitor" => Ok(Self::Monitor),
            "supervised" => Ok(Self::Supervised),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ExecStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ExecStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ExecStrategy {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///Filesystem capability grants and denials. All paths are unresolved strings — canonicalization happens at enforcement time to keep manifests portable across machines.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Filesystem capability grants and denials. All paths are unresolved strings — canonicalization happens at enforcement time to keep manifests portable across machines.",
///  "type": "object",
///  "properties": {
///    "deny": {
///      "description": "Filesystem paths to deny access to. Deny rules take precedence over grants on platforms that support them (macOS/Seatbelt). On Linux/Landlock, deny is expressed by omitting grants.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/FsDeny"
///      }
///    },
///    "grants": {
///      "description": "Filesystem access grants. Each grant specifies a path, access mode, and whether it targets a file or directory.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/FsGrant"
///      }
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct Filesystem {
    ///Filesystem paths to deny access to. Deny rules take precedence over grants on platforms that support them (macOS/Seatbelt). On Linux/Landlock, deny is expressed by omitting grants.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub deny: ::std::vec::Vec<FsDeny>,
    ///Filesystem access grants. Each grant specifies a path, access mode, and whether it targets a file or directory.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub grants: ::std::vec::Vec<FsGrant>,
}
impl ::std::default::Default for Filesystem {
    fn default() -> Self {
        Self {
            deny: Default::default(),
            grants: Default::default(),
        }
    }
}
impl Filesystem {
    pub fn builder() -> builder::Filesystem {
        Default::default()
    }
}
///A filesystem path to deny access to.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "A filesystem path to deny access to.",
///  "type": "object",
///  "required": [
///    "path"
///  ],
///  "properties": {
///    "path": {
///      "description": "Filesystem path to deny access to.",
///      "type": "string",
///      "minLength": 1
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct FsDeny {
    ///Filesystem path to deny access to.
    pub path: FsDenyPath,
}
impl FsDeny {
    pub fn builder() -> builder::FsDeny {
        Default::default()
    }
}
///Filesystem path to deny access to.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Filesystem path to deny access to.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct FsDenyPath(::std::string::String);
impl ::std::ops::Deref for FsDenyPath {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<FsDenyPath> for ::std::string::String {
    fn from(value: FsDenyPath) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for FsDenyPath {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for FsDenyPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for FsDenyPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for FsDenyPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for FsDenyPath {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Whether a filesystem grant targets a single file or a directory (recursive).
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Whether a filesystem grant targets a single file or a directory (recursive).",
///  "type": "string",
///  "enum": [
///    "file",
///    "directory"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum FsEntryType {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "directory")]
    Directory,
}
impl ::std::fmt::Display for FsEntryType {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::File => f.write_str("file"),
            Self::Directory => f.write_str("directory"),
        }
    }
}
impl ::std::str::FromStr for FsEntryType {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "file" => Ok(Self::File),
            "directory" => Ok(Self::Directory),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for FsEntryType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for FsEntryType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for FsEntryType {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///A filesystem access grant for a single path.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "A filesystem access grant for a single path.",
///  "type": "object",
///  "required": [
///    "access",
///    "path"
///  ],
///  "properties": {
///    "access": {
///      "description": "Access mode for this path.",
///      "$ref": "#/$defs/AccessMode"
///    },
///    "path": {
///      "description": "Filesystem path to grant access to. May use ~ for home directory. Not canonicalized — resolution happens at enforcement time.",
///      "type": "string",
///      "minLength": 1
///    },
///    "type": {
///      "description": "Whether this grant targets a file or directory. Defaults to directory.",
///      "default": "directory",
///      "$ref": "#/$defs/FsEntryType"
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct FsGrant {
    ///Access mode for this path.
    pub access: AccessMode,
    ///Filesystem path to grant access to. May use ~ for home directory. Not canonicalized — resolution happens at enforcement time.
    pub path: FsGrantPath,
    ///Whether this grant targets a file or directory. Defaults to directory.
    #[serde(rename = "type", default = "defaults::fs_grant_type")]
    pub type_: FsEntryType,
}
impl FsGrant {
    pub fn builder() -> builder::FsGrant {
        Default::default()
    }
}
///Filesystem path to grant access to. May use ~ for home directory. Not canonicalized — resolution happens at enforcement time.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Filesystem path to grant access to. May use ~ for home directory. Not canonicalized — resolution happens at enforcement time.",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct FsGrantPath(::std::string::String);
impl ::std::ops::Deref for FsGrantPath {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<FsGrantPath> for ::std::string::String {
    fn from(value: FsGrantPath) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for FsGrantPath {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for FsGrantPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for FsGrantPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for FsGrantPath {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for FsGrantPath {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Credential injection mode for the reverse proxy.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Credential injection mode for the reverse proxy.",
///  "type": "string",
///  "enum": [
///    "header",
///    "url_path",
///    "query_param",
///    "basic_auth"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum InjectMode {
    #[serde(rename = "header")]
    Header,
    #[serde(rename = "url_path")]
    UrlPath,
    #[serde(rename = "query_param")]
    QueryParam,
    #[serde(rename = "basic_auth")]
    BasicAuth,
}
impl ::std::fmt::Display for InjectMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Header => f.write_str("header"),
            Self::UrlPath => f.write_str("url_path"),
            Self::QueryParam => f.write_str("query_param"),
            Self::BasicAuth => f.write_str("basic_auth"),
        }
    }
}
impl ::std::str::FromStr for InjectMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "header" => Ok(Self::Header),
            "url_path" => Ok(Self::UrlPath),
            "query_param" => Ok(Self::QueryParam),
            "basic_auth" => Ok(Self::BasicAuth),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for InjectMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for InjectMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for InjectMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///IPC mode. 'shared_memory_only' allows POSIX shared memory but denies semaphores. 'full' enables both (required for Python multiprocessing and similar runtimes).
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "IPC mode. 'shared_memory_only' allows POSIX shared memory but denies semaphores. 'full' enables both (required for Python multiprocessing and similar runtimes).",
///  "type": "string",
///  "enum": [
///    "shared_memory_only",
///    "full"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum IpcMode {
    #[serde(rename = "shared_memory_only")]
    SharedMemoryOnly,
    #[serde(rename = "full")]
    Full,
}
impl ::std::fmt::Display for IpcMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::SharedMemoryOnly => f.write_str("shared_memory_only"),
            Self::Full => f.write_str("full"),
        }
    }
}
impl ::std::str::FromStr for IpcMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "shared_memory_only" => Ok(Self::SharedMemoryOnly),
            "full" => Ok(Self::Full),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for IpcMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for IpcMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for IpcMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///Network access policy. Controls outbound connectivity, domain filtering, L7 endpoint restrictions, and port allowlists.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Network access policy. Controls outbound connectivity, domain filtering, L7 endpoint restrictions, and port allowlists.",
///  "type": "object",
///  "properties": {
///    "allow_domains": {
///      "description": "Domains allowed through the proxy. Only meaningful when mode is 'proxy'. Supports exact hostnames and wildcard suffixes (e.g., '.googleapis.com').",
///      "type": "array",
///      "items": {
///        "type": "string"
///      }
///    },
///    "dns": {
///      "description": "Whether DNS resolution is allowed. Defaults to true.",
///      "default": true,
///      "type": "boolean"
///    },
///    "endpoints": {
///      "description": "L7 endpoint definitions with per-host method+path filtering. Only meaningful when mode is 'proxy'.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/NetworkEndpoint"
///      }
///    },
///    "mode": {
///      "description": "Network access mode. 'blocked' denies all network access. 'proxy' routes traffic through the nono proxy for domain/endpoint filtering and credential injection. 'unrestricted' allows all network access (default).",
///      "default": "unrestricted",
///      "$ref": "#/$defs/NetworkMode"
///    },
///    "ports": {
///      "description": "TCP port allowlists for fine-grained network control.",
///      "$ref": "#/$defs/PortConfig"
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct Network {
    ///Domains allowed through the proxy. Only meaningful when mode is 'proxy'. Supports exact hostnames and wildcard suffixes (e.g., '.googleapis.com').
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub allow_domains: ::std::vec::Vec<::std::string::String>,
    ///Whether DNS resolution is allowed. Defaults to true.
    #[serde(default = "defaults::default_bool::<true>")]
    pub dns: bool,
    ///L7 endpoint definitions with per-host method+path filtering. Only meaningful when mode is 'proxy'.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub endpoints: ::std::vec::Vec<NetworkEndpoint>,
    ///Network access mode. 'blocked' denies all network access. 'proxy' routes traffic through the nono proxy for domain/endpoint filtering and credential injection. 'unrestricted' allows all network access (default).
    #[serde(default = "defaults::network_mode")]
    pub mode: NetworkMode,
    ///TCP port allowlists for fine-grained network control.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub ports: ::std::option::Option<PortConfig>,
}
impl ::std::default::Default for Network {
    fn default() -> Self {
        Self {
            allow_domains: Default::default(),
            dns: defaults::default_bool::<true>(),
            endpoints: Default::default(),
            mode: defaults::network_mode(),
            ports: Default::default(),
        }
    }
}
impl Network {
    pub fn builder() -> builder::Network {
        Default::default()
    }
}
///An L7 endpoint definition with host-level access and optional method+path filtering.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "An L7 endpoint definition with host-level access and optional method+path filtering.",
///  "type": "object",
///  "required": [
///    "host"
///  ],
///  "properties": {
///    "host": {
///      "description": "Hostname or hostname:port to allow (e.g., 'api.github.com:443').",
///      "type": "string",
///      "minLength": 1
///    },
///    "rules": {
///      "description": "L7 method+path filtering rules. When non-empty, only requests matching at least one rule are allowed (default-deny). When empty or absent, all paths on this host are permitted.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/EndpointRule"
///      }
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct NetworkEndpoint {
    ///Hostname or hostname:port to allow (e.g., 'api.github.com:443').
    pub host: NetworkEndpointHost,
    ///L7 method+path filtering rules. When non-empty, only requests matching at least one rule are allowed (default-deny). When empty or absent, all paths on this host are permitted.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub rules: ::std::vec::Vec<EndpointRule>,
}
impl NetworkEndpoint {
    pub fn builder() -> builder::NetworkEndpoint {
        Default::default()
    }
}
///Hostname or hostname:port to allow (e.g., 'api.github.com:443').
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Hostname or hostname:port to allow (e.g., 'api.github.com:443').",
///  "type": "string",
///  "minLength": 1
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct NetworkEndpointHost(::std::string::String);
impl ::std::ops::Deref for NetworkEndpointHost {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<NetworkEndpointHost> for ::std::string::String {
    fn from(value: NetworkEndpointHost) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for NetworkEndpointHost {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        if value.chars().count() < 1usize {
            return Err("shorter than 1 characters".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for NetworkEndpointHost {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for NetworkEndpointHost {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NetworkEndpointHost {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for NetworkEndpointHost {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///Network access mode for the sandboxed process.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Network access mode for the sandboxed process.",
///  "type": "string",
///  "enum": [
///    "blocked",
///    "proxy",
///    "unrestricted"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum NetworkMode {
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "proxy")]
    Proxy,
    #[serde(rename = "unrestricted")]
    Unrestricted,
}
impl ::std::fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Blocked => f.write_str("blocked"),
            Self::Proxy => f.write_str("proxy"),
            Self::Unrestricted => f.write_str("unrestricted"),
        }
    }
}
impl ::std::str::FromStr for NetworkMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "blocked" => Ok(Self::Blocked),
            "proxy" => Ok(Self::Proxy),
            "unrestricted" => Ok(Self::Unrestricted),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for NetworkMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for NetworkMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NetworkMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///A fully-resolved, portable capability manifest describing what a nono sandbox enforces. Manifests are the machine-consumable contract for sandbox configuration — they contain no inheritance, no legacy aliases, and no composition. Profiles compile down to manifests. NOTE: additionalProperties is true during schema development to allow forward-compatible evolution without breaking existing consumers. Switch to false once the schema stabilizes (post-1.0).
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "$id": "https://nono.dev/schemas/capability-manifest.schema.json",
///  "title": "nono Capability Manifest",
///  "description": "A fully-resolved, portable capability manifest describing what a nono sandbox enforces. Manifests are the machine-consumable contract for sandbox configuration — they contain no inheritance, no legacy aliases, and no composition. Profiles compile down to manifests. NOTE: additionalProperties is true during schema development to allow forward-compatible evolution without breaking existing consumers. Switch to false once the schema stabilizes (post-1.0).",
///  "type": "object",
///  "required": [
///    "version"
///  ],
///  "properties": {
///    "$schema": {
///      "description": "JSON Schema URI for editor validation and autocompletion.",
///      "type": "string"
///    },
///    "credentials": {
///      "description": "Credential definitions for the reverse proxy. Each credential configures a service route with secret injection and optional L7 endpoint filtering.",
///      "type": "array",
///      "items": {
///        "$ref": "#/$defs/Credential"
///      }
///    },
///    "filesystem": {
///      "description": "Filesystem capability grants and denials.",
///      "$ref": "#/$defs/Filesystem"
///    },
///    "network": {
///      "description": "Network access policy including domain filtering, endpoint rules, and port allowlists.",
///      "$ref": "#/$defs/Network"
///    },
///    "process": {
///      "description": "Process-level isolation and execution configuration.",
///      "$ref": "#/$defs/Process"
///    },
///    "rollback": {
///      "description": "Rollback snapshot configuration controlling which files are excluded from undo snapshots.",
///      "$ref": "#/$defs/Rollback"
///    },
///    "version": {
///      "description": "Schema version (semver). Consumers use this to determine which fields are available.",
///      "type": "string",
///      "const": "0.1.0",
///      "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+$"
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct NonoCapabilityManifest {
    ///Credential definitions for the reverse proxy. Each credential configures a service route with secret injection and optional L7 endpoint filtering.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub credentials: ::std::vec::Vec<Credential>,
    ///Filesystem capability grants and denials.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub filesystem: ::std::option::Option<Filesystem>,
    ///Network access policy including domain filtering, endpoint rules, and port allowlists.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub network: ::std::option::Option<Network>,
    ///Process-level isolation and execution configuration.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub process: ::std::option::Option<Process>,
    ///Rollback snapshot configuration controlling which files are excluded from undo snapshots.
    #[serde(default, skip_serializing_if = "::std::option::Option::is_none")]
    pub rollback: ::std::option::Option<Rollback>,
    ///JSON Schema URI for editor validation and autocompletion.
    #[serde(
        rename = "$schema",
        default,
        skip_serializing_if = "::std::option::Option::is_none"
    )]
    pub schema: ::std::option::Option<::std::string::String>,
    ///Schema version (semver). Consumers use this to determine which fields are available.
    pub version: NonoCapabilityManifestVersion,
}
impl NonoCapabilityManifest {
    pub fn builder() -> builder::NonoCapabilityManifest {
        Default::default()
    }
}
///Schema version (semver). Consumers use this to determine which fields are available.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Schema version (semver). Consumers use this to determine which fields are available.",
///  "type": "string",
///  "const": "0.1.0",
///  "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+$"
///}
/// ```
/// </details>
#[derive(::serde::Serialize, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct NonoCapabilityManifestVersion(::std::string::String);
impl ::std::ops::Deref for NonoCapabilityManifestVersion {
    type Target = ::std::string::String;
    fn deref(&self) -> &::std::string::String {
        &self.0
    }
}
impl ::std::convert::From<NonoCapabilityManifestVersion> for ::std::string::String {
    fn from(value: NonoCapabilityManifestVersion) -> Self {
        value.0
    }
}
impl ::std::str::FromStr for NonoCapabilityManifestVersion {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        static PATTERN: ::std::sync::LazyLock<::regress::Regex> = ::std::sync::LazyLock::new(||
        { ::regress::Regex::new("^[0-9]+\\.[0-9]+\\.[0-9]+$").unwrap() });
        if PATTERN.find(value).is_none() {
            return Err("doesn't match pattern \"^[0-9]+\\.[0-9]+\\.[0-9]+$\"".into());
        }
        Ok(Self(value.to_string()))
    }
}
impl ::std::convert::TryFrom<&str> for NonoCapabilityManifestVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for NonoCapabilityManifestVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for NonoCapabilityManifestVersion {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl<'de> ::serde::Deserialize<'de> for NonoCapabilityManifestVersion {
    fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
    where
        D: ::serde::Deserializer<'de>,
    {
        ::std::string::String::deserialize(deserializer)?
            .parse()
            .map_err(|e: self::error::ConversionError| {
                <D::Error as ::serde::de::Error>::custom(e.to_string())
            })
    }
}
///TCP port allowlists.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "TCP port allowlists.",
///  "type": "object",
///  "properties": {
///    "bind": {
///      "description": "TCP ports allowed for listening/binding.",
///      "type": "array",
///      "items": {
///        "type": "integer",
///        "maximum": 65535.0,
///        "minimum": 1.0
///      }
///    },
///    "connect": {
///      "description": "TCP ports allowed for outbound connections.",
///      "type": "array",
///      "items": {
///        "type": "integer",
///        "maximum": 65535.0,
///        "minimum": 1.0
///      }
///    },
///    "localhost": {
///      "description": "Localhost TCP ports allowed for bidirectional IPC (connect + bind).",
///      "type": "array",
///      "items": {
///        "type": "integer",
///        "maximum": 65535.0,
///        "minimum": 1.0
///      }
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct PortConfig {
    ///TCP ports allowed for listening/binding.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub bind: ::std::vec::Vec<::std::num::NonZeroU64>,
    ///TCP ports allowed for outbound connections.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub connect: ::std::vec::Vec<::std::num::NonZeroU64>,
    ///Localhost TCP ports allowed for bidirectional IPC (connect + bind).
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub localhost: ::std::vec::Vec<::std::num::NonZeroU64>,
}
impl ::std::default::Default for PortConfig {
    fn default() -> Self {
        Self {
            bind: Default::default(),
            connect: Default::default(),
            localhost: Default::default(),
        }
    }
}
impl PortConfig {
    pub fn builder() -> builder::PortConfig {
        Default::default()
    }
}
///Process-level isolation and execution configuration.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Process-level isolation and execution configuration.",
///  "type": "object",
///  "properties": {
///    "allowed_commands": {
///      "description": "Deprecated in v0.33.0. Startup-only command allowlist override. Not enforced for child processes.",
///      "deprecated": true,
///      "type": "array",
///      "items": {
///        "type": "string"
///      }
///    },
///    "blocked_commands": {
///      "description": "Deprecated in v0.33.0. Startup-only command denylist extension. Not enforced for child processes.",
///      "deprecated": true,
///      "type": "array",
///      "items": {
///        "type": "string"
///      }
///    },
///    "exec_strategy": {
///      "description": "Execution strategy for the sandboxed process. Defaults to 'monitor'.",
///      "default": "monitor",
///      "$ref": "#/$defs/ExecStrategy"
///    },
///    "ipc_mode": {
///      "description": "IPC mode controlling POSIX semaphore access. Defaults to 'shared_memory_only'.",
///      "default": "shared_memory_only",
///      "$ref": "#/$defs/IpcMode"
///    },
///    "process_info_mode": {
///      "description": "Process inspection mode. Defaults to 'isolated'.",
///      "default": "isolated",
///      "$ref": "#/$defs/ProcessInfoMode"
///    },
///    "signal_mode": {
///      "description": "Signal isolation mode. Defaults to 'isolated'.",
///      "default": "isolated",
///      "$ref": "#/$defs/SignalMode"
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct Process {
    ///Deprecated in v0.33.0. Startup-only command allowlist override. Not enforced for child processes.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub allowed_commands: ::std::vec::Vec<::std::string::String>,
    ///Deprecated in v0.33.0. Startup-only command denylist extension. Not enforced for child processes.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub blocked_commands: ::std::vec::Vec<::std::string::String>,
    ///Execution strategy for the sandboxed process. Defaults to 'monitor'.
    #[serde(default = "defaults::process_exec_strategy")]
    pub exec_strategy: ExecStrategy,
    ///IPC mode controlling POSIX semaphore access. Defaults to 'shared_memory_only'.
    #[serde(default = "defaults::process_ipc_mode")]
    pub ipc_mode: IpcMode,
    ///Process inspection mode. Defaults to 'isolated'.
    #[serde(default = "defaults::process_process_info_mode")]
    pub process_info_mode: ProcessInfoMode,
    ///Signal isolation mode. Defaults to 'isolated'.
    #[serde(default = "defaults::process_signal_mode")]
    pub signal_mode: SignalMode,
}
impl ::std::default::Default for Process {
    fn default() -> Self {
        Self {
            allowed_commands: Default::default(),
            blocked_commands: Default::default(),
            exec_strategy: defaults::process_exec_strategy(),
            ipc_mode: defaults::process_ipc_mode(),
            process_info_mode: defaults::process_process_info_mode(),
            signal_mode: defaults::process_signal_mode(),
        }
    }
}
impl Process {
    pub fn builder() -> builder::Process {
        Default::default()
    }
}
///Process inspection mode. Controls whether the sandboxed process can read process info for other processes.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Process inspection mode. Controls whether the sandboxed process can read process info for other processes.",
///  "type": "string",
///  "enum": [
///    "isolated",
///    "allow_same_sandbox",
///    "allow_all"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum ProcessInfoMode {
    #[serde(rename = "isolated")]
    Isolated,
    #[serde(rename = "allow_same_sandbox")]
    AllowSameSandbox,
    #[serde(rename = "allow_all")]
    AllowAll,
}
impl ::std::fmt::Display for ProcessInfoMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Isolated => f.write_str("isolated"),
            Self::AllowSameSandbox => f.write_str("allow_same_sandbox"),
            Self::AllowAll => f.write_str("allow_all"),
        }
    }
}
impl ::std::str::FromStr for ProcessInfoMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "isolated" => Ok(Self::Isolated),
            "allow_same_sandbox" => Ok(Self::AllowSameSandbox),
            "allow_all" => Ok(Self::AllowAll),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for ProcessInfoMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for ProcessInfoMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for ProcessInfoMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
///Rollback snapshot configuration.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Rollback snapshot configuration.",
///  "type": "object",
///  "properties": {
///    "enabled": {
///      "description": "Whether rollback snapshots are enabled. Defaults to false.",
///      "default": false,
///      "type": "boolean"
///    },
///    "exclude_globs": {
///      "description": "Glob patterns to exclude from rollback snapshots.",
///      "type": "array",
///      "items": {
///        "type": "string"
///      }
///    },
///    "exclude_patterns": {
///      "description": "Patterns to exclude from rollback snapshots. Matched against path components (exact match) or as substrings of the full path if they contain a slash.",
///      "type": "array",
///      "items": {
///        "type": "string"
///      }
///    }
///  },
///  "additionalProperties": true
///}
/// ```
/// </details>
#[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
pub struct Rollback {
    ///Whether rollback snapshots are enabled. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    ///Glob patterns to exclude from rollback snapshots.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub exclude_globs: ::std::vec::Vec<::std::string::String>,
    ///Patterns to exclude from rollback snapshots. Matched against path components (exact match) or as substrings of the full path if they contain a slash.
    #[serde(default, skip_serializing_if = "::std::vec::Vec::is_empty")]
    pub exclude_patterns: ::std::vec::Vec<::std::string::String>,
}
impl ::std::default::Default for Rollback {
    fn default() -> Self {
        Self {
            enabled: Default::default(),
            exclude_globs: Default::default(),
            exclude_patterns: Default::default(),
        }
    }
}
impl Rollback {
    pub fn builder() -> builder::Rollback {
        Default::default()
    }
}
///Signal isolation mode. 'isolated' restricts signals to the current process only. 'allow_same_sandbox' allows signaling same-sandbox children. 'allow_all' allows signaling any process.
///
/// <details><summary>JSON schema</summary>
///
/// ```json
///{
///  "description": "Signal isolation mode. 'isolated' restricts signals to the current process only. 'allow_same_sandbox' allows signaling same-sandbox children. 'allow_all' allows signaling any process.",
///  "type": "string",
///  "enum": [
///    "isolated",
///    "allow_same_sandbox",
///    "allow_all"
///  ]
///}
/// ```
/// </details>
#[derive(
    ::serde::Deserialize,
    ::serde::Serialize,
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd
)]
pub enum SignalMode {
    #[serde(rename = "isolated")]
    Isolated,
    #[serde(rename = "allow_same_sandbox")]
    AllowSameSandbox,
    #[serde(rename = "allow_all")]
    AllowAll,
}
impl ::std::fmt::Display for SignalMode {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match *self {
            Self::Isolated => f.write_str("isolated"),
            Self::AllowSameSandbox => f.write_str("allow_same_sandbox"),
            Self::AllowAll => f.write_str("allow_all"),
        }
    }
}
impl ::std::str::FromStr for SignalMode {
    type Err = self::error::ConversionError;
    fn from_str(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        match value {
            "isolated" => Ok(Self::Isolated),
            "allow_same_sandbox" => Ok(Self::AllowSameSandbox),
            "allow_all" => Ok(Self::AllowAll),
            _ => Err("invalid value".into()),
        }
    }
}
impl ::std::convert::TryFrom<&str> for SignalMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &str,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<&::std::string::String> for SignalMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: &::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
impl ::std::convert::TryFrom<::std::string::String> for SignalMode {
    type Error = self::error::ConversionError;
    fn try_from(
        value: ::std::string::String,
    ) -> ::std::result::Result<Self, self::error::ConversionError> {
        value.parse()
    }
}
/// Types for composing complex structures.
pub mod builder {
    #[derive(Clone, Debug)]
    pub struct Credential {
        endpoint_rules: ::std::result::Result<
            ::std::vec::Vec<super::EndpointRule>,
            ::std::string::String,
        >,
        env_var: ::std::result::Result<
            ::std::option::Option<super::CredentialEnvVar>,
            ::std::string::String,
        >,
        inject: ::std::result::Result<
            ::std::option::Option<super::CredentialInject>,
            ::std::string::String,
        >,
        name: ::std::result::Result<super::CredentialName, ::std::string::String>,
        source: ::std::result::Result<super::CredentialSource, ::std::string::String>,
        upstream: ::std::result::Result<
            super::CredentialUpstream,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for Credential {
        fn default() -> Self {
            Self {
                endpoint_rules: Ok(Default::default()),
                env_var: Ok(Default::default()),
                inject: Ok(Default::default()),
                name: Err("no value supplied for name".to_string()),
                source: Err("no value supplied for source".to_string()),
                upstream: Err("no value supplied for upstream".to_string()),
            }
        }
    }
    impl Credential {
        pub fn endpoint_rules<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::EndpointRule>>,
            T::Error: ::std::fmt::Display,
        {
            self.endpoint_rules = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for endpoint_rules: {e}")
                });
            self
        }
        pub fn env_var<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::CredentialEnvVar>>,
            T::Error: ::std::fmt::Display,
        {
            self.env_var = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for env_var: {e}")
                });
            self
        }
        pub fn inject<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::CredentialInject>>,
            T::Error: ::std::fmt::Display,
        {
            self.inject = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for inject: {e}"));
            self
        }
        pub fn name<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CredentialName>,
            T::Error: ::std::fmt::Display,
        {
            self.name = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for name: {e}"));
            self
        }
        pub fn source<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CredentialSource>,
            T::Error: ::std::fmt::Display,
        {
            self.source = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for source: {e}"));
            self
        }
        pub fn upstream<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::CredentialUpstream>,
            T::Error: ::std::fmt::Display,
        {
            self.upstream = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for upstream: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<Credential> for super::Credential {
        type Error = super::error::ConversionError;
        fn try_from(
            value: Credential,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                endpoint_rules: value.endpoint_rules?,
                env_var: value.env_var?,
                inject: value.inject?,
                name: value.name?,
                source: value.source?,
                upstream: value.upstream?,
            })
        }
    }
    impl ::std::convert::From<super::Credential> for Credential {
        fn from(value: super::Credential) -> Self {
            Self {
                endpoint_rules: Ok(value.endpoint_rules),
                env_var: Ok(value.env_var),
                inject: Ok(value.inject),
                name: Ok(value.name),
                source: Ok(value.source),
                upstream: Ok(value.upstream),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct CredentialInject {
        format: ::std::result::Result<::std::string::String, ::std::string::String>,
        header: ::std::result::Result<::std::string::String, ::std::string::String>,
        mode: ::std::result::Result<super::InjectMode, ::std::string::String>,
        path_pattern: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
        path_replacement: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
        query_param_name: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for CredentialInject {
        fn default() -> Self {
            Self {
                format: Ok(super::defaults::credential_inject_format()),
                header: Ok(super::defaults::credential_inject_header()),
                mode: Ok(super::defaults::credential_inject_mode()),
                path_pattern: Ok(Default::default()),
                path_replacement: Ok(Default::default()),
                query_param_name: Ok(Default::default()),
            }
        }
    }
    impl CredentialInject {
        pub fn format<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.format = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for format: {e}"));
            self
        }
        pub fn header<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::string::String>,
            T::Error: ::std::fmt::Display,
        {
            self.header = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for header: {e}"));
            self
        }
        pub fn mode<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::InjectMode>,
            T::Error: ::std::fmt::Display,
        {
            self.mode = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for mode: {e}"));
            self
        }
        pub fn path_pattern<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.path_pattern = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for path_pattern: {e}")
                });
            self
        }
        pub fn path_replacement<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.path_replacement = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for path_replacement: {e}")
                });
            self
        }
        pub fn query_param_name<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.query_param_name = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for query_param_name: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<CredentialInject> for super::CredentialInject {
        type Error = super::error::ConversionError;
        fn try_from(
            value: CredentialInject,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                format: value.format?,
                header: value.header?,
                mode: value.mode?,
                path_pattern: value.path_pattern?,
                path_replacement: value.path_replacement?,
                query_param_name: value.query_param_name?,
            })
        }
    }
    impl ::std::convert::From<super::CredentialInject> for CredentialInject {
        fn from(value: super::CredentialInject) -> Self {
            Self {
                format: Ok(value.format),
                header: Ok(value.header),
                mode: Ok(value.mode),
                path_pattern: Ok(value.path_pattern),
                path_replacement: Ok(value.path_replacement),
                query_param_name: Ok(value.query_param_name),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct EndpointRule {
        method: ::std::result::Result<super::EndpointRuleMethod, ::std::string::String>,
        path: ::std::result::Result<super::EndpointRulePath, ::std::string::String>,
    }
    impl ::std::default::Default for EndpointRule {
        fn default() -> Self {
            Self {
                method: Err("no value supplied for method".to_string()),
                path: Err("no value supplied for path".to_string()),
            }
        }
    }
    impl EndpointRule {
        pub fn method<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::EndpointRuleMethod>,
            T::Error: ::std::fmt::Display,
        {
            self.method = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for method: {e}"));
            self
        }
        pub fn path<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::EndpointRulePath>,
            T::Error: ::std::fmt::Display,
        {
            self.path = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for path: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<EndpointRule> for super::EndpointRule {
        type Error = super::error::ConversionError;
        fn try_from(
            value: EndpointRule,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                method: value.method?,
                path: value.path?,
            })
        }
    }
    impl ::std::convert::From<super::EndpointRule> for EndpointRule {
        fn from(value: super::EndpointRule) -> Self {
            Self {
                method: Ok(value.method),
                path: Ok(value.path),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Filesystem {
        deny: ::std::result::Result<
            ::std::vec::Vec<super::FsDeny>,
            ::std::string::String,
        >,
        grants: ::std::result::Result<
            ::std::vec::Vec<super::FsGrant>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for Filesystem {
        fn default() -> Self {
            Self {
                deny: Ok(Default::default()),
                grants: Ok(Default::default()),
            }
        }
    }
    impl Filesystem {
        pub fn deny<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::FsDeny>>,
            T::Error: ::std::fmt::Display,
        {
            self.deny = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for deny: {e}"));
            self
        }
        pub fn grants<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::FsGrant>>,
            T::Error: ::std::fmt::Display,
        {
            self.grants = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for grants: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Filesystem> for super::Filesystem {
        type Error = super::error::ConversionError;
        fn try_from(
            value: Filesystem,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                deny: value.deny?,
                grants: value.grants?,
            })
        }
    }
    impl ::std::convert::From<super::Filesystem> for Filesystem {
        fn from(value: super::Filesystem) -> Self {
            Self {
                deny: Ok(value.deny),
                grants: Ok(value.grants),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct FsDeny {
        path: ::std::result::Result<super::FsDenyPath, ::std::string::String>,
    }
    impl ::std::default::Default for FsDeny {
        fn default() -> Self {
            Self {
                path: Err("no value supplied for path".to_string()),
            }
        }
    }
    impl FsDeny {
        pub fn path<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::FsDenyPath>,
            T::Error: ::std::fmt::Display,
        {
            self.path = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for path: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<FsDeny> for super::FsDeny {
        type Error = super::error::ConversionError;
        fn try_from(
            value: FsDeny,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self { path: value.path? })
        }
    }
    impl ::std::convert::From<super::FsDeny> for FsDeny {
        fn from(value: super::FsDeny) -> Self {
            Self { path: Ok(value.path) }
        }
    }
    #[derive(Clone, Debug)]
    pub struct FsGrant {
        access: ::std::result::Result<super::AccessMode, ::std::string::String>,
        path: ::std::result::Result<super::FsGrantPath, ::std::string::String>,
        type_: ::std::result::Result<super::FsEntryType, ::std::string::String>,
    }
    impl ::std::default::Default for FsGrant {
        fn default() -> Self {
            Self {
                access: Err("no value supplied for access".to_string()),
                path: Err("no value supplied for path".to_string()),
                type_: Ok(super::defaults::fs_grant_type()),
            }
        }
    }
    impl FsGrant {
        pub fn access<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::AccessMode>,
            T::Error: ::std::fmt::Display,
        {
            self.access = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for access: {e}"));
            self
        }
        pub fn path<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::FsGrantPath>,
            T::Error: ::std::fmt::Display,
        {
            self.path = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for path: {e}"));
            self
        }
        pub fn type_<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::FsEntryType>,
            T::Error: ::std::fmt::Display,
        {
            self.type_ = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for type_: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<FsGrant> for super::FsGrant {
        type Error = super::error::ConversionError;
        fn try_from(
            value: FsGrant,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                access: value.access?,
                path: value.path?,
                type_: value.type_?,
            })
        }
    }
    impl ::std::convert::From<super::FsGrant> for FsGrant {
        fn from(value: super::FsGrant) -> Self {
            Self {
                access: Ok(value.access),
                path: Ok(value.path),
                type_: Ok(value.type_),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Network {
        allow_domains: ::std::result::Result<
            ::std::vec::Vec<::std::string::String>,
            ::std::string::String,
        >,
        dns: ::std::result::Result<bool, ::std::string::String>,
        endpoints: ::std::result::Result<
            ::std::vec::Vec<super::NetworkEndpoint>,
            ::std::string::String,
        >,
        mode: ::std::result::Result<super::NetworkMode, ::std::string::String>,
        ports: ::std::result::Result<
            ::std::option::Option<super::PortConfig>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for Network {
        fn default() -> Self {
            Self {
                allow_domains: Ok(Default::default()),
                dns: Ok(super::defaults::default_bool::<true>()),
                endpoints: Ok(Default::default()),
                mode: Ok(super::defaults::network_mode()),
                ports: Ok(Default::default()),
            }
        }
    }
    impl Network {
        pub fn allow_domains<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.allow_domains = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for allow_domains: {e}")
                });
            self
        }
        pub fn dns<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<bool>,
            T::Error: ::std::fmt::Display,
        {
            self.dns = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for dns: {e}"));
            self
        }
        pub fn endpoints<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::NetworkEndpoint>>,
            T::Error: ::std::fmt::Display,
        {
            self.endpoints = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for endpoints: {e}")
                });
            self
        }
        pub fn mode<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::NetworkMode>,
            T::Error: ::std::fmt::Display,
        {
            self.mode = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for mode: {e}"));
            self
        }
        pub fn ports<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::PortConfig>>,
            T::Error: ::std::fmt::Display,
        {
            self.ports = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for ports: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<Network> for super::Network {
        type Error = super::error::ConversionError;
        fn try_from(
            value: Network,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                allow_domains: value.allow_domains?,
                dns: value.dns?,
                endpoints: value.endpoints?,
                mode: value.mode?,
                ports: value.ports?,
            })
        }
    }
    impl ::std::convert::From<super::Network> for Network {
        fn from(value: super::Network) -> Self {
            Self {
                allow_domains: Ok(value.allow_domains),
                dns: Ok(value.dns),
                endpoints: Ok(value.endpoints),
                mode: Ok(value.mode),
                ports: Ok(value.ports),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct NetworkEndpoint {
        host: ::std::result::Result<super::NetworkEndpointHost, ::std::string::String>,
        rules: ::std::result::Result<
            ::std::vec::Vec<super::EndpointRule>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for NetworkEndpoint {
        fn default() -> Self {
            Self {
                host: Err("no value supplied for host".to_string()),
                rules: Ok(Default::default()),
            }
        }
    }
    impl NetworkEndpoint {
        pub fn host<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::NetworkEndpointHost>,
            T::Error: ::std::fmt::Display,
        {
            self.host = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for host: {e}"));
            self
        }
        pub fn rules<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::EndpointRule>>,
            T::Error: ::std::fmt::Display,
        {
            self.rules = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for rules: {e}"));
            self
        }
    }
    impl ::std::convert::TryFrom<NetworkEndpoint> for super::NetworkEndpoint {
        type Error = super::error::ConversionError;
        fn try_from(
            value: NetworkEndpoint,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                host: value.host?,
                rules: value.rules?,
            })
        }
    }
    impl ::std::convert::From<super::NetworkEndpoint> for NetworkEndpoint {
        fn from(value: super::NetworkEndpoint) -> Self {
            Self {
                host: Ok(value.host),
                rules: Ok(value.rules),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct NonoCapabilityManifest {
        credentials: ::std::result::Result<
            ::std::vec::Vec<super::Credential>,
            ::std::string::String,
        >,
        filesystem: ::std::result::Result<
            ::std::option::Option<super::Filesystem>,
            ::std::string::String,
        >,
        network: ::std::result::Result<
            ::std::option::Option<super::Network>,
            ::std::string::String,
        >,
        process: ::std::result::Result<
            ::std::option::Option<super::Process>,
            ::std::string::String,
        >,
        rollback: ::std::result::Result<
            ::std::option::Option<super::Rollback>,
            ::std::string::String,
        >,
        schema: ::std::result::Result<
            ::std::option::Option<::std::string::String>,
            ::std::string::String,
        >,
        version: ::std::result::Result<
            super::NonoCapabilityManifestVersion,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for NonoCapabilityManifest {
        fn default() -> Self {
            Self {
                credentials: Ok(Default::default()),
                filesystem: Ok(Default::default()),
                network: Ok(Default::default()),
                process: Ok(Default::default()),
                rollback: Ok(Default::default()),
                schema: Ok(Default::default()),
                version: Err("no value supplied for version".to_string()),
            }
        }
    }
    impl NonoCapabilityManifest {
        pub fn credentials<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<super::Credential>>,
            T::Error: ::std::fmt::Display,
        {
            self.credentials = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for credentials: {e}")
                });
            self
        }
        pub fn filesystem<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Filesystem>>,
            T::Error: ::std::fmt::Display,
        {
            self.filesystem = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for filesystem: {e}")
                });
            self
        }
        pub fn network<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Network>>,
            T::Error: ::std::fmt::Display,
        {
            self.network = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for network: {e}")
                });
            self
        }
        pub fn process<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Process>>,
            T::Error: ::std::fmt::Display,
        {
            self.process = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for process: {e}")
                });
            self
        }
        pub fn rollback<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<super::Rollback>>,
            T::Error: ::std::fmt::Display,
        {
            self.rollback = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for rollback: {e}")
                });
            self
        }
        pub fn schema<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::option::Option<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.schema = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for schema: {e}"));
            self
        }
        pub fn version<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::NonoCapabilityManifestVersion>,
            T::Error: ::std::fmt::Display,
        {
            self.version = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for version: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<NonoCapabilityManifest>
    for super::NonoCapabilityManifest {
        type Error = super::error::ConversionError;
        fn try_from(
            value: NonoCapabilityManifest,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                credentials: value.credentials?,
                filesystem: value.filesystem?,
                network: value.network?,
                process: value.process?,
                rollback: value.rollback?,
                schema: value.schema?,
                version: value.version?,
            })
        }
    }
    impl ::std::convert::From<super::NonoCapabilityManifest> for NonoCapabilityManifest {
        fn from(value: super::NonoCapabilityManifest) -> Self {
            Self {
                credentials: Ok(value.credentials),
                filesystem: Ok(value.filesystem),
                network: Ok(value.network),
                process: Ok(value.process),
                rollback: Ok(value.rollback),
                schema: Ok(value.schema),
                version: Ok(value.version),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct PortConfig {
        bind: ::std::result::Result<
            ::std::vec::Vec<::std::num::NonZeroU64>,
            ::std::string::String,
        >,
        connect: ::std::result::Result<
            ::std::vec::Vec<::std::num::NonZeroU64>,
            ::std::string::String,
        >,
        localhost: ::std::result::Result<
            ::std::vec::Vec<::std::num::NonZeroU64>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for PortConfig {
        fn default() -> Self {
            Self {
                bind: Ok(Default::default()),
                connect: Ok(Default::default()),
                localhost: Ok(Default::default()),
            }
        }
    }
    impl PortConfig {
        pub fn bind<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::num::NonZeroU64>>,
            T::Error: ::std::fmt::Display,
        {
            self.bind = value
                .try_into()
                .map_err(|e| format!("error converting supplied value for bind: {e}"));
            self
        }
        pub fn connect<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::num::NonZeroU64>>,
            T::Error: ::std::fmt::Display,
        {
            self.connect = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for connect: {e}")
                });
            self
        }
        pub fn localhost<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::num::NonZeroU64>>,
            T::Error: ::std::fmt::Display,
        {
            self.localhost = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for localhost: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<PortConfig> for super::PortConfig {
        type Error = super::error::ConversionError;
        fn try_from(
            value: PortConfig,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                bind: value.bind?,
                connect: value.connect?,
                localhost: value.localhost?,
            })
        }
    }
    impl ::std::convert::From<super::PortConfig> for PortConfig {
        fn from(value: super::PortConfig) -> Self {
            Self {
                bind: Ok(value.bind),
                connect: Ok(value.connect),
                localhost: Ok(value.localhost),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Process {
        allowed_commands: ::std::result::Result<
            ::std::vec::Vec<::std::string::String>,
            ::std::string::String,
        >,
        blocked_commands: ::std::result::Result<
            ::std::vec::Vec<::std::string::String>,
            ::std::string::String,
        >,
        exec_strategy: ::std::result::Result<super::ExecStrategy, ::std::string::String>,
        ipc_mode: ::std::result::Result<super::IpcMode, ::std::string::String>,
        process_info_mode: ::std::result::Result<
            super::ProcessInfoMode,
            ::std::string::String,
        >,
        signal_mode: ::std::result::Result<super::SignalMode, ::std::string::String>,
    }
    impl ::std::default::Default for Process {
        fn default() -> Self {
            Self {
                allowed_commands: Ok(Default::default()),
                blocked_commands: Ok(Default::default()),
                exec_strategy: Ok(super::defaults::process_exec_strategy()),
                ipc_mode: Ok(super::defaults::process_ipc_mode()),
                process_info_mode: Ok(super::defaults::process_process_info_mode()),
                signal_mode: Ok(super::defaults::process_signal_mode()),
            }
        }
    }
    impl Process {
        pub fn allowed_commands<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.allowed_commands = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for allowed_commands: {e}")
                });
            self
        }
        pub fn blocked_commands<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.blocked_commands = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for blocked_commands: {e}")
                });
            self
        }
        pub fn exec_strategy<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ExecStrategy>,
            T::Error: ::std::fmt::Display,
        {
            self.exec_strategy = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for exec_strategy: {e}")
                });
            self
        }
        pub fn ipc_mode<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::IpcMode>,
            T::Error: ::std::fmt::Display,
        {
            self.ipc_mode = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for ipc_mode: {e}")
                });
            self
        }
        pub fn process_info_mode<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::ProcessInfoMode>,
            T::Error: ::std::fmt::Display,
        {
            self.process_info_mode = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for process_info_mode: {e}")
                });
            self
        }
        pub fn signal_mode<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<super::SignalMode>,
            T::Error: ::std::fmt::Display,
        {
            self.signal_mode = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for signal_mode: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<Process> for super::Process {
        type Error = super::error::ConversionError;
        fn try_from(
            value: Process,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                allowed_commands: value.allowed_commands?,
                blocked_commands: value.blocked_commands?,
                exec_strategy: value.exec_strategy?,
                ipc_mode: value.ipc_mode?,
                process_info_mode: value.process_info_mode?,
                signal_mode: value.signal_mode?,
            })
        }
    }
    impl ::std::convert::From<super::Process> for Process {
        fn from(value: super::Process) -> Self {
            Self {
                allowed_commands: Ok(value.allowed_commands),
                blocked_commands: Ok(value.blocked_commands),
                exec_strategy: Ok(value.exec_strategy),
                ipc_mode: Ok(value.ipc_mode),
                process_info_mode: Ok(value.process_info_mode),
                signal_mode: Ok(value.signal_mode),
            }
        }
    }
    #[derive(Clone, Debug)]
    pub struct Rollback {
        enabled: ::std::result::Result<bool, ::std::string::String>,
        exclude_globs: ::std::result::Result<
            ::std::vec::Vec<::std::string::String>,
            ::std::string::String,
        >,
        exclude_patterns: ::std::result::Result<
            ::std::vec::Vec<::std::string::String>,
            ::std::string::String,
        >,
    }
    impl ::std::default::Default for Rollback {
        fn default() -> Self {
            Self {
                enabled: Ok(Default::default()),
                exclude_globs: Ok(Default::default()),
                exclude_patterns: Ok(Default::default()),
            }
        }
    }
    impl Rollback {
        pub fn enabled<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<bool>,
            T::Error: ::std::fmt::Display,
        {
            self.enabled = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for enabled: {e}")
                });
            self
        }
        pub fn exclude_globs<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.exclude_globs = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for exclude_globs: {e}")
                });
            self
        }
        pub fn exclude_patterns<T>(mut self, value: T) -> Self
        where
            T: ::std::convert::TryInto<::std::vec::Vec<::std::string::String>>,
            T::Error: ::std::fmt::Display,
        {
            self.exclude_patterns = value
                .try_into()
                .map_err(|e| {
                    format!("error converting supplied value for exclude_patterns: {e}")
                });
            self
        }
    }
    impl ::std::convert::TryFrom<Rollback> for super::Rollback {
        type Error = super::error::ConversionError;
        fn try_from(
            value: Rollback,
        ) -> ::std::result::Result<Self, super::error::ConversionError> {
            Ok(Self {
                enabled: value.enabled?,
                exclude_globs: value.exclude_globs?,
                exclude_patterns: value.exclude_patterns?,
            })
        }
    }
    impl ::std::convert::From<super::Rollback> for Rollback {
        fn from(value: super::Rollback) -> Self {
            Self {
                enabled: Ok(value.enabled),
                exclude_globs: Ok(value.exclude_globs),
                exclude_patterns: Ok(value.exclude_patterns),
            }
        }
    }
}
/// Generation of default values for serde.
pub mod defaults {
    pub(super) fn default_bool<const V: bool>() -> bool {
        V
    }
    pub(super) fn credential_inject_format() -> ::std::string::String {
        "Bearer {}".to_string()
    }
    pub(super) fn credential_inject_header() -> ::std::string::String {
        "Authorization".to_string()
    }
    pub(super) fn credential_inject_mode() -> super::InjectMode {
        super::InjectMode::Header
    }
    pub(super) fn fs_grant_type() -> super::FsEntryType {
        super::FsEntryType::Directory
    }
    pub(super) fn network_mode() -> super::NetworkMode {
        super::NetworkMode::Unrestricted
    }
    pub(super) fn process_exec_strategy() -> super::ExecStrategy {
        super::ExecStrategy::Monitor
    }
    pub(super) fn process_ipc_mode() -> super::IpcMode {
        super::IpcMode::SharedMemoryOnly
    }
    pub(super) fn process_process_info_mode() -> super::ProcessInfoMode {
        super::ProcessInfoMode::Isolated
    }
    pub(super) fn process_signal_mode() -> super::SignalMode {
        super::SignalMode::Isolated
    }
}
