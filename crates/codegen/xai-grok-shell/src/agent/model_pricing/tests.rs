use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

/// The document modelinfo answers with for `anthropic/claude-opus-4-5`,
/// trimmed to the fields this module reads.
const OPUS_DOCUMENT: &str = r#"{
  "input_cost_per_token": 0.000005,
  "output_cost_per_token": 0.000025,
  "cache_read_input_token_cost": 5e-7,
  "cache_creation_input_token_cost": 0.00000625,
  "max_tokens": 64000
}"#;

/// A model priced with no cache tier. Both cache fields are null.
const NO_CACHE_TIER_DOCUMENT: &str = r#"{
  "input_cost_per_token": 0.0000029999999999999997,
  "output_cost_per_token": 0.000015,
  "cache_read_input_token_cost": 2e-7,
  "cache_creation_input_token_cost": null
}"#;

/// Serve one HTTP response on a loopback port, then stop. Returns the base
/// URL to point the fetcher at.
fn serve_once(status_line: &str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let response = format!(
        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // Drain the request head so the client is not writing into a closed
        // socket while it waits for the answer.
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let mut stream = reader.into_inner();
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    base
}

#[test]
fn the_catalog_document_maps_onto_the_four_billing_tiers() {
    let document: ModelinfoDocument = serde_json::from_str(OPUS_DOCUMENT).expect("parse");
    let pricing = document.to_pricing().expect("priced");
    assert_eq!(pricing.input_per_token_usd, 0.000005);
    assert_eq!(pricing.output_per_token_usd, 0.000025);
    assert_eq!(pricing.cached_read_per_token_usd, 5e-7);
    assert_eq!(pricing.cache_creation_per_token_usd, 0.00000625);
}

#[test]
fn a_null_cache_tier_reads_as_zero_and_leaves_the_rest_priced() {
    let document: ModelinfoDocument = serde_json::from_str(NO_CACHE_TIER_DOCUMENT).expect("parse");
    let pricing = document.to_pricing().expect("priced");
    assert_eq!(pricing.cache_creation_per_token_usd, 0.0);
    assert_eq!(pricing.output_per_token_usd, 0.000015);
}

/// A document that prices nothing must not be recorded as an all-zero price:
/// `compute_cost_ticks` reads that as unusable anyway, and storing it as an
/// answer claims a price the catalog never gave.
#[test]
fn a_document_that_prices_nothing_is_not_an_answer() {
    let document: ModelinfoDocument = serde_json::from_str("{}").expect("parse");
    assert!(document.to_pricing().is_none());
}

#[test]
fn a_priced_model_comes_back_from_the_catalog() {
    let base = serve_once("HTTP/1.1 200 OK", OPUS_DOCUMENT);
    let pricing = fetch_pricing_blocking(&base, "anthropic/claude-opus-4-5")
        .expect("lookup")
        .expect("priced");
    assert_eq!(pricing.input_per_token_usd, 0.000005);
}

/// A 404 is an ANSWER: the catalog does not know this model. The caller
/// caches it, so a turn on an unpriced model does not re-fetch every time.
#[test]
fn an_unknown_model_answers_with_no_price_rather_than_an_error() {
    let base = serve_once("HTTP/1.1 404 Not Found", r#"{"error":"no model"}"#);
    let answer = fetch_pricing_blocking(&base, "not-a-real-model").expect("lookup");
    assert!(answer.is_none());
}

/// A failed lookup is not an answer. Caching one would pin a transient
/// outage into the catalog for the whole TTL.
#[test]
fn a_broken_catalog_is_an_error_and_not_a_cached_absence() {
    let base = serve_once("HTTP/1.1 500 Internal Server Error", "boom");
    let error = fetch_pricing_blocking(&base, "anthropic/claude-opus-4-5").expect_err("error");
    assert!(error.contains("500"), "{error}");
}

#[test]
fn a_price_stays_fresh_far_longer_than_an_absence() {
    let now = Utc::now();
    let day_old = now - chrono::Duration::hours(24);
    let priced = PricingCacheEntry {
        pricing: Some(ModelPricing {
            input_per_token_usd: 1.0,
            ..ModelPricing::default()
        }),
        fetched_at: day_old,
    };
    let absent = PricingCacheEntry {
        pricing: None,
        fetched_at: day_old,
    };
    assert!(priced.is_fresh(now));
    assert!(!absent.is_fresh(now));
}

/// A clock that moved backwards must expire the entry rather than hold it
/// fresh forever.
#[test]
fn an_entry_stamped_in_the_future_is_not_fresh() {
    let now = Utc::now();
    let entry = PricingCacheEntry {
        pricing: None,
        fetched_at: now + chrono::Duration::hours(1),
    };
    assert!(!entry.is_fresh(now));
}

/// Writing one model's answer keeps every other model already on disk.
#[test]
fn persisting_one_model_keeps_the_rest_of_the_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pricing.json");
    let first = PricingCacheEntry {
        pricing: Some(ModelPricing {
            input_per_token_usd: 1.0,
            ..ModelPricing::default()
        }),
        fetched_at: Utc::now(),
    };
    let second = PricingCacheEntry {
        pricing: None,
        fetched_at: Utc::now(),
    };
    persist_entry(&path, "model-a", &first);
    persist_entry(&path, "model-b", &second);
    let file = read_cache_file(&path);
    assert_eq!(
        file.entries
            .get("model-a")
            .and_then(|e| e.pricing.as_ref())
            .map(|p| p.input_per_token_usd),
        Some(1.0)
    );
    assert!(file.entries.contains_key("model-b"));
}

/// A corrupt file must not take the cost indicator down with it.
#[test]
fn a_corrupt_cache_file_reads_as_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pricing.json");
    std::fs::write(&path, "not json").expect("write");
    assert!(read_cache_file(&path).entries.is_empty());
}
