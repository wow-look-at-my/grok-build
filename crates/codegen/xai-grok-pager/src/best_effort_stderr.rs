
use std::io::Write;

/// Write `line` and a newline to `w`; `false` when the write failed.
pub fn write_line(w: &mut impl Write, line: &str) -> bool {
    writeln!(w, "{line}").is_ok()
}

/// [`write_line`] to process stderr, outcome discarded.
pub fn eprint_line(line: &str) {
    write_line(&mut std::io::stderr(), line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_line_appends_a_newline_and_reports_success() {
        let mut buf = Vec::new();
        assert!(write_line(&mut buf, "Finishing session…"));
        assert_eq!(buf, "Finishing session…\n".as_bytes());
    }

    /// Closing the read end makes every write fail with EPIPE, like the dead tty a closed pane
    /// leaves behind. SIGPIPE is ignored in Rust binaries, so the write returns an error.
    #[test]
    fn write_line_reports_failure_on_a_dead_pipe() {
        let (reader, mut writer) = std::io::pipe().expect("pipe");
        drop(reader);
        assert!(!write_line(&mut writer, "unreachable"));
    }
}
