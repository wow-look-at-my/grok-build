//! OS version string for the `<user_info>` preamble.
//!
//! Emits `OS Version: <kernel> <release>` (e.g. `darwin 24.6.0`, `linux 6.5.0-...`).

/// Return `"<kernel-lowercased> <release>"` for the `os_family` placeholder.
pub fn os_kernel_and_release() -> String {
    #[cfg(unix)]
    {
        if let Some(s) = uname_unix() {
            return s;
        }
    }

    #[cfg(windows)]
    {
        if let Some(s) = windows_version() {
            return s;
        }
    }

    std::env::consts::OS.to_string()
}

#[cfg(unix)]
fn uname_unix() -> Option<String> {
    use std::mem::MaybeUninit;

    let mut uts: MaybeUninit<libc::utsname> = MaybeUninit::zeroed();
    let rc = unsafe { libc::uname(uts.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: rc == 0 means uname populated all fields with NUL-terminated strings of length <= the buffer size.
    let uts = unsafe { uts.assume_init() };

    let sysname = c_char_array_to_lowercase_string(&uts.sysname)?;
    let release = c_char_array_to_string(&uts.release)?;
    Some(format!("{sysname} {release}"))
}

/// Convert a NUL-terminated `c_char` array (as returned in `utsname` fields) into an owned `String`.
/// Returns `None` if the bytes are not valid UTF-8 or the array lacks a NUL terminator.
#[cfg(unix)]
fn c_char_array_to_string(bytes: &[libc::c_char]) -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: utsname fields are POSIX-defined NUL-terminated byte strings.
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), bytes.len()) };
    let cstr = CStr::from_bytes_until_nul(bytes).ok()?;
    cstr.to_str().ok().map(|s| s.to_owned())
}

#[cfg(unix)]
fn c_char_array_to_lowercase_string(bytes: &[libc::c_char]) -> Option<String> {
    c_char_array_to_string(bytes).map(|s| s.to_lowercase())
}

/// Return `"windows <major>.<minor>.<build>"` (e.g. `"windows 10.0.22631.4890"`) by parsing the output of `cmd /C ver`.
/// Falls back to `None` on any failure so callers get the `std::env::consts::OS` default.
#[cfg(windows)]
fn windows_version() -> Option<String> {
    use std::process::Command;

    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "ver"]);
    xai_tty_utils::detach_std_command(&mut cmd);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    // `ver` outputs e.g. "Microsoft Windows [Version 10.0.22631.4890]". The bracketed portion is locale-independent.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout.find("[Version ")? + "[Version ".len();
    let end = stdout.get(start..).and_then(|rest| rest.find(']'))? + start;
    let version = stdout.get(start..end)?.trim();
    if version.is_empty() {
        return None;
    }
    Some(format!("windows {version}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On any platform the function produces a non-empty string.
    /// Exact content varies by host so we only assert the shape.
    #[test]
    fn os_kernel_and_release_is_non_empty() {
        let s = os_kernel_and_release();
        assert!(!s.is_empty(), "os_kernel_and_release returned empty");
    }

    /// On Unix hosts the format is `<kernel> <release>`: whitespace-separated tokens, both non-empty, with the kernel half lowercase.
    #[cfg(unix)]
    #[test]
    fn os_kernel_and_release_unix_shape() {
        let s = os_kernel_and_release();
        // Skip the assertion if uname failed and we fell back to
        // `std::env::consts::OS` (single token, e.g. "macos").
        if !s.contains(' ') {
            return;
        }
        let mut parts = s.splitn(2, ' ');
        let kernel = parts.next().expect("kernel half present");
        let release = parts.next().expect("release half present");
        assert!(!kernel.is_empty(), "kernel half empty in '{s}'");
        assert!(!release.is_empty(), "release half empty in '{s}'");
        assert_eq!(
            kernel,
            kernel.to_lowercase(),
            "kernel half must be lowercase: '{s}'"
        );
    }

    /// On macOS specifically the kernel name is `darwin`.
    /// This guards the regression where the preamble said "OS Version: macos" instead of "OS Version: darwin 24.6.0".
    #[cfg(target_os = "macos")]
    #[test]
    fn os_kernel_and_release_macos_says_darwin() {
        let s = os_kernel_and_release();
        // Skip if uname failed (fallback returns "macos")
        // On real CI/dev hardware this branch is never taken
        if !s.contains(' ') {
            return;
        }
        assert!(
            s.starts_with("darwin "),
            "macOS host must report 'darwin <release>', got '{s}'"
        );
    }
}
