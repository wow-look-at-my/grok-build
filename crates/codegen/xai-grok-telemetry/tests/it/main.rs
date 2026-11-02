//! The telemetry integration tests. Each reads process-global state, so each
//! runs alone in a child process.

/// Runs the calling test alone in a child process of this binary, then returns.
/// A test that sets process-global state must not share its process.
macro_rules! own_process {
    ($test:ident) => {
        if $crate::rerun_in_own_process(concat!(module_path!(), "::", stringify!($test))) {
            return;
        }
    };
}

mod agent_id_prewarm;
mod external_otlp;
mod external_otlp_collector;
mod external_otlp_gates_on;
mod external_otlp_grpc;
mod external_otlp_grpc_tls;
mod external_otlp_guard;
mod external_otlp_mtls_grpc;
mod external_otlp_mtls_grpc_reject;
mod external_otlp_mtls_http;
mod external_otlp_session_ctx;
mod machine_id_off_boot_path;
mod manual_auth_emit;
mod otlp_collector;
mod process_snapshot;

const OWN_PROCESS_ENV: &str = "XAI_TEST_OWN_PROCESS";

/// Returns false in the child, which runs the test body. In the parent it
/// runs the child and fails unless the child ran and passed this test.
fn rerun_in_own_process(path: &str) -> bool {
    if std::env::var_os(OWN_PROCESS_ENV).is_some() {
        return false;
    }
    // libtest names a test without the crate name that `module_path!()` starts with.
    let test = path.split_once("::").map_or(path, |(_, rest)| rest);
    let exe = std::env::current_exe().expect("current test binary");
    let output = std::process::Command::new(exe)
        .args([
            test,
            "--exact",
            "--include-ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(OWN_PROCESS_ENV, "1")
        .output()
        .expect("spawn the test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "{test} failed in its own process: {}",
        output.status
    );
    true
}
