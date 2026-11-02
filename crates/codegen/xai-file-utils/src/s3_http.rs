//! A small S3 client: SigV4-signed object requests sent through `xai_grok_net::HttpClient`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use xai_grok_net::header::{HeaderMap, HeaderName, HeaderValue};
use xai_grok_net::{Auth, ClientOptions, Method, Url};

use crate::s3_sigv4::{self, Credentials, Signer};

/// The session token header of temporary AWS credentials.
const AMZ_SECURITY_TOKEN: &str = "x-amz-security-token";
/// Attempts per request, the first included. Only a transient failure is tried again.
const MAX_ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(3100);

/// A failed S3 request.
#[derive(Debug)]
pub(crate) enum S3Error {
    /// S3 answered with a non-success status, or with an `<Error>` body.
    Status {
        operation: &'static str,
        status: u16,
        code: Option<String>,
        message: Option<String>,
    },
    /// The request was not sent or got no response.
    Transport {
        operation: &'static str,
        error: xai_grok_net::Error,
    },
    /// The response body could not be read.
    Body {
        operation: &'static str,
        error: xai_grok_net::TransportError,
    },
    /// The request could not be built.
    Build {
        operation: &'static str,
        message: String,
    },
}

impl S3Error {
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::Transport { .. } | Self::Body { .. } | Self::Build { .. } => None,
        }
    }

    fn is_transient(&self) -> bool {
        match self {
            Self::Status { status, code, .. } => {
                matches!(status, 429 | 500 | 502 | 503 | 504)
                    || matches!(code.as_deref(), Some("SlowDown" | "InternalError"))
            }
            Self::Transport { error, .. } => {
                error.auth_refusal().is_none() && (error.is_timeout() || error.is_connect())
            }
            Self::Body { .. } => true,
            Self::Build { .. } => false,
        }
    }
}

impl std::fmt::Display for S3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status {
                operation,
                status,
                code,
                message,
            } => {
                write!(f, "S3 {operation} returned HTTP {status}")?;
                if let Some(code) = code {
                    write!(f, ": {code}")?;
                }
                if let Some(message) = message {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::Transport { operation, error } => write!(f, "S3 {operation} failed: {error}"),
            Self::Body { operation, error } => write!(f, "S3 {operation} response body: {error}"),
            Self::Build { operation, message } => write!(f, "S3 {operation}: {message}"),
        }
    }
}

impl std::error::Error for S3Error {}

/// A successful S3 response.
pub(crate) struct S3Response {
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

/// The text of the first `<tag>` element in `xml`, unescaped.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let rest = xml.get(start..)?;
    let len = rest.find(&close)?;
    Some(xml_unescape(rest.get(..len)?))
}

fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

pub(crate) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn error_from_body(operation: &'static str, status: u16, body: &[u8]) -> S3Error {
    let text = String::from_utf8_lossy(body);
    S3Error::Status {
        operation,
        status,
        code: xml_text(&text, "Code"),
        message: xml_text(&text, "Message"),
    }
}

fn body_is_error(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    let text = text.trim_start();
    let root = match text.strip_prefix("<?xml") {
        Some(rest) => rest.split_once("?>").map_or("", |(_, r)| r.trim_start()),
        None => text,
    };
    root.starts_with("<Error>")
}

/// Path style under `endpoint` when one is set.
#[derive(Debug, Clone)]
pub(crate) struct Addressing {
    region: String,
    endpoint: Option<Url>,
}

fn is_virtual_hostable(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !bucket.starts_with('-')
        && !bucket.ends_with('-')
}

impl Addressing {
    pub(crate) fn new(region: &str, endpoint_url: Option<&str>) -> anyhow::Result<Self> {
        let endpoint = endpoint_url
            .map(|u| {
                Url::parse(u).map_err(|e| anyhow::anyhow!("invalid S3 endpoint URL {u:?}: {e}"))
            })
            .transpose()?;
        Ok(Self {
            region: region.to_owned(),
            endpoint,
        })
    }

    pub(crate) fn region(&self) -> &str {
        &self.region
    }

