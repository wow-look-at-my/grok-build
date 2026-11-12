// billed_client_tests.rs holds constructor_call.

use super::*;

use std::path::{Path, PathBuf};

/// The constructors a scan must find. Each builds a client with no check of
/// its sink.
fn constructor_call(line: &str) -> Option<&'static str> {
    const QUALIFIED: [&str; 3] = [
        "SamplingClient::new(",
        "OaiCompatClient::new(",
        "sampling::Client::new(",
    ];
    if let Some(found) = QUALIFIED.iter().find(|p| line.contains(*p)) {
        return Some(found);
    }
    // `crate::sampling::Client` is re-exported as `Client`. A path such as `request::Client` is another type.
    let bare = "Client::new(";
    line.match_indices(bare).find_map(|(at, _)| {
        let before = line.get(..at)?.chars().next_back();
        let own_name = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == ':'));
        own_name.then_some("Client::new(")
    })
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Test-only files.
fn is_test_file(rel: &Path) -> bool {
    rel.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        name == "tests"
            || name == "tests.rs"
            || name.ends_with("_tests")
            || name.ends_with("_tests.rs")
    })
}

/// The lines of `source` outside `#[cfg(test)]` modules, with their 1-based numbers.
fn non_test_lines(source: &str) -> Vec<(usize, &str)> {
    let mut kept = Vec::new();
    let mut lines = source.lines().enumerate().peekable();
    let mut cfg_test_pending = false;
    while let Some((idx, line)) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[cfg(test)]") {
            cfg_test_pending = true;
            continue;
        }
        if cfg_test_pending && trimmed.starts_with("#[") {
            continue;
        }
        let opens_test_mod = cfg_test_pending
            && (trimmed.starts_with("mod ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub(crate) mod "));
        cfg_test_pending = false;
        if opens_test_mod {
            if !line.contains('{') {
                // `mod x;` names a file, which the file walk judges on its own.
                continue;
            }
            let mut depth = brace_delta(line);
            while depth > 0 {
                let Some((_, inner)) = lines.next() else {
                    break;
                };
                depth += brace_delta(inner);
            }
            continue;
        }
        kept.push((idx + 1, line));
    }
    kept
}

fn brace_delta(line: &str) -> i64 {
    let open = line.matches('{').count() as i64;
    let close = line.matches('}').count() as i64;
    open - close
}

/// Every shell client goes through `billed_sampling_client`, which refuses a config without a sink.
/// A direct constructor anywhere else would send calls no ledger records.
#[test]
fn no_shell_code_builds_a_sampling_client_outside_the_sanctioned_constructor() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sanctioned = Path::new("session/billed_client.rs");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut offenders = Vec::new();
    let mut sanctioned_hits = 0;
    for path in &files {
        let rel = path.strip_prefix(&src).expect("under src");
        if is_test_file(rel) {
            continue;
        }
        let source = std::fs::read_to_string(path).expect("read source");
        for (number, line) in non_test_lines(&source) {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some(call) = constructor_call(line) else {
                continue;
            };
            if rel == sanctioned {
                sanctioned_hits += 1;
            } else {
                offenders.push(format!("{}:{number}: {call}", rel.display()));
            }
        }
    }
    assert_eq!(
        sanctioned_hits, 1,
        "the sanctioned constructor must build the client exactly once"
    );
    assert!(
        offenders.is_empty(),
        "build sampling clients through crate::session::billed_client::billed_sampling_client \
         (or SessionActor::sampling_client), never directly:\n{}",
        offenders.join("\n")
    );
}

