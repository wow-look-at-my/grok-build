//! Every HTTP request in the workspace goes through `xai-grok-net`.

use std::path::{Path, PathBuf};

/// Libraries that open an HTTP or WebSocket connection.
const CLIENT_LIBRARIES: &[&str] = &[
    "reqwest",
    "request",
    "reqwest-middleware",
    "tokio-tungstenite",
    "tungstenite",
    "ureq",
    "isahc",
    "surf",
    "attohttpc",
    "awc",
];

/// Crates that may link a client library.
const ALLOWED_CRATES: &[&str] = &["xai-grok-net", "xai-grok-extra-ca"];

/// Calls that hand out a raw client or a raw TLS config. Only `xai-grok-net` and the
/// listed files may make them.
const RAW_ACCESS: &[&str] = &[
    "build_http_client(",
    "build_blocking_http_client(",
    "library_client(",
];

/// Files that pass the net client to a library that sends its own requests.
const LIBRARY_CLIENT_CALLERS: &[(&str, &str)] = &[];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates/codegen/xai-grok-net sits three levels under the root")
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for top in ["crates", "prod"] {
        walk(&root.join(top), &mut out);
    }
    out
}

fn crate_name(manifest: &toml::Table) -> Option<&str> {
    manifest.get("package")?.get("name")?.as_str()
}

/// The runtime dependency tables: `[dependencies]` and every `[target.*.dependencies]`.
fn runtime_dependencies(manifest: &toml::Table) -> Vec<&toml::Table> {
    let mut tables = Vec::new();
    if let Some(t) = manifest.get("dependencies").and_then(|v| v.as_table()) {
        tables.push(t);
    }
    if let Some(targets) = manifest.get("target").and_then(|v| v.as_table()) {
        for target in targets.values() {
            if let Some(t) = target.get("dependencies").and_then(|v| v.as_table()) {
                tables.push(t);
            }
        }
    }
    tables
}

#[test]
fn only_the_net_crate_links_an_http_client() {
    let mut violations = Vec::new();
    for path in files() {
        if path.file_name().is_none_or(|n| n != "Cargo.toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read manifest");
        let manifest: toml::Table = text.parse().expect("parse manifest");
        let Some(name) = crate_name(&manifest) else {
            continue;
        };
        if ALLOWED_CRATES.contains(&name) {
            continue;
        }
        for table in runtime_dependencies(&manifest) {
            for (dep, spec) in table {
                let package = spec
                    .get("package")
                    .and_then(|p| p.as_str())
                    .unwrap_or(dep.as_str());
                if CLIENT_LIBRARIES.contains(&package) {
                    violations.push(format!("{name} depends on {package}"));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "these crates can send HTTP around xai-grok-net, which owns every credential. \
         Depend on xai-grok-net instead:\n  {}",
        violations.join("\n  ")
    );
}

fn is_test_source(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/tests/")
        || s.contains("/benches/")
        || s.contains("/examples/")
        || s.ends_with("_tests.rs")
        || s.ends_with("/tests.rs")
        || s.ends_with("build.rs")
}

#[test]
fn only_the_net_crate_hands_out_a_raw_client() {
    let root = workspace_root();
    let net = root.join("crates/codegen/xai-grok-net");
    let extra_ca = root.join("crates/codegen/xai-grok-extra-ca");
    let mut violations = Vec::new();
    for path in files() {
        if path.extension().is_none_or(|e| e != "rs")
            || path.starts_with(&net)
            || path.starts_with(&extra_ca)
            || is_test_source(&path)
        {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for call in RAW_ACCESS {
            if !text.contains(call) {
                continue;
            }
            let listed = *call == "library_client("
                && LIBRARY_CLIENT_CALLERS.iter().any(|(file, _)| *file == rel);
            if !listed {
                violations.push(format!("{rel} calls {call}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a raw client skips the credential rules in xai-grok-net:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn every_listed_library_client_caller_exists() {
    let root = workspace_root();
    for (file, reason) in LIBRARY_CLIENT_CALLERS {
        let text = std::fs::read_to_string(root.join(file)).unwrap_or_default();
        assert!(
            text.contains("library_client("),
            "{file} is listed ({reason}) but no longer calls library_client"
        );
    }
}
