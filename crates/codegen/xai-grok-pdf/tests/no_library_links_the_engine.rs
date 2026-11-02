//! Only binaries may depend on the PDF engine. A library that does makes every crate after it wait for `pdf_oxide`.

use std::process::Command;

const ENGINE_CRATES: [&str; 2] = ["pdf_oxide", "xai-grok-pdf"];

#[test]
fn no_library_depends_on_the_pdf_engine() {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
            manifest,
        ])
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata prints JSON");

    let mut offenders = Vec::new();
    for package in metadata["packages"].as_array().expect("packages array") {
        let name = package["name"].as_str().unwrap_or_default();
        if name == "xai-grok-pdf" {
            continue;
        }
        let has_lib = package["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| {
                t["kind"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|k| k == "lib")
            });
        if !has_lib {
            continue;
        }
        for dep in package["dependencies"].as_array().into_iter().flatten() {
            let dep_name = dep["name"].as_str().unwrap_or_default();
            if dep["kind"].is_null() && ENGINE_CRATES.contains(&dep_name) {
                offenders.push(format!("{name} -> {dep_name}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these libraries depend on the PDF engine; move the dependency to a binary \
         package that calls xai_grok_pdf::install(): {offenders:?}"
    );
}
