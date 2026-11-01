//! A child process that can read the whole filesystem and write none of it.
use std::ffi::OsStr;

/// The reason a write-confined child cannot start on this host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no kernel write sandbox for a child process: {0}")]
pub struct ReadOnlyUnavailable(pub String);

/// A command for `program` that the kernel stops from writing to any file but
/// `/dev/null`.
pub fn read_only_command(program: &OsStr) -> Result<tokio::process::Command, ReadOnlyUnavailable> {
    imp::read_only_command(program)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::ReadOnlyUnavailable;
    use std::ffi::OsStr;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    // Kernel UAPI, `include/uapi/linux/landlock.h`. libc carries only the syscall numbers.
    const CREATE_RULESET_VERSION: libc::c_uint = 1;
    const RULE_PATH_BENEATH: libc::c_uint = 1;
    const FS_WRITE_FILE: u64 = 1 << 1;
    const FS_REMOVE_DIR: u64 = 1 << 4;
    const FS_REMOVE_FILE: u64 = 1 << 5;
    const FS_MAKE_CHAR: u64 = 1 << 6;
    const FS_MAKE_DIR: u64 = 1 << 7;
    const FS_MAKE_REG: u64 = 1 << 8;
    const FS_MAKE_SOCK: u64 = 1 << 9;
    const FS_MAKE_FIFO: u64 = 1 << 10;
    const FS_MAKE_BLOCK: u64 = 1 << 11;
    const FS_MAKE_SYM: u64 = 1 << 12;
    const FS_REFER: u64 = 1 << 13;
    const FS_TRUNCATE: u64 = 1 << 14;

    /// The kernel accepts it on every ABI.
    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    /// Every right that changes the filesystem, for this kernel's ABI. A right
    /// the ruleset does not handle stays allowed, so reads and exec are open.
    pub(super) fn write_rights(abi: i64) -> u64 {
        let mut rights = FS_WRITE_FILE
            | FS_REMOVE_DIR
            | FS_REMOVE_FILE
            | FS_MAKE_CHAR
            | FS_MAKE_DIR
            | FS_MAKE_REG
            | FS_MAKE_SOCK
            | FS_MAKE_FIFO
            | FS_MAKE_BLOCK
            | FS_MAKE_SYM;
        if abi >= 2 {
            rights |= FS_REFER;
        }
        if abi >= 3 {
            rights |= FS_TRUNCATE;
        }
        rights
    }

    fn os_error(what: &str) -> ReadOnlyUnavailable {
        ReadOnlyUnavailable(format!("{what}: {}", std::io::Error::last_os_error()))
    }

    /// Build the ruleset in the parent. The child only has to call
    /// `landlock_restrict_self`, which is async-signal-safe.
    fn build_ruleset() -> Result<OwnedFd, ReadOnlyUnavailable> {
        // SAFETY: a version query passes a null attr and the first size, as the UAPI documents.
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if abi < 1 {
            return Err(os_error("Landlock is not available on this kernel"));
        }
        let handled = write_rights(abi);
        let attr = RulesetAttr {
            handled_access_fs: handled,
        };
        // SAFETY: `attr` is a valid one ABI ruleset attr and the size names exactly it.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(os_error("landlock_create_ruleset failed"));
        }
        // SAFETY: the kernel returned a new file descriptor that nothing else owns.
        let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        allow_dev_null_writes(&ruleset, handled)?;
        Ok(ruleset)
    }

    fn allow_dev_null_writes(ruleset: &OwnedFd, handled: u64) -> Result<(), ReadOnlyUnavailable> {
        let dev_null = std::fs::File::open("/dev/null")
            .map_err(|e| ReadOnlyUnavailable(format!("open /dev/null: {e}")))?;
        let rule = PathBeneathAttr {
            allowed_access: handled & (FS_WRITE_FILE | FS_TRUNCATE),
            parent_fd: dev_null.as_raw_fd(),
        };
        // SAFETY: `rule` is a valid path-beneath attr whose fd stays open for the call.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                RULE_PATH_BENEATH,
                &rule as *const PathBeneathAttr,
                0 as libc::c_uint,
            )
        };
        if rc != 0 {
            return Err(os_error("landlock_add_rule for /dev/null failed"));
        }
        Ok(())
    }

    pub(super) fn read_only_command(
        program: &OsStr,
    ) -> Result<tokio::process::Command, ReadOnlyUnavailable> {
        let ruleset = build_ruleset()?;
        let mut cmd = tokio::process::Command::new(program);
        // SAFETY: the closure makes raw syscalls and allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::syscall(
                    libc::SYS_landlock_restrict_self,
                    ruleset.as_raw_fd(),
                    0 as libc::c_uint,
                ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(cmd)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::ReadOnlyUnavailable;
    use std::ffi::OsStr;

    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

    /// Last match wins in SBPL, so the `/dev/null` grant comes after the deny.
    pub(super) const PROFILE: &str = "(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write* (literal \"/dev/null\"))\n";

    pub(super) fn read_only_command(
        program: &OsStr,
    ) -> Result<tokio::process::Command, ReadOnlyUnavailable> {
        if !std::path::Path::new(SANDBOX_EXEC).exists() {
            return Err(ReadOnlyUnavailable(format!("{SANDBOX_EXEC} is missing")));
        }
        let mut cmd = tokio::process::Command::new(SANDBOX_EXEC);
        cmd.arg("-p").arg(PROFILE).arg(program);
        Ok(cmd)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use super::ReadOnlyUnavailable;
    use std::ffi::OsStr;

    pub(super) fn read_only_command(
        _program: &OsStr,
    ) -> Result<tokio::process::Command, ReadOnlyUnavailable> {
        Err(ReadOnlyUnavailable(
            "this platform has no write sandbox for a child process".to_owned(),
        ))
    }
}

#[cfg(test)]
#[path = "read_only_child_tests.rs"]
mod tests;
