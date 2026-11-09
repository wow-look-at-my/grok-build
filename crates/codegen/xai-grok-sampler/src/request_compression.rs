//! zstd compression of JSON request bodies.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use crate::config::RequestCompression;

/// Origins whose last response that carried `Accept-Encoding` listed zstd.
static ADVERTISED_ZSTD: RwLock<Option<HashSet<String>>> = RwLock::new(None);

/// Origins whose last such response also sent `X-Zstd-Delta: sha256`.
static ADVERTISED_DELTA: RwLock<Option<HashSet<String>>> = RwLock::new(None);

/// The delta protocol's headers (ai-proxy `docs/delta-protocol.md`).
pub(crate) const DELTA_HEADER: &str = "x-zstd-delta";
pub(crate) const DELTA_VALUE: &str = "sha256";
pub(crate) const BASE_HEADER: &str = "x-zstd-base";
pub(crate) const BASE_MISSING_HEADER: &str = "x-zstd-base-missing";

/// A base plus a body past this skips the delta.
pub(crate) const MAX_DELTA_WINDOW_BYTES: usize = 128 << 20;

/// How many kept bodies the process remembers across all relays.
const MAX_DELTA_BASES: usize = 8;

/// The memory those bodies may use together.
const MAX_DELTA_BASE_BYTES: usize = 64 << 20;

/// A body a delta relay kept, under the endpoint URL it went to.
#[derive(Clone)]
pub(crate) struct DeltaBase {
    url: String,
    pub(crate) hash: String,
    pub(crate) body: Arc<[u8]>,
}

/// Process-wide, oldest first. Each turn builds a new client, so a per-client base is gone by the next request.
static DELTA_BASES: Mutex<VecDeque<DeltaBase>> = Mutex::new(VecDeque::new());

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// The kept body for `url` that shares the longest prefix with `plain`. The
/// newest wins a tie. Sessions and subagents share the store, so the prefix
/// finds the conversation `plain` continues.
pub(crate) fn pick_delta_base(url: &str, plain: &[u8]) -> Option<DeltaBase> {
    let bases = DELTA_BASES.lock().ok()?;
    bases
        .iter()
        .enumerate()
        .filter(|(_, base)| base.url == url)
        .filter(|(_, base)| base.body.len() + plain.len() <= MAX_DELTA_WINDOW_BYTES)
        .max_by_key(|(age, base)| (common_prefix(&base.body, plain), *age))
        .map(|(_, base)| base.clone())
}

/// Keep `plain`, the body a relay at `url` now holds, as a base.
pub(crate) fn remember_delta_base(url: String, plain: Arc<[u8]>) {
    let Ok(mut bases) = DELTA_BASES.lock() else {
        return;
    };
    let hash = sha256_hex(&plain);
    bases.retain(|base| base.hash != hash || base.url != url);
    bases.push_back(DeltaBase {
        url,
        hash,
        body: plain,
    });
    let mut total: usize = bases.iter().map(|base| base.body.len()).sum();
    while bases.len() > MAX_DELTA_BASES || (total > MAX_DELTA_BASE_BYTES && bases.len() > 1) {
        let Some(old) = bases.pop_front() else {
            break;
        };
        total -= old.body.len();
    }
}

fn set_membership(set: &RwLock<Option<HashSet<String>>>, origin: &str, member: bool, what: &str) {
    let Ok(mut guard) = set.write() else {
        return;
    };
    let set = guard.get_or_insert_with(HashSet::new);
    if member {
        if set.insert(origin.to_owned()) {
            tracing::info!(%origin, "endpoint advertised {what}");
        }
    } else {
        set.remove(origin);
    }
}

fn contains(set: &RwLock<Option<HashSet<String>>>, url: &str) -> bool {
    let Some(origin) = origin_of(url) else {
        return false;
    };
    set.read()
        .ok()
        .is_some_and(|guard| guard.as_ref().is_some_and(|set| set.contains(&origin)))
}

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
    let zstd = values.iter().any(|value| lists_zstd(value));
    let delta = zstd
        && headers
            .get_all(DELTA_HEADER)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| {
                value
                    .split(',')
                    .any(|v| v.trim().eq_ignore_ascii_case(DELTA_VALUE))
            });
    set_membership(&ADVERTISED_ZSTD, &origin, zstd, "zstd request bodies");
    set_membership(
        &ADVERTISED_DELTA,
        &origin,
        delta,
        "zstd delta request bodies",
    );
}

fn origin_advertised_zstd(url: &str) -> bool {
    contains(&ADVERTISED_ZSTD, url)
}

/// True when the origin of `url` keeps request bodies for deltas.
pub(crate) fn delta_capable(url: &str) -> bool {
    contains(&ADVERTISED_DELTA, url)
}

