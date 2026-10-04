//! Reproducible harness for benchmarking clipboard attachment reads (the
//! paste hot path).
//!
//! Runs one `get_attachments()`, the same probe the pager's paste pipeline
//! executes, and prints the outcome plus wall time. Benchmark the native
//! in-process read against the `osascript` fallback with hyperfine:
//!

fn main() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let attachments = xai_grok_shared::clipboard::get_attachments()?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1e3;

    let image = attachments
        .image
        .map(|img| format!("{} ({} bytes)", img.mime_type, img.data.len()));
    let file_urls = attachments
        .file_urls
        .map(|urls| format!("{} path(s)", urls.lines().count()));
    println!(
        "{elapsed_ms:.1} ms  image={}  file_urls={}",
        image.as_deref().unwrap_or("none"),
        file_urls.as_deref().unwrap_or("none"),
    );
    Ok(())
}
