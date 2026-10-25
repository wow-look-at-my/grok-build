//! A writer outside this process's sandbox, for the config files grok edits about itself.

use std::io;
use std::path::Path;
use std::sync::OnceLock;

/// Writes `contents` to `path` with `mode`. `None` gives the write back to the caller.
pub type HostWriter = fn(&Path, &str, Option<u32>) -> Option<io::Result<()>>;

static WRITER: OnceLock<HostWriter> = OnceLock::new();

/// Install the process-wide host writer. Only the first install takes effect.
pub fn install(writer: HostWriter) {
    let _ = WRITER.set(writer);
}

/// Give the write to the installed host writer. `None` means no writer took
/// it, and the caller must write the file itself.
pub fn try_write(path: &Path, contents: &str, mode: Option<u32>) -> Option<io::Result<()>> {
    WRITER.get()?(path, contents, mode)
}
