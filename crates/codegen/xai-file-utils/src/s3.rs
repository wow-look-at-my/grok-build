//! S3 URL presigning for the ZDR video output bucket. It signs locally and sends no request.

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

/// Path-style addressing is used when `endpoint_url` is set.
fn presign_client(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
) -> aws_sdk_s3::Client {
    let credentials = aws_sdk_s3::config::Credentials::new(
        &creds.access_key_id,
        &creds.secret_access_key,
        None,
        None,
        "grok-static",
    );
    let mut builder = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(region.to_owned()))
        .credentials_provider(credentials)
        .force_path_style(endpoint_url.is_some());
    if let Some(url) = endpoint_url {
        builder = builder.endpoint_url(url);
    }
    aws_sdk_s3::Client::from_conf(builder.build())
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
    let client = presign_client(region, endpoint_url, creds);
    let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)?;
    let presigned = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .presigned(presigning_config)
        .await?;
    Ok(presigned.uri().to_string())
}

pub async fn presign_get_url(
    region: &str,
    endpoint_url: Option<&str>,
    creds: &S3StaticCredentials,
    bucket: &str,
    key: &str,
    expires_in: std::time::Duration,
) -> anyhow::Result<String> {
    let client = presign_client(region, endpoint_url, creds);
    let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(expires_in)?;
    let presigned = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .presigned(presigning_config)
        .await?;
    Ok(presigned.uri().to_string())
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
