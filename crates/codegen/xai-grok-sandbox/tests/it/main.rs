//! The sandbox's libtest integration tests. A re-exec names a test by module path.

/// Runs the calling test alone in a child process of this binary, then returns.
/// A test that sets process-global state must not share its process.
#[allow(unused_macros, reason = "only the Linux tests call it")]
macro_rules! own_process {
    ($test:ident) => {
        if $crate::rerun_in_own_process(concat!(module_path!(), "::", stringify!($test))) {
            return;
        }
    };
}

mod child_net_e2e;
mod deny_paths_e2e;
mod gh_contract_e2e;
mod integration_test;
mod jail_bwrap_e2e;
mod jail_config_e2e;
mod jail_gh_shim_e2e;
mod jail_grok_home_e2e;
mod jail_seatbelt_e2e;
mod read_write_trailing_glob_e2e;

const OWN_PROCESS_ENV: &str = "XAI_TEST_OWN_PROCESS";

/// Returns false in the child, which runs the test body. In the parent it
/// runs the child and fails unless the child ran and passed this test.
#[allow(dead_code, reason = "only the Linux tests call it")]
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
