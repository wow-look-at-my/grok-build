//! The sampler's integration tests, one module per area.

/// Runs the calling test alone in a child process of this binary, then returns.
/// A test that sets process-global state must not share its process.
macro_rules! own_process {
    ($test:ident) => {
        if $crate::rerun_in_own_process(concat!(module_path!(), "::", stringify!($test))) {
            return;
        }
    };
}

mod backend_detect;
mod cf_edge_error_message;
mod prewarm;
mod request_query_and_headers;
mod request_slots;
mod shared_http_kill_switch;
mod shared_http_wire;
mod support;
mod test_actor;
mod usage_sink;

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
