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
