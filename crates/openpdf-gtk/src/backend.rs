// SPDX-License-Identifier: MPL-2.0
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Size of a page in PDF points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f64,
    pub height: f64,
}

impl PageSize {
    pub const A4: Self = Self {
        width: 595.0,
        height: 842.0,
    };
}

/// One tile of a page, RGBA8, row-major, unpremultiplied.
#[derive(Debug, Clone)]
pub struct TileBitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl TileBitmap {
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TileRequest {
    pub page: u32,
    pub zoom: f64,
    pub tx: u32,
    pub ty: u32,
    pub tile_size: u32,
}

#[derive(Debug)]
pub enum BackendError {
    Io(std::io::Error),
    Invalid(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(
                f,
                "{}",
                openpdf_core::tr_args(
                    "Could not read the file: {detail}",
                    &[("detail", &e.to_string())]
                )
            ),
            Self::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<std::io::Error> for BackendError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Document surface used by [`crate::backend::MockDocument`] for tests and
/// `--measure`. Live viewing goes through `openpdf-core::{Engine, Document}`.
pub trait DocumentBackend: Send + Sync {
    fn title(&self) -> &str;
    fn path(&self) -> Option<&Path>;
    fn page_count(&self) -> usize;
    fn page_size(&self, page: u32) -> Result<PageSize, BackendError>;
    fn render_tile(&self, req: TileRequest) -> Result<TileBitmap, BackendError>;
    fn extract_text(&self, page: u32) -> Result<String, BackendError>;
    /// True when tiles are a stand-in, not PDFium output.
    fn is_placeholder(&self) -> bool {
        false
    }
}

/// Local stand-in for tests, `--measure`, and the rare case that PDFium is
/// missing. Live documents go through `openpdf-core::Engine`.
#[derive(Debug)]
pub struct MockDocument {
    title: String,
    path: Option<PathBuf>,
    pages: usize,
    sizes: Vec<PageSize>,
}

impl MockDocument {
    pub fn sample(pages: usize) -> Arc<Self> {
        let pages = pages.max(1);
        Arc::new(Self {
            title: format!("Sample ({pages} pages)"),
            path: None,
            pages,
            sizes: vec![PageSize::A4; pages],
        })
    }

    pub fn open(path: &Path) -> Result<Arc<Self>, BackendError> {
        if !path.exists() {
            return Err(BackendError::Invalid(openpdf_core::tr_args(
                "file not found: {path}",
                &[("path", &path.display().to_string())],
            )));
        }
        let bytes = std::fs::read(path)?;
        let pages = peek_pdf_page_count(&bytes).unwrap_or(1).max(1);
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        Ok(Arc::new(Self {
            title,
            path: Some(path.to_path_buf()),
            pages,
            sizes: vec![PageSize::A4; pages],
        }))
    }
}

impl DocumentBackend for MockDocument {
    fn title(&self) -> &str {
        &self.title
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn page_count(&self) -> usize {
        self.pages
    }

    fn page_size(&self, page: u32) -> Result<PageSize, BackendError> {
        self.sizes.get(page as usize).copied().ok_or_else(|| {
            BackendError::Invalid(openpdf_core::tr_args(
                "page {page} is out of range ({count} pages)",
                &[
                    ("page", &page.to_string()),
                    ("count", &self.pages.to_string()),
                ],
            ))
        })
    }

    fn render_tile(&self, req: TileRequest) -> Result<TileBitmap, BackendError> {
        let size = self.page_size(req.page)?;
        render_placeholder_tile(&self.title, self.pages, size, req)
    }

    fn extract_text(&self, page: u32) -> Result<String, BackendError> {
        if page as usize >= self.pages {
            return Err(BackendError::Invalid(openpdf_core::tr_args(
                "page {page} is out of range ({count} pages)",
                &[
                    ("page", &page.to_string()),
                    ("count", &self.pages.to_string()),
                ],
            )));
        }
        Ok(format!(
            "Page {} of {}\n{}\nopenPDF placeholder renderer",
            page + 1,
            self.pages,
            self.title
        ))
    }

    fn is_placeholder(&self) -> bool {
        true
    }
}

/// Count `/Type /Page` objects minus `/Type /Pages` trees. Good enough to
/// size the UI before core parses the document for real.
pub fn peek_pdf_page_count(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 5 || &bytes[..5] != b"%PDF-" {
        return None;
    }
    let text = String::from_utf8_lossy(bytes);
    let mut pages = 0usize;
    let mut trees = 0usize;
    for token in [" /Type /Page", "/Type /Page", "/Type/Page"] {
        pages += text.matches(token).count();
    }
    for token in [" /Type /Pages", "/Type /Pages", "/Type/Pages"] {
        trees += text.matches(token).count();
    }
    // `/Type /Pages` also contains the prefix `/Type /Page`.
    let n = pages.saturating_sub(trees);
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

fn render_placeholder_tile(
    title: &str,
    page_count: usize,
    page: PageSize,
    req: TileRequest,
) -> Result<TileBitmap, BackendError> {
    let zoom = req.zoom.max(0.05);
    let pw = ((page.width * zoom).ceil() as u32).max(1);
    let ph = ((page.height * zoom).ceil() as u32).max(1);
    let x0 = req.tx * req.tile_size;
    let y0 = req.ty * req.tile_size;
    if x0 >= pw || y0 >= ph {
        return Err(BackendError::Invalid(openpdf_core::tr("tile outside page")));
    }
    let tw = (req.tile_size).min(pw - x0);
    let th = (req.tile_size).min(ph - y0);

    let mut pixels = vec![0u8; tw as usize * th as usize * 4];
    let header = ((56.0 * zoom).round() as u32).clamp(8, 80);
    let accent = (14u8, 124u8, 102u8);

    for y in 0..th {
        for x in 0..tw {
            let px = x0 + x;
            let py = y0 + y;
            let i = (y as usize * tw as usize + x as usize) * 4;
            let mut r = 250u8;
            let mut g = 250u8;
            let mut b = 248u8;
            if py < header {
                r = accent.0;
                g = accent.1;
                b = accent.2;
            }
            // Tile grid so lazy loading is visible while core is missing.
            if px.is_multiple_of(req.tile_size) || py.is_multiple_of(req.tile_size) {
                r = r.saturating_sub(18);
                g = g.saturating_sub(18);
                b = b.saturating_sub(18);
            }
            // Page number block near the centre of the page.
            let cx = pw / 2;
            let cy = ph / 2;
            let digit_w = (pw / 8).max(24);
            let digit_h = (ph / 10).max(32);
            if px + digit_w > cx
                && px < cx + digit_w
                && py + digit_h > cy
                && py < cy + digit_h
                && py >= header
            {
                r = 40;
                g = 40;
                b = 42;
            }
            let edge = 2u32.saturating_add((zoom * 1.5) as u32);
            if px < edge || py < edge || px + edge >= pw || py + edge >= ph {
                r = 190;
                g = 188;
                b = 182;
            }
            pixels[i] = r;
            pixels[i + 1] = g;
            pixels[i + 2] = b;
            pixels[i + 3] = 255;
        }
    }

    // Keep the title/page in the type system so a future real renderer
    // can stamp the same metadata; the bitmap itself stays cheap.
    let _ = (title, page_count, req.page);
    Ok(TileBitmap {
        width: tw,
        height: th,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_page_count() {
        let doc = MockDocument::sample(12);
        assert_eq!(doc.page_count(), 12);
        assert_eq!(doc.page_size(0).unwrap(), PageSize::A4);
        assert!(doc.page_size(12).is_err());
        assert!(doc.is_placeholder());
    }

    #[test]
    fn tile_edges_are_clipped() {
        let doc = MockDocument::sample(1);
        let req = TileRequest {
            page: 0,
            zoom: 1.0,
            tx: 2,
            ty: 3,
            tile_size: 256,
        };
        let tile = doc.render_tile(req).unwrap();
        assert!(tile.width <= 256);
        assert!(tile.height <= 256);
        assert_eq!(
            tile.pixels.len(),
            tile.width as usize * tile.height as usize * 4
        );
    }

    #[test]
    fn extract_text_names_page() {
        let doc = MockDocument::sample(3);
        let t = doc.extract_text(1).unwrap();
        assert!(t.contains("Page 2 of 3"));
        assert!(doc.extract_text(9).is_err());
    }

    #[test]
    fn peek_minimal_pdf() {
        let pdf = b"%PDF-1.4\n1 0 obj<</Type /Catalog /Pages 2 0 R>>endobj\n\
            2 0 obj<</Type /Pages /Count 2 /Kids [3 0 R 4 0 R]>>endobj\n\
            3 0 obj<</Type /Page /Parent 2 0 R /MediaBox [0 0 595 842]>>endobj\n\
            4 0 obj<</Type /Page /Parent 2 0 R /MediaBox [0 0 595 842]>>endobj\n";
        assert_eq!(peek_pdf_page_count(pdf), Some(2));
        assert_eq!(peek_pdf_page_count(b"not a pdf"), None);
    }

    #[test]
    fn open_missing_file_errors() {
        let err = MockDocument::open(Path::new("/no/such/openpdf-test.pdf")).unwrap_err();
        assert!(matches!(err, BackendError::Invalid(_)));
    }
}