/// The sampler actor builds its clients from its config, so only session spawn may start one, with the session's sink stamped.
#[test]
fn only_session_spawn_starts_a_sampler_actor() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = Path::new("session/acp_session_impl/spawn.rs");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let offenders: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let rel = path.strip_prefix(&src).expect("under src");
            if is_test_file(rel) || rel == allowed {
                return None;
            }
            let source = std::fs::read_to_string(path).expect("read source");
            let hits: Vec<String> = non_test_lines(&source)
                .into_iter()
                .filter(|(_, line)| line.contains("SamplerActor::spawn("))
                .map(|(n, _)| format!("{}:{n}", rel.display()))
                .collect();
            (!hits.is_empty()).then(|| hits.join("\n"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "a sampler actor outside session spawn:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_scan_sees_each_constructor_spelling() {
    assert!(constructor_call("let c = xai_grok_sampler::SamplingClient::new(cfg)?;").is_some());
    assert!(constructor_call("OaiCompatClient::new(config)").is_some());
    assert!(constructor_call("crate::sampling::Client::new(c)").is_some());
    assert!(constructor_call("    Client::new(config.clone()).unwrap();").is_some());
    assert!(constructor_call("request::Client::new()").is_none());
    assert!(constructor_call("BackendClient::new()").is_none());
}

#[test]
fn the_scan_skips_inline_test_modules() {
    let source = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { SamplingClient::new(x); }\n}\nfn c() {}\n";
    let kept: Vec<&str> = non_test_lines(source).into_iter().map(|(_, l)| l).collect();
    assert_eq!(kept, vec!["fn a() {}", "fn c() {}"]);
}

#[test]
fn a_config_without_a_sink_is_refused() {
    let cfg = SamplerConfig {
        base_url: "http://127.0.0.1:9/v1".into(),
        model: "m".into(),
        ..SamplerConfig::default()
    };
    let err = billed_sampling_client(cfg).expect_err("no sink, no client");
    assert!(err.to_string().contains("usage sink"), "{err}");
}

#[test]
fn a_config_with_a_sink_builds_a_metered_client() {
    let cfg = SamplerConfig {
        base_url: "http://127.0.0.1:9/v1".into(),
        model: "m".into(),
        usage_sink: Some(xai_grok_sampler::no_session_sink("test")),
        ..SamplerConfig::default()
    };
    let client = billed_sampling_client(cfg).expect("client builds");
    assert!(client.has_usage_sink());
    assert!(client.with_http1().expect("http1 client").has_usage_sink());
}

#[test]
fn a_reported_cost_wins_over_the_price_table() {
    assert_eq!(price_model_call("no-such-model", None, Some(5)), Some(5));
    assert_eq!(price_model_call("", None, None), None);
}

/// The crates that hold model-calling code. Only `xai-grok-sampler` sends to a model provider.
const MODEL_CALLING_CRATES: [&str; 5] = [
    "xai-grok-tools",
    "xai-grok-memory",
    "xai-grok-voice",
    "xai-grok-shell",
    "xai-grok-workspace",
];

/// Code that opens a connection without the sampler.
const DIRECT_CONNECTIONS: [&str; 5] = [
    "HttpClient::build(",
    "reqwest::Client",
    "connect_ws(",
    "connect_ws_over(",
    "connect_async(",
];

/// Model provider endpoint paths. A request to one of them belongs in `SamplingClient`.
const PROVIDER_PATHS: [&str; 8] = [
    "chat/completions",
    "/responses\"",
    "/messages\"",
    "/embeddings",
    "images/generations",
    "images/edits",
    "videos/generations",
    "/tokenize-text",
];

/// The reviewed sites outside the sampler: (crate, path under `src`, pattern, hits, reason).
/// The hit count is exact, so a new site in a listed file fails too.
const REVIEWED_SITES: &[(&str, &str, &str, usize, &str)] = &[
    (
        "xai-grok-tools",
        "implementations/web_search/client.rs",
        "HttpClient::build(",
        1,
        "Kagi's search API, not a model provider",
    ),
    (
        "xai-grok-tools",
        "implementations/web_search/client.rs",
        "/responses\"",
        1,
        "the URL only feeds the endpoint allowlist check; create_response sends",
    ),
    (
        "xai-grok-tools",
        "implementations/grok_build/web_fetch/http.rs",
        "HttpClient::build(",
        1,
        "fetches the web pages the model asks for",
    ),
    (
        "xai-grok-tools",
        "implementations/grok_build/video_gen/mod.rs",
        "HttpClient::build(",
        1,
        "downloads the finished video from its presigned storage URL",
    ),
    (
        "xai-grok-tools",
        "implementations/grok_build/video_gen/mod.rs",
        "videos/generations",
        1,
        "the URL only feeds the endpoint allowlist check; generate_video sends",
    ),
    (
        "xai-grok-tools",
        "implementations/grok_build/image_gen/mod.rs",
        "images/generations",
        1,
        "the URL only feeds the endpoint allowlist check; generate_images sends",
    ),
    (
        "xai-grok-memory",
        "embedding.rs",
        "/embeddings",
        1,
        "the URL only feeds the endpoint allowlist check; create_embeddings sends",
    ),
    (
        "xai-grok-shell",
        "agent/model_pricing.rs",
        "HttpClient::build(",
        1,
        "fetches the pricing catalog",
    ),
    (
        "xai-grok-shell",
        "agent/relay.rs",
        "connect_ws(",
        1,
        "the relay socket to a remote agent session",
    ),
    (
        "xai-grok-shell",
        "agent/relay.rs",
        "connect_ws_over(",
        1,
        "the relay socket to a remote agent session, through a proxy tunnel",
    ),
    (
        "xai-grok-shell",
        "remote/client.rs",
        "HttpClient::build(",
        1,
        "the backend client for model listings and bundles",
    ),
    (
        "xai-grok-shell",
        "remote/client.rs",
        "/messages\"",
        1,
        "reads a listing's supported_endpoints to pick a backend",
    ),
    (
        "xai-grok-shell",
        "remote/client.rs",
        "/responses\"",
        1,
        "reads a listing's supported_endpoints to pick a backend",
    ),
    (
        "xai-grok-shell",
        "remote/client.rs",
        "chat/completions",
        1,
        "reads a listing's supported_endpoints to pick a backend",
    ),
    (
        "xai-grok-workspace",
        "hub_auth/proactive.rs",
        "HttpClient::build(",
        1,
        "the OIDC token endpoint",
    ),
];

