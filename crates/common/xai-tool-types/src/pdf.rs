//! The seam between the read tools and the PDF engine.

use std::sync::OnceLock;

/// One open PDF document.
pub trait PdfDocument: Send {
    /// The number of pages in the document.
    fn page_count(&self) -> Result<usize, String>;
    /// The text of the 0-based page `page`.
    fn extract_text(&self, page: usize) -> Result<String, String>;
    /// The 0-based page `page` as a JPEG at `dpi`, with JPEG `quality`.
    fn render_jpeg(&self, page: usize, dpi: u32, quality: u8) -> Result<Vec<u8>, String>;
}

/// Parses PDF bytes into a document.
pub type PdfOpenFn = fn(Vec<u8>) -> Result<Box<dyn PdfDocument>, String>;

static PDF_BACKEND: OnceLock<PdfOpenFn> = OnceLock::new();

/// Sets the PDF engine for this process. A second call keeps the first engine.
pub fn install_pdf_backend(open: PdfOpenFn) {
    let _ = PDF_BACKEND.set(open);
}

/// Opens `bytes` with the installed engine.
pub fn open_pdf(bytes: Vec<u8>) -> Result<Box<dyn PdfDocument>, String> {
    match PDF_BACKEND.get() {
        Some(open) => open(bytes),
        None => Err("PDF support is not installed in this binary: it must call \
             xai_grok_pdf::install() at startup"
            .to_string()),
    }
}
