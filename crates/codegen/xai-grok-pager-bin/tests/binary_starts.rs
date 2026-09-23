//! This test makes `cargo test --workspace` build the real binary.
#[test]
fn the_binary_reports_its_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_xai-grok-pager"))
        .arg("--version")
        .output()
        .expect("spawn the pager binary");
    assert!(
        out.status.success(),
        "--version failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "--version printed nothing to stdout"
    );
}
