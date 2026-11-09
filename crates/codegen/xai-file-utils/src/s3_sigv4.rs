//! AWS Signature Version 4 for S3: header signing and presigned query strings.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub(crate) const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
/// S3 refuses a presigned URL that lives longer than several days.
pub(crate) const MAX_PRESIGN_SECS: u64 = 7 * 24 * 60 * 60;

/// An access key pair, with the session token of temporary credentials.
#[derive(Clone)]
pub(crate) struct Credentials {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &"[redacted]")
            .field("secret_access_key", &"[redacted]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac_sha256(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC accepts a key of any length, so `new_from_slice` cannot fail here.
    match Hmac::<Sha256>::new_from_slice(key) {
        Ok(mut mac) => {
            mac.update(data.as_bytes());
            mac.finalize().into_bytes().to_vec()
        }
        Err(_) => Vec::new(),
    }
}

/// RFC 3986 encoding as SigV4 defines it. S3 encodes a path once, and keeps `/` in a key.
pub(crate) fn uri_encode(input: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b'/' if keep_slash => out.push('/'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// The query string in canonical form: each name and value encoded, then sorted.
pub(crate) fn canonical_query(pairs: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    encoded.sort();
    encoded
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Signs requests at one instant for one region.
pub(crate) struct Signer<'a> {
    creds: &'a Credentials,
    region: &'a str,
    amz_date: String,
    date: String,
}

impl<'a> Signer<'a> {
    pub(crate) fn new(creds: &'a Credentials, region: &'a str, time: DateTime<Utc>) -> Self {
        Self {
            creds,
            region,
            amz_date: time.format("%Y%m%dT%H%M%SZ").to_string(),
            date: time.format("%Y%m%d").to_string(),
        }
    }

    /// The value of the `x-amz-date` header or the `X-Amz-Date` parameter.
    pub(crate) fn amz_date(&self) -> &str {
        &self.amz_date
    }

    fn scope(&self) -> String {
        format!("{}/{}/{SERVICE}/aws4_request", self.date, self.region)
    }

    fn signature(&self, canonical_request: &str) -> String {
        let string_to_sign = format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            self.amz_date,
            self.scope(),
            sha256_hex(canonical_request.as_bytes())
        );
        let k_secret = format!("AWS4{}", self.creds.secret_access_key);
        let k_date = hmac_sha256(k_secret.as_bytes(), &self.date);
        let k_region = hmac_sha256(&k_date, self.region);
        let k_service = hmac_sha256(&k_region, SERVICE);
        let k_signing = hmac_sha256(&k_service, "aws4_request");
        hex(&hmac_sha256(&k_signing, &string_to_sign))
    }

    /// The `Authorization` header for a request.
    pub(crate) fn authorization(
        &self,
        method: &str,
        canonical_uri: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        payload_hash: &str,
    ) -> String {
        let (request, signed_headers) =
            canonical_request(method, canonical_uri, query, headers, payload_hash);
        format!(
            "{ALGORITHM} Credential={}/{}, SignedHeaders={signed_headers}, Signature={}",
            self.creds.access_key_id,
            self.scope(),
            self.signature(&request)
        )
    }

    /// The full query of a presigned URL: `query`, the `X-Amz-*` parameters and the signature.
    /// The payload is `UNSIGNED-PAYLOAD`.
    pub(crate) fn presign_query(
        &self,
        method: &str,
        canonical_uri: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        expires_secs: u64,
    ) -> Vec<(String, String)> {
        let mut names: Vec<String> = headers
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        names.sort();
        let mut pairs = query.to_vec();
        pairs.push(("X-Amz-Algorithm".into(), ALGORITHM.into()));
        pairs.push((
            "X-Amz-Credential".into(),
            format!("{}/{}", self.creds.access_key_id, self.scope()),
        ));
        pairs.push(("X-Amz-Date".into(), self.amz_date.clone()));
        pairs.push(("X-Amz-Expires".into(), expires_secs.to_string()));
        if let Some(token) = &self.creds.session_token {
            pairs.push(("X-Amz-Security-Token".into(), token.clone()));
        }
        pairs.push(("X-Amz-SignedHeaders".into(), names.join(";")));
        let (request, _) =
            canonical_request(method, canonical_uri, &pairs, headers, UNSIGNED_PAYLOAD);
        pairs.push(("X-Amz-Signature".into(), self.signature(&request)));
        pairs
    }
}

/// The canonical request and its signed-header list.
fn canonical_request(
    method: &str,
    canonical_uri: &str,
    query: &[(String, String)],
    headers: &[(String, String)],
    payload_hash: &str,
) -> (String, String) {
    let mut canonical: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| {
            let value = v.split_whitespace().collect::<Vec<_>>().join(" ");
            (k.to_ascii_lowercase(), value)
        })
        .collect();
    canonical.sort();
    let header_block: String = canonical
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();
    let signed_headers = canonical
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let request = format!(
        "{method}\n{canonical_uri}\n{}\n{header_block}\n{signed_headers}\n{payload_hash}",
        canonical_query(query)
    );
    (request, signed_headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn example_creds() -> Credentials {
        Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    fn example_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap()
    }

    /// The "GET Object" example of the S3 SigV4 header-signing docs.
    #[test]
    fn header_signing_matches_the_aws_get_object_example() {
        let creds = example_creds();
        let signer = Signer::new(&creds, "us-east-1", example_time());
        let empty = sha256_hex(b"");
        assert_eq!(
            empty,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let headers = vec![
            (
                "Host".to_owned(),
                "examplebucket.s3.amazonaws.com".to_owned(),
            ),
            ("Range".to_owned(), "bytes=0-9".to_owned()),
            ("x-amz-content-sha256".to_owned(), empty.clone()),
            ("x-amz-date".to_owned(), signer.amz_date().to_owned()),
        ];
        let auth = signer.authorization("GET", "/test.txt", &[], &headers, &empty);
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
			 SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
			 Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    /// The "GET /test.txt" example of the S3 SigV4 query-string docs.
    #[test]
    fn presign_matches_the_aws_query_string_example() {
        let creds = example_creds();
        let signer = Signer::new(&creds, "us-east-1", example_time());
        let headers = vec![(
            "host".to_owned(),
            "examplebucket.s3.amazonaws.com".to_owned(),
        )];
        let pairs = signer.presign_query("GET", "/test.txt", &[], &headers, 86400);
        let query = canonical_query(&pairs);
        assert_eq!(
            query,
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
			 &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
			 &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400\
			 &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404\
			 &X-Amz-SignedHeaders=host"
        );
    }

    #[test]
    fn presign_signs_the_session_token() {
        let mut creds = example_creds();
        creds.session_token = Some("tok/en".into());
        let signer = Signer::new(&creds, "us-east-1", example_time());
        let headers = vec![("host".to_owned(), "h".to_owned())];
        let pairs = signer.presign_query("GET", "/k", &[], &headers, 60);
        assert!(pairs.contains(&("X-Amz-Security-Token".to_owned(), "tok/en".to_owned())));
        assert!(canonical_query(&pairs).contains("X-Amz-Security-Token=tok%2Fen"));
    }

    #[test]
    fn uri_encode_keeps_unreserved_and_slash_in_keys() {
        assert_eq!(uri_encode("a b/c~d+e", true), "a%20b/c~d%2Be");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("é", true), "%C3%A9");
    }

    #[test]
    fn canonical_query_sorts_and_keeps_empty_values() {
        let pairs = vec![
            ("uploads".to_owned(), String::new()),
            ("partNumber".to_owned(), "2".to_owned()),
        ];
        assert_eq!(canonical_query(&pairs), "partNumber=2&uploads=");
    }

    #[test]
    fn debug_redacts_secrets() {
        let text = format!("{:?}", example_creds());
        assert!(!text.contains("AKIA"));
        assert!(!text.contains("EXAMPLEKEY"));
    }
}
