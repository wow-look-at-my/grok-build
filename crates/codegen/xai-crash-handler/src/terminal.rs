//! Terminal restore sequences for signal handler context.

// ----------------------------------------------------------------------- Canonical list.

/// Raw CSI sequences to disable every mouse-tracking mode the pager enables (`?1000/?1002/?1003/?1015/?1006`) — the mouse subset.
pub const MOUSE_TRACKING_RESET: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1015l\x1b[?1006l";

/// Raw CSI sequences to disable mouse tracking and bracketed paste.
pub const MOUSE_PASTE_RESET: &[u8] =
    b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1015l\x1b[?1006l\x1b[?2004l";

/// Full escape sequence to restore the terminal to a sane state.
/// The kitty CSI-u pop precedes `?1049l` per spec (the protocol stack is per-screen).
pub const RESTORE_SEQ: &[u8] =
    b"\x1b[?2026l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1015l\x1b[?1006l\x1b[?2004l\x1b[?1004l\x1b[<u\x1b[?1049l";

/// Write terminal restore sequences to stderr using raw `libc::write`.
#[cfg(unix)]
pub fn restore_in_signal_handler() {
    unsafe {
        libc::write(
            2, // stderr
            RESTORE_SEQ.as_ptr() as *const libc::c_void,
            RESTORE_SEQ.len(),
        );
    }
}

#[cfg(windows)]
pub fn restore_in_signal_handler() {
    unsafe {
        let stderr = windows_sys::Win32::System::Console::GetStdHandle(
            windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
        );
        if !stderr.is_null() && stderr != -1isize as *mut std::ffi::c_void {
            let mut written: u32 = 0;
            windows_sys::Win32::Storage::FileSystem::WriteFile(
                stderr,
                RESTORE_SEQ.as_ptr(),
                RESTORE_SEQ.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            );
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub fn restore_in_signal_handler() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn position_of(needle: &[u8]) -> usize {
        RESTORE_SEQ
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap_or_else(|| {
                panic!(
                    "RESTORE_SEQ must contain {:?}",
                    std::str::from_utf8(needle).unwrap_or("<binary>")
                )
            })
    }

    #[test]
    fn restore_seq_pops_kitty_before_alt_screen_leave() {
        assert!(position_of(b"\x1b[<u") < position_of(b"\x1b[?1049l"));
    }

    #[test]
    fn restore_seq_includes_all_modes() {
        for needle in [
            b"\x1b[?2026l".as_slice(),
            b"\x1b[?25h".as_slice(),
            b"\x1b[?1000l".as_slice(),
            b"\x1b[?1002l".as_slice(),
            b"\x1b[?1003l".as_slice(),
            b"\x1b[?1015l".as_slice(),
            b"\x1b[?1006l".as_slice(),
            b"\x1b[?2004l".as_slice(),
            b"\x1b[?1004l".as_slice(),
            b"\x1b[<u".as_slice(),
            b"\x1b[?1049l".as_slice(),
        ] {
            position_of(needle);
        }
    }

    #[test]
    fn restore_seq_ends_synchronized_update_first() {
        // Multiplexers (zellij/tmux) must stop buffering before subsequent resets arrive.
        let end_sync = b"\x1b[?2026l";
        assert_eq!(RESTORE_SEQ.get(..end_sync.len()), Some(end_sync.as_slice()));
    }
}
