//! Compare two PDFs: per-page text diff plus pixel diff.

use crate::document::Document;
use crate::error::Result;
use crate::render::Engine;

/// One page's text and raster comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct PageDiff {
    pub page: u32,
    pub text_equal: bool,
    pub pixel_equal: bool,
    pub changed_pixels: u32,
    pub total_pixels: u32,
}

/// Full-document comparison (read-only; allowed on signed files).
#[derive(Clone, Debug, PartialEq)]
pub struct CompareReport {
    pub left_pages: u32,
    pub right_pages: u32,
    pub pages: Vec<PageDiff>,
    pub identical: bool,
}

/// Compare text (PDFium) and pixels at `zoom`. Page-count mismatch is a difference.
pub fn compare_pdfs(
    engine: &Engine,
    left: &Document,
    right: &Document,
    zoom: f32,
) -> Result<CompareReport> {
    engine.bind(left)?;
    engine.bind(right)?;
    let left_n = left.page_count();
    let right_n = right.page_count();
    let n = left_n.max(right_n);
    let mut pages = Vec::with_capacity(n as usize);
    let mut identical = left_n == right_n;
    for i in 0..n {
        if i >= left_n || i >= right_n {
            identical = false;
            pages.push(PageDiff {
                page: i,
                text_equal: false,
                pixel_equal: false,
                changed_pixels: 0,
                total_pixels: 0,
            });
            continue;
        }
        let lt = engine.page_text(left.id(), i)?;
        let rt = engine.page_text(right.id(), i)?;
        let text_equal = normalize(&lt) == normalize(&rt);
        let lr = engine.render_page(left.id(), i, zoom)?;
        let rr = engine.render_page(right.id(), i, zoom)?;
        let (changed, total, pixel_equal) = pixel_diff(&lr.rgba, &rr.rgba, lr.width, lr.height, rr.width, rr.height);
        if !text_equal || !pixel_equal {
            identical = false;
        }
        pages.push(PageDiff {
            page: i,
            text_equal,
            pixel_equal,
            changed_pixels: changed,
            total_pixels: total,
        });
    }
    Ok(CompareReport {
        left_pages: left_n,
        right_pages: right_n,
        pages,
        identical,
    })
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pixel_diff(
    a: &[u8],
    b: &[u8],
    aw: u32,
    ah: u32,
    bw: u32,
    bh: u32,
) -> (u32, u32, bool) {
    if aw != bw || ah != bh {
        let total = aw.max(bw) * ah.max(bh);
        return (total, total, false);
    }
    let total = aw * ah;
    let mut changed = 0u32;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        if pa != pb {
            changed += 1;
        }
    }
    (changed, total, changed == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::corpus;

    fn engine() -> Option<Engine> {
        match Engine::new() {
            Ok(e) => Some(e),
            Err(err) => {
                if std::env::var("CI").is_ok() {
                    panic!("PDFium required in CI: {err}");
                }
                None
            }
        }
    }

    #[test]
    fn same_file_is_identical() {
        let Some(engine) = engine() else {
            return;
        };
        let a = engine.open(corpus("hello.pdf")).unwrap();
        let b = engine.open(corpus("hello.pdf")).unwrap();
        let report = compare_pdfs(&engine, &a, &b, 1.0).unwrap();
        assert!(report.identical, "{report:?}");
        assert_eq!(report.pages.len(), 1);
        assert!(report.pages[0].text_equal);
        assert!(report.pages[0].pixel_equal);
    }

    #[test]
    fn different_files_differ() {
        let Some(engine) = engine() else {
            return;
        };
        let a = engine.open(corpus("hello.pdf")).unwrap();
        let b = engine.open(corpus("two_page.pdf")).unwrap();
        let report = compare_pdfs(&engine, &a, &b, 1.0).unwrap();
        assert!(!report.identical);
        assert_eq!(report.left_pages, 1);
        assert_eq!(report.right_pages, 2);
    }
}