    /// The object URL, with no query.
    pub(crate) fn object_url(&self, bucket: &str, key: &str) -> anyhow::Result<Url> {
        let bucket_seg = s3_sigv4::uri_encode(bucket, false);
        let key_path = s3_sigv4::uri_encode(key, true);
        let url = match &self.endpoint {
            Some(base) => {
                let mut url = base.clone();
                let prefix = base.path().trim_end_matches('/');
                url.set_path(&format!("{prefix}/{bucket_seg}/{key_path}"));
                url.set_query(None);
                url
            }
            None if is_virtual_hostable(bucket) => Url::parse(&format!(
                "https://{bucket}.s3.{}.amazonaws.com/{key_path}",
                self.region
            ))?,
            None => Url::parse(&format!(
                "https://s3.{}.amazonaws.com/{bucket_seg}/{key_path}",
                self.region
            ))?,
        };
        Ok(url)
    }
}

/// The `Host` header value reqwest sends for `url`.
pub(crate) fn host_header(url: &Url) -> anyhow::Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("S3 URL has no host: {url}"))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

/// Put `pairs` on `url` in the encoding the signature covers.
fn set_query(url: &mut Url, pairs: &[(String, String)]) {
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&s3_sigv4::canonical_query(pairs)));
    }
}

/// One object operation.
pub(crate) struct ObjectRequest<'a> {
    pub(crate) operation: &'static str,
    pub(crate) method: Method,
    pub(crate) bucket: &'a str,
    pub(crate) key: &'a str,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) content_type: Option<&'a str>,
    pub(crate) body: Bytes,
}

#[derive(Clone)]
pub(crate) struct S3Client {
    inner: Arc<Inner>,
}

struct Inner {
    http: xai_grok_net::HttpClient,
    creds: Credentials,
    addressing: Addressing,
}

impl S3Client {
    pub(crate) fn new(creds: Credentials, addressing: Addressing) -> anyhow::Result<Self> {
        let http = xai_grok_net::HttpClient::build(
            ClientOptions::default().connect_timeout(CONNECT_TIMEOUT),
        )
        .map_err(|e| anyhow::anyhow!("build the S3 HTTP client: {e}"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                creds,
                addressing,
            }),
        })
    }

    /// Send `request`. A transient failure is sent again, up to [`MAX_ATTEMPTS`] in all.
    pub(crate) async fn send(&self, request: &ObjectRequest<'_>) -> Result<S3Response, S3Error> {
        let mut attempt = 1;
        loop {
            match self.send_once(request).await {
                Err(e) if attempt < MAX_ATTEMPTS && e.is_transient() => {
                    tracing::debug!(error = %e, attempt, "retrying S3 request");
                    attempt += 1;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                other => return other,
            }
        }
    }

    async fn send_once(&self, request: &ObjectRequest<'_>) -> Result<S3Response, S3Error> {
        let operation = request.operation;
        let build_error = |message: String| S3Error::Build { operation, message };
        let inner = &*self.inner;
        let mut url = inner
            .addressing
            .object_url(request.bucket, request.key)
            .map_err(|e| build_error(e.to_string()))?;
        set_query(&mut url, &request.query);
        let host = host_header(&url).map_err(|e| build_error(e.to_string()))?;

        let payload_hash = s3_sigv4::sha256_hex(&request.body);
        let signer = Signer::new(&inner.creds, inner.addressing.region(), chrono::Utc::now());
        let mut signed = vec![
            ("host".to_owned(), host),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), signer.amz_date().to_owned()),
        ];
        if let Some(ct) = request.content_type {
            signed.push(("content-type".to_owned(), ct.to_owned()));
        }
        if let Some(token) = &inner.creds.session_token {
            signed.push((AMZ_SECURITY_TOKEN.to_owned(), token.clone()));
        }
        let authorization = signer.authorization(
            request.method.as_str(),
            url.path(),
            &request.query,
            &signed,
            &payload_hash,
        );

        let header = |name: &str, value: &str| -> Result<(HeaderName, HeaderValue), S3Error> {
            let n = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| build_error(format!("header name {name}: {e}")))?;
            let v = HeaderValue::from_str(value)
                .map_err(|e| build_error(format!("header {name}: {e}")))?;
            Ok((n, v))
        };
        // The signature and the session token travel as the request's credential.
        let mut credential = HeaderMap::new();
        let mut plain = HeaderMap::new();
        let (n, v) = header("authorization", &authorization)?;
        credential.insert(n, v);
        for (name, value) in signed.iter().filter(|(name, _)| name != "host") {
            let (n, v) = header(name, value)?;
            if name == AMZ_SECURITY_TOKEN {
                credential.insert(n, v);
            } else {
                plain.insert(n, v);
            }
        }

        let response = inner
            .http
            .request(request.method.clone(), url, Auth::Configured(credential))
            .headers(plain)
            .body(request.body.clone())
            .send()
            .await
            .map_err(|error| S3Error::Transport { operation, error })?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|error| S3Error::Body { operation, error })?;
        if !status.is_success() || (request.method == Method::POST && body_is_error(&body)) {
            return Err(error_from_body(operation, status.as_u16(), &body));
        }
        Ok(S3Response { headers, body })
    }
}