/// Every pattern hit in the non-test code of `crate_name`, as (path under `src`, pattern, line).
fn provider_call_hits(crate_name: &str) -> Vec<(String, &'static str, usize)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(crate_name)
        .join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut hits = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(&src).expect("under src");
        if is_test_file(rel) {
            continue;
        }
        let source = std::fs::read_to_string(path).expect("read source");
        for (number, line) in non_test_lines(&source) {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let found = DIRECT_CONNECTIONS
                .iter()
                .chain(PROVIDER_PATHS.iter())
                .filter(|pattern| line.contains(*pattern));
            for pattern in found {
                hits.push((rel.display().to_string(), *pattern, number));
            }
        }
    }
    hits
}

/// Outside the sampler, no model-calling crate opens a connection or names a provider endpoint,
/// except at a reviewed site. A reintroduced bypass sends calls with no slot and no bill.
#[test]
fn no_crate_sends_to_a_model_provider_outside_the_sampler() {
    let mut problems = Vec::new();
    for crate_name in MODEL_CALLING_CRATES {
        let hits = provider_call_hits(crate_name);
        let mut counted: std::collections::BTreeMap<(String, &str), Vec<usize>> =
            std::collections::BTreeMap::new();
        for (file, pattern, line) in hits {
            counted.entry((file, pattern)).or_default().push(line);
        }
        for ((file, pattern), lines) in &counted {
            let reviewed = REVIEWED_SITES
                .iter()
                .find(|(c, f, p, _, _)| *c == crate_name && f == file && p == pattern);
            match reviewed {
                Some((_, _, _, count, _)) if *count == lines.len() => {}
                Some((_, _, _, count, _)) => problems.push(format!(
                    "{crate_name}/src/{file}: `{pattern}` at lines {lines:?}, reviewed for {count}"
                )),
                None => problems.push(format!(
                    "{crate_name}/src/{file}: `{pattern}` at lines {lines:?}"
                )),
            }
        }
        for (c, file, pattern, _, _) in REVIEWED_SITES.iter().filter(|s| s.0 == crate_name) {
            if !counted.contains_key(&(file.to_string(), *pattern)) {
                problems.push(format!(
                    "{c}/src/{file}: reviewed site for `{pattern}` no longer exists; delete it"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "send model requests through xai_grok_sampler::SamplingClient, which takes a request \
         slot and bills the call. A site that is not a model provider goes in REVIEWED_SITES \
         with its reason:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_provider_scan_sees_a_direct_call() {
    let line = r#"    let resp = client.post(format!("{base}/embeddings"), auth).send();"#;
    assert!(PROVIDER_PATHS.iter().any(|p| line.contains(p)));
    let line = "    let http = xai_grok_net::HttpClient::build(options)?;";
    assert!(DIRECT_CONNECTIONS.iter().any(|p| line.contains(p)));
    let line = r#"tracing::warn!("tokenize-text failed");"#;
    assert!(!PROVIDER_PATHS.iter().any(|p| line.contains(p)));
}
