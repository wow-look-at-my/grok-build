pub(crate) mod lsp_runtime;

pub(crate) use xai_grok_shell_core::test_support::*;

/// Keep this crate's unit-test binary from writing synthetic events into the real unified log.
/// Runs pre-main so the redirect beats the lazily-opened writer.
/// Integration binaries under `tests/` isolate via `TestSandbox` homes instead.
#[ctor::ctor]
fn redirect_unified_log_for_tests() {
    xai_grok_telemetry::unified_log::redirect_to_temp_for_tests();
}
