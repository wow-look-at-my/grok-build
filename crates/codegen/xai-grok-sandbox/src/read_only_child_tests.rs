use super::*;
use std::path::Path;

async fn sh(script: &str, dir: &Path) -> std::process::Output {
    let mut cmd =
        read_only_command(OsStr::new("/bin/sh")).expect("this host must have a write sandbox");
    cmd.arg("-c").arg(script).current_dir(dir);
    cmd.output().await.expect("spawn /bin/sh")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn a_confined_child_cannot_create_change_or_delete_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("kept"), "original\n").unwrap();

    let out = sh("printf x > created", tmp.path()).await;
    assert!(!out.status.success(), "creating a file must fail");
    assert!(
        !tmp.path().join("created").exists(),
        "the file must not exist"
    );

    let out = sh("printf changed > kept", tmp.path()).await;
    assert!(!out.status.success(), "overwriting a file must fail");
    let out = sh("printf more >> kept", tmp.path()).await;
    assert!(!out.status.success(), "appending to a file must fail");
    let out = sh("rm kept", tmp.path()).await;
    assert!(!out.status.success(), "deleting a file must fail");
    let out = sh("mkdir sub", tmp.path()).await;
    assert!(!out.status.success(), "creating a directory must fail");
    let out = sh("mv kept moved", tmp.path()).await;
    assert!(!out.status.success(), "renaming a file must fail");

    assert_eq!(
        std::fs::read_to_string(tmp.path().join("kept")).unwrap(),
        "original\n",
        "the file must be unchanged"
    );
    assert!(!tmp.path().join("sub").exists());
    assert!(!tmp.path().join("moved").exists());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn a_confined_child_can_read_and_write_dev_null() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("readme"), "hello\n").unwrap();

    let out = sh("cat readme && printf discarded > /dev/null", tmp.path()).await;
    assert!(
        out.status.success(),
        "read and /dev/null write must work: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[test]
fn a_platform_with_no_sandbox_refuses_to_build_the_command() {
    assert!(read_only_command(OsStr::new("git")).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn every_write_right_of_the_abi_is_handled() {
    assert_eq!(imp::write_rights(1).count_ones(), 10);
    assert_eq!(imp::write_rights(2).count_ones(), 11, "ABI 2 adds REFER");
    assert_eq!(imp::write_rights(3).count_ones(), 12, "ABI 3 adds TRUNCATE");
    assert_eq!(imp::write_rights(6), imp::write_rights(3));
}