/// Lowercase hex SHA-256: the name a relay keeps a body under.
pub(crate) fn sha256_hex(body: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    sha2::Sha256::digest(body)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Compress `plain` against `base` as a libzstd reference prefix, the frame
/// `zstd --patch-from=base` writes. The window covers both, so every match
/// into the base is in reach.
pub(crate) fn zstd_compress_against(plain: &[u8], base: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let started = std::time::Instant::now();
    let span = (plain.len() + base.len()).max(1 << 10);
    let window_log = usize::BITS - (span - 1).leading_zeros();
    let mut encoder = zstd::stream::write::Encoder::with_ref_prefix(
        Vec::with_capacity(plain.len() / 8),
        ZSTD_LEVEL,
        base,
    )?;
    encoder.window_log(window_log)?;
    encoder.long_distance_matching(span > 8 << 20)?;
    encoder.include_contentsize(true)?;
    encoder.set_pledged_src_size(Some(plain.len() as u64))?;
    encoder.write_all(plain)?;
    let compressed = encoder.finish()?;
    tracing::info!(
        pre_compression_bytes = plain.len(),
        base_bytes = base.len(),
        post_compression_bytes = compressed.len(),
        compression_duration_ms = started.elapsed().as_millis() as u64,
        "compressed request body as a zstd delta"
    );
    Ok(compressed)
}

/// [`zstd_compress_against`] off the runtime for large bodies. `None` means it
/// failed and the caller should compress without the base.
pub(crate) async fn compress_body_against(
    plain: std::sync::Arc<[u8]>,
    base: std::sync::Arc<[u8]>,
) -> Option<Vec<u8>> {
    let result = if plain.len() >= OFFLOAD_COMPRESS_BYTES {
        match xai_spawn::spawn_blocking(move || zstd_compress_against(&plain, &base)).await {
            Ok(result) => result,
            Err(join) => Err(std::io::Error::other(join)),
        }
    } else {
        zstd_compress_against(&plain, &base)
    };
    result
        .inspect_err(|error| tracing::warn!(%error, "zstd delta failed; sending a full frame"))
        .ok()
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
        RequestCompression::Auto => origin_advertised_zstd(url),
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

    const UNSEEN: &str = "https://never-advertised.invalid/v1/chat/completions";

    /// Conversations take turns on one relay. Each next body is encoded
    /// against its own conversation, not the body sent last.
    #[test]
    fn a_body_builds_on_the_conversation_it_continues() {
        let url = "https://picker.invalid/up/v1/messages";
        let other = "https://picker.invalid/other/v1/messages";
        let main: Arc<[u8]> = Arc::from(&b"main conversation turn one"[..]);
        let side: Arc<[u8]> = Arc::from(&b"side request about titles"[..]);
        remember_delta_base(url.to_owned(), main.clone());
        remember_delta_base(url.to_owned(), side.clone());

        let next = b"main conversation turn one, turn two";
        let picked = pick_delta_base(url, next).expect("a base for the url");
        assert_eq!(picked.body, main, "the main turn builds on the main body");
        assert_eq!(picked.hash, sha256_hex(&main));
        assert!(
            pick_delta_base(other, next).is_none(),
            "a body kept under one route is never a base for another"
        );

        for n in 0..MAX_DELTA_BASES {
            remember_delta_base(
                url.to_owned(),
                Arc::from(format!("filler {n}").into_bytes()),
            );
        }
        assert!(
            pick_delta_base(url, next).is_none_or(|base| base.body != main),
            "the oldest bodies leave once the store is full"
        );
    }

    /// A config written before the rename still says "none".
    #[test]
    fn auto_reads_its_old_name() {
        for (raw, want) in [
            ("\"auto\"", RequestCompression::Auto),
            ("\"none\"", RequestCompression::Auto),
            ("\"off\"", RequestCompression::Off),
            ("\"zstd\"", RequestCompression::Zstd),
        ] {
            assert_eq!(
                serde_json::from_str::<RequestCompression>(raw).ok(),
                Some(want),
                "{raw}"
            );
        }
        assert_eq!(
            serde_json::to_string(&RequestCompression::Auto)
                .ok()
                .as_deref(),
            Some("\"auto\"")
        );
    }

    #[test]
    fn should_compress_requires_zstd_config_and_min_size() {
        assert!(should_compress(
            RequestCompression::Zstd,
            UNSEEN,
            MIN_COMPRESS_BYTES
        ));
        assert!(!should_compress(
            RequestCompression::Zstd,
            UNSEEN,
            MIN_COMPRESS_BYTES - 1
        ));
        assert!(!should_compress(
            RequestCompression::Auto,
            UNSEEN,
            MIN_COMPRESS_BYTES * 4
        ));
    }

    fn headers(values: &[&str]) -> xai_grok_net::header::HeaderMap {
        let mut map = xai_grok_net::header::HeaderMap::new();
        for value in values {
            map.append(
                xai_grok_net::header::ACCEPT_ENCODING,
                value.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn accept_encoding_parsing() {
        assert!(lists_zstd("zstd"));
        assert!(lists_zstd("gzip, ZSTD;q=0.5"));
        assert!(!lists_zstd("gzip, br"));
        assert!(!lists_zstd("zstd;q=0"));
        assert!(!lists_zstd("zstdx"));
    }

    #[test]
    fn advertised_origin_compresses_until_it_stops_advertising() {
        let url = "https://Proxy.Example:8317/v1/messages?beta=true";
        let sibling = "https://proxy.example:8317/v1/models";
        let big = MIN_COMPRESS_BYTES;
        assert!(!should_compress(RequestCompression::Auto, url, big));

        note_response_headers(sibling, &headers(&["zstd"]));
        assert!(should_compress(RequestCompression::Auto, url, big));
        assert!(!should_compress(RequestCompression::Off, url, big));
        assert!(!should_compress(RequestCompression::Auto, url, big - 1));
        assert!(!should_compress(
            RequestCompression::Auto,
            "https://proxy.example:9999/v1/messages",
            big
        ));

        note_response_headers(url, &headers(&[]));
        assert!(should_compress(RequestCompression::Auto, url, big));

        note_response_headers(url, &headers(&["gzip"]));
        assert!(!should_compress(RequestCompression::Auto, url, big));
    }
}