/// A presigned URL for `method` on the object. A `content_type` is a signed header,
/// so the request that uses the URL must send that same value.
#[allow(clippy::too_many_arguments)]
pub(crate) fn presign_url(
    creds: &Credentials,
    addressing: &Addressing,
    method: &Method,
    bucket: &str,
    key: &str,
    content_type: Option<&str>,
    expires_in: Duration,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<String> {
    let expires_secs = expires_in.as_secs();
    if expires_secs > s3_sigv4::MAX_PRESIGN_SECS {
        anyhow::bail!(
            "presigned URL expiry {expires_secs}s exceeds the S3 limit of {}s",
            s3_sigv4::MAX_PRESIGN_SECS
        );
    }
    let mut url = addressing.object_url(bucket, key)?;
    let mut headers = vec![("host".to_owned(), host_header(&url)?)];
    if let Some(ct) = content_type {
        headers.push(("content-type".to_owned(), ct.to_owned()));
    }
    let signer = Signer::new(creds, addressing.region(), now);
    let pairs = signer.presign_query(method.as_str(), url.path(), &[], &headers, expires_secs);
    set_query(&mut url, &pairs);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressing_uses_path_style_under_an_endpoint() {
        let a = Addressing::new("us-east-1", Some("http://127.0.0.1:9000/base/")).unwrap();
        let url = a.object_url("my-bucket", "dir/a b+c.txt").unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:9000/base/my-bucket/dir/a%20b%2Bc.txt"
        );
        assert_eq!(host_header(&url).unwrap(), "127.0.0.1:9000");
    }

    #[test]
    fn addressing_uses_virtual_host_on_aws() {
        let a = Addressing::new("eu-west-1", None).unwrap();
        let url = a.object_url("my-bucket", "k/v.json").unwrap();
        assert_eq!(
            url.as_str(),
            "https://my-bucket.s3.eu-west-1.amazonaws.com/k/v.json"
        );
        assert_eq!(
            host_header(&url).unwrap(),
            "my-bucket.s3.eu-west-1.amazonaws.com"
        );
        let dotted = a.object_url("my.bucket", "k").unwrap();
        assert_eq!(
            dotted.as_str(),
            "https://s3.eu-west-1.amazonaws.com/my.bucket/k"
        );
    }

    #[test]
    fn xml_helpers_round_trip() {
        let body = "<?xml version=\"1.0\"?><Error><Code>NoSuchUpload</Code>\
                    <Message>a &amp; b</Message></Error>";
        assert_eq!(xml_text(body, "Code").as_deref(), Some("NoSuchUpload"));
        assert_eq!(xml_text(body, "Message").as_deref(), Some("a & b"));
        assert!(body_is_error(body.as_bytes()));
        assert!(!body_is_error(b"<CompleteMultipartUploadResult/>"));
        assert_eq!(xml_escape("\"e&t\""), "&quot;e&amp;t&quot;");
    }

    #[test]
    fn presign_rejects_more_than_seven_days() {
        let creds = Credentials {
            access_key_id: "a".into(),
            secret_access_key: "s".into(),
            session_token: None,
        };
        let a = Addressing::new("us-east-1", None).unwrap();
        let err = presign_url(
            &creds,
            &a,
            &Method::GET,
            "bucket",
            "k",
            None,
            Duration::from_secs(s3_sigv4::MAX_PRESIGN_SECS + 1),
            chrono::Utc::now(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn presign_put_signs_the_content_type() {
        let creds = Credentials {
            access_key_id: "AK".into(),
            secret_access_key: "s".into(),
            session_token: None,
        };
        let a = Addressing::new("us-east-1", Some("https://s3.example.com")).unwrap();
        let url = presign_url(
            &creds,
            &a,
            &Method::PUT,
            "b",
            "dir/v.mp4",
            Some("video/mp4"),
            Duration::from_secs(60),
            chrono::Utc::now(),
        )
        .unwrap();
        assert!(
            url.starts_with("https://s3.example.com/b/dir/v.mp4?"),
            "{url}"
        );
        assert!(
            url.contains("X-Amz-SignedHeaders=content-type%3Bhost"),
            "{url}"
        );
        assert!(url.contains("X-Amz-Expires=60"), "{url}");
    }
}
