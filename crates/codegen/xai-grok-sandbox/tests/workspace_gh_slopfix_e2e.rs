//! Runs `gh` and `slopfix fix` inside the real `--sandbox=workspace` profile,
//! in the order a grok session starts.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const MODE_ENV: &str = "GROK_WORKSPACE_E2E_MODE";
const WORKSPACE_ENV: &str = "GROK_WORKSPACE_E2E_WORKSPACE";
const TEST_NAME: &str = "gh_and_slopfix_work_inside_the_workspace_profile";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--list") {
        if !args.iter().any(|arg| arg == "--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    match std::env::var(MODE_ENV).as_deref() {
        Ok("child") => child(),
        _ => parent(),
    }
}

fn on_path(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{name} must be on PATH: this test runs it in the sandbox"))
}

fn executable(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn parent() {
    on_path("api-cli");
    on_path("slopfix");
    let root = Path::new("/tmp").join(format!("gws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let grok_home = root.join("home");
    let workspace = root.join("ws");
    let host_bin = root.join("hostbin");
    let gh_config = root.join("ghcfg");
    for dir in [&grok_home, &workspace, &host_bin, &gh_config] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let root = dunce::canonicalize(&root).unwrap();
    let workspace = root.join("ws");
    std::fs::write(
        gh_config.join("hosts.yml"),
        "github.com:\n    oauth_token: on-disk-secret\n",
    )
    .unwrap();
    executable(
        &host_bin.join("gh"),
        "#!/bin/sh\nif [ \"$1 $2\" = 'api graphql' ]; then echo '{\"fork\":false,\"parent\":null}'; exit 0; fi\necho \"argv: $*\"\necho \"token: ${GH_TOKEN:-none}\"\n",
    );
    std::fs::write(
        workspace.join("doc.md"),
        "# Probe\n\nThis paragraph is wrapped\nover a line break.\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&workspace)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "https://github.com/o/r.git"]);

    let path = format!(
        "{}:{}",
        host_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .env(MODE_ENV, "child")
        .env(WORKSPACE_ENV, &workspace)
        .env("GROK_HOME", &grok_home)
        .env("GH_CONFIG_DIR", &gh_config)
        .env("GH_TOKEN", "host-only")
        .env("GITHUB_TOKEN", "host-only")
        .env("PATH", path)
        .current_dir(&workspace)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let doc = std::fs::read_to_string(workspace.join("doc.md")).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let report = format!(
        "status={}\nstdout={stdout}\nstderr={stderr}\ndoc={doc}",
        output.status
    );
    assert!(output.status.success(), "{report}");
    for expected in [
        "inner_token=none",
        "hosts_denied",
        "argv: pr view 7 --json=title",
        "token: host-only",
        "slopfix_rc=0",
    ] {
        assert!(stdout.contains(expected), "missing '{expected}'\n{report}");
    }
    for refused in ["merge_rc=", "mutation_rc="] {
        let line = stdout
            .lines()
            .find(|line| line.starts_with(refused))
            .unwrap_or_else(|| panic!("no {refused} line\n{report}"));
        assert_ne!(line, format!("{refused}0"), "{report}");
    }
    assert!(!stdout.contains("argv: pr merge"), "{report}");
    assert!(
        doc.contains("This paragraph is wrapped over a line break."),
        "slopfix fix did not repair doc.md inside the sandbox\n{report}"
    );
    println!("{TEST_NAME}: ok");
}

fn child() {
    let workspace = PathBuf::from(std::env::var(WORKSPACE_ENV).unwrap());
    let profile = xai_grok_sandbox::ProfileName::Workspace;
    #[cfg(target_os = "linux")]
    if !xai_grok_sandbox::is_inside_bwrap() {
        xai_grok_sandbox::gh_shim::start();
        let mut cmd = xai_grok_sandbox::bwrap_reexec_for_profile(&profile, &workspace)
            .expect("the workspace profile re-execs into bwrap to deny reads");
        use std::os::unix::process::CommandExt as _;
        let error = cmd.exec();
        panic!("bwrap exec failed: {error}");
    }
    #[cfg(not(target_os = "linux"))]
    xai_grok_sandbox::gh_shim::start();

    xai_grok_sandbox::jail::redirect_caches_in_place();
    let mut sandbox = xai_grok_sandbox::SandboxManager::new(profile, &workspace);
    sandbox
        .apply(&workspace)
        .expect("apply the workspace profile");
    assert!(
        sandbox.is_applied(),
        "the workspace profile must be in force"
    );
    sandbox.install();

    let script = "\
echo \"inner_token=${GH_TOKEN:-none}${GITHUB_TOKEN:-}\"
cat \"$GH_CONFIG_DIR/hosts.yml\" >/dev/null 2>&1 && echo HOSTS_READABLE || echo hosts_denied
gh pr view 7 --json title
gh pr merge 7; echo \"merge_rc=$?\"
gh api graphql -f 'query=mutation { addStar(input: {starrableId: \"x\"}) { clientMutationId } }'; echo \"mutation_rc=$?\"
slopfix fix doc.md; echo \"slopfix_rc=$?\"
";
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(xai_grok_sandbox::gh_shim::prefix_shell_command(script))
        .current_dir(&workspace)
        .status()
        .unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
