//! S3 object addressing and SigV4 presigned URLs. Nothing here sends a request.

use std::time::Duration;

use xai_grok_net::{Method, Url};

use crate::s3_sigv4::{self, Credentials, Signer};

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
