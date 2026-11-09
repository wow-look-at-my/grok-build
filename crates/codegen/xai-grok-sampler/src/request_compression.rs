//! zstd compression of JSON request bodies.

use std::collections::HashSet;
use std::sync::RwLock;

use crate::config::RequestCompression;

/// Origins whose last response that carried `Accept-Encoding` listed zstd.
static ADVERTISED_ZSTD: RwLock<Option<HashSet<String>>> = RwLock::new(None);

/// `scheme://authority` of `url`, lowercased. `None` for a string with no scheme.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    Some(format!("{scheme}://{authority}").to_ascii_lowercase())
}

/// True when an `Accept-Encoding` value lists zstd with a non-zero weight.
fn lists_zstd(value: &str) -> bool {
    value.split(',').any(|entry| {
        let mut parts = entry.split(';').map(str::trim);
        if !parts
            .next()
            .is_some_and(|coding| coding.eq_ignore_ascii_case("zstd"))
        {
            return false;
        }
        !parts.any(|param| {
            param
                .strip_prefix("q=")
                .and_then(|q| q.parse::<f32>().ok())
                .is_some_and(|q| q == 0.0)
        })
    })
}

/// Record what the response from `url` said about request encodings. A
/// response with no `Accept-Encoding` header changes nothing.
pub(crate) fn note_response_headers(url: &str, headers: &xai_grok_net::header::HeaderMap) {
    let Some(origin) = origin_of(url) else {
        return;
    };
    let values: Vec<&str> = headers
        .get_all(xai_grok_net::header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    if values.is_empty() {
        return;
    }
    let advertised = values.iter().any(|value| lists_zstd(value));
    let Ok(mut guard) = ADVERTISED_ZSTD.write() else {
        return;
    };
    let set = guard.get_or_insert_with(HashSet::new);
    if advertised {
        if set.insert(origin.clone()) {
            tracing::info!(%origin, "endpoint advertised zstd request bodies");
        }
    } else {
        set.remove(&origin);
    }
}

fn origin_advertised_zstd(url: &str) -> bool {
    let Some(origin) = origin_of(url) else {
        return false;
    };
    ADVERTISED_ZSTD
        .read()
        .ok()
        .is_some_and(|guard| guard.as_ref().is_some_and(|set| set.contains(&origin)))
}

/// Below this the CPU spent compressing is not worth the bytes saved.
pub(crate) const MIN_COMPRESS_BYTES: usize = 64 * 1024;

/// Level 3: fast, good ratio on JSON, and a 2 MiB window that stays well
/// inside the proxy's decoder bound.
const ZSTD_LEVEL: i32 = 3;

/// From here up, compression leaves the async worker: level-3 zstd runs at a
/// few hundred MB/s, so a multi-megabyte body would stall it for tens of ms.
const OFFLOAD_COMPRESS_BYTES: usize = 1024 * 1024;

pub(crate) fn should_compress(config: RequestCompression, url: &str, body_len: usize) -> bool {
    if body_len < MIN_COMPRESS_BYTES {
        return false;
    }
    match config {
        RequestCompression::Zstd => true,
        RequestCompression::None => origin_advertised_zstd(url),
        RequestCompression::Off => false,
    }
}

/// Compress `json` for the wire, off the runtime for large bodies. `None`
/// means compression failed and the caller should send the plain bytes.
pub(crate) async fn compress_body(json: &[u8]) -> Option<Vec<u8>> {
    let result = if json.len() >= OFFLOAD_COMPRESS_BYTES {
        let owned = json.to_vec();
        match xai_spawn::spawn_blocking(move || zstd_compress(&owned)).await {
            Ok(result) => result,
            Err(join) => Err(std::io::Error::other(join)),
        }
    } else {
        zstd_compress(json)
    };
    result
        .inspect_err(|error| tracing::warn!(%error, "zstd compression failed; sending plain JSON"))
        .ok()
}

pub(crate) fn zstd_compress(json: &[u8]) -> std::io::Result<Vec<u8>> {
    let started = std::time::Instant::now();
    let compressed = zstd::encode_all(json, ZSTD_LEVEL)?;
    // Info, not debug: the `--debug` firehose keeps this crate at info and a support log must show whether a body was compressed.
    tracing::info!(
        pre_compression_bytes = json.len(),
        post_compression_bytes = compressed.len(),
        compression_duration_ms = started.elapsed().as_millis() as u64,
        "compressed request body with zstd"
    );
    Ok(compressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_compress_requires_zstd_config_and_min_size() {
        assert!(should_compress(
            RequestCompression::Zstd,
            MIN_COMPRESS_BYTES
        ));
        assert!(!should_compress(
            RequestCompression::Zstd,
            MIN_COMPRESS_BYTES - 1
        ));
        assert!(!should_compress(
            RequestCompression::None,
            MIN_COMPRESS_BYTES * 4
        ));
    }
}
