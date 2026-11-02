//! The `pdf_oxide` engine for `xai_tool_types::pdf`.

use xai_tool_types::pdf::{PdfDocument, install_pdf_backend};

struct OxideDocument(pdf_oxide::PdfDocument);

impl PdfDocument for OxideDocument {
    fn page_count(&self) -> Result<usize, String> {
        self.0.page_count().map_err(|e| e.to_string())
    }

    fn extract_text(&self, page: usize) -> Result<String, String> {
        self.0.extract_text(page).map_err(|e| e.to_string())
    }

    fn render_jpeg(&self, page: usize, dpi: u32, quality: u8) -> Result<Vec<u8>, String> {
        let opts = pdf_oxide::rendering::RenderOptions::with_dpi(dpi).as_jpeg(quality);
        pdf_oxide::rendering::render_page(&self.0, page, &opts)
            .map(|image| image.data)
            .map_err(|e| e.to_string())
    }
}

fn open(bytes: Vec<u8>) -> Result<Box<dyn PdfDocument>, String> {
    pdf_oxide::PdfDocument::from_bytes(bytes)
        .map(|doc| Box::new(OxideDocument(doc)) as Box<dyn PdfDocument>)
        .map_err(|e| e.to_string())
}

/// Makes `pdf_oxide` the PDF engine of this process.
pub fn install() {
    install_pdf_backend(open);
}
