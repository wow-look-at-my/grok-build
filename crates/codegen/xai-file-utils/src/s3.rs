//! S3 URL presigning for the ZDR video output bucket. It signs locally and sends no request.

use xai_grok_net::Method;

use crate::s3_http::Addressing;
use crate::s3_sigv4::Credentials;

/// Static access-key credentials for presigning S3 URLs. `Debug` is redacted,
/// because the struct holds plaintext secrets.
#[derive(Clone)]
pub struct S3StaticCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for S3StaticCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3StaticCredentials")
            .field("access_key_id", &"[redacted]")
            .field("secret_access_key", &"[redacted]")
            .finish()
    }
}

impl S3StaticCredentials {
    fn to_credentials(&self) -> Credentials {
        Credentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: None,
        }
    }
}

pub async fn presign_put_url(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
    bucket: &str,
    key: &str,
    content_type: &str,
    expires_in: std::time::Duration,
) -> anyhow::Result<String> {
    crate::s3_http::presign_url(
        &creds.to_credentials(),
        &Addressing::new(region, endpoint_url)?,
        &Method::PUT,
        bucket,
        key,
        Some(content_type),
        expires_in,
        chrono::Utc::now(),
    )
}

pub async fn presign_get_url(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
    bucket: &str,
    key: &str,
    expires_in: std::time::Duration,
) -> anyhow::Result<String> {
    crate::s3_http::presign_url(
        &creds.to_credentials(),
        &Addressing::new(region, endpoint_url)?,
        &Method::GET,
        bucket,
        key,
        None,
        expires_in,
        chrono::Utc::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> S3StaticCredentials {
        S3StaticCredentials {
            access_key_id: "AKIDPRESIGN".into(),
            secret_access_key: "secret".into(),
        }
    }

    #[tokio::test]
    async fn presign_put_url_signs_a_path_style_url_for_a_custom_endpoint() {
        let url = presign_put_url(
            "us-east-1",
            Some("https://s3.example.com"),
            &creds(),
            "bucket",
            "dir/video.mp4",
            "video/mp4",
            std::time::Duration::from_secs(600),
        )
        .await
        .expect("presign must not need a network");
        assert!(
            url.starts_with("https://s3.example.com/bucket/dir/video.mp4?"),
            "path-style URL expected: {url}"
        );
        assert!(url.contains("X-Amz-Signature="), "unsigned URL: {url}");
        assert!(url.contains("AKIDPRESIGN"), "credential missing: {url}");
        assert!(url.contains("X-Amz-Expires=600"), "expiry missing: {url}");
    }

    #[tokio::test]
    async fn presign_get_url_signs_a_url() {
        let url = presign_get_url(
            "us-east-1",
            Some("https://s3.example.com"),
            &creds(),
            "bucket",
            "video.mp4",
            std::time::Duration::from_secs(60),
        )
        .await
        .expect("presign must not need a network");
        assert!(
            url.starts_with("https://s3.example.com/bucket/video.mp4?"),
            "{url}"
        );
        assert!(url.contains("X-Amz-Signature="), "unsigned URL: {url}");
    }

    #[test]
    fn debug_redacts_the_credentials() {
        let shown = format!("{:?}", creds());
        assert!(!shown.contains("\"secret\""), "{shown}");
        assert!(!shown.contains("AKIDPRESIGN"), "{shown}");
    }
}
