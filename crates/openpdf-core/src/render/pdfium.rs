use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use pdfium_render::prelude::*;

use crate::error::{Error, Result};

/// Last `as_rgba_bytes` + pad/truncate, nanoseconds. No channel swizzle.
static LAST_CONVERT_NS: AtomicU64 = AtomicU64::new(0);

/// Milliseconds for the last PDFium buffer copy (`as_rgba_bytes` + `force_rgba`).
/// Reverse byte order is already on, so this is a copy — not BGRA swizzle or premultiply.
pub fn last_convert_ms() -> f64 {
    LAST_CONVERT_NS.load(Ordering::Relaxed) as f64 / 1_000_000.0
}

fn take_rgba(bitmap: &PdfBitmap, w: u32, h: u32) -> Vec<u8> {
    let t = Instant::now();
    let out = force_rgba(bitmap.as_rgba_bytes(), w, h);
    LAST_CONVERT_NS.store(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    out
}

/// Platform library file name (`libpdfium.so` / `pdfium.dll` / `libpdfium.dylib`).
pub fn pdfium_library_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

fn push_abs(out: &mut Vec<PathBuf>, path: PathBuf) {
    if let Some(p) = crate::dll::require_absolute(path) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
}

/// Absolute PDFium candidates only. No CWD, no bare library name.
pub(crate) fn pdfium_search_paths(bundled: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let lib = pdfium_library_name();

    if let Some(p) = crate::dll::env_absolute("OPENPDF_PDFIUM") {
        push_abs(&mut candidates, p);
    }
    if let Some(p) = bundled {
        push_abs(&mut candidates, p.to_path_buf());
    }
    if let Some(dir) = crate::dll::exe_dir() {
        push_abs(&mut candidates, dir.clone());
        push_abs(&mut candidates, dir.join(lib));
        #[cfg(target_os = "macos")]
        {
            push_abs(&mut candidates, dir.join("Frameworks"));
            push_abs(&mut candidates, dir.join("Frameworks").join(lib));
            if let Some(contents) = dir.parent() {
                let frameworks = contents.join("Frameworks");
                push_abs(&mut candidates, frameworks.clone());
                push_abs(&mut candidates, frameworks.join(lib));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for dir in [
            "/usr/local/lib",
            "/usr/lib",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
        ] {
            push_abs(&mut candidates, PathBuf::from(dir));
            push_abs(&mut candidates, PathBuf::from(dir).join(lib));
        }
    }

    #[cfg(target_os = "macos")]
    {
        for dir in ["/usr/local/lib", "/opt/homebrew/lib"] {
            push_abs(&mut candidates, PathBuf::from(dir));
            push_abs(&mut candidates, PathBuf::from(dir).join(lib));
        }
    }

    #[cfg(windows)]
    {
        if let Some(windir) = crate::dll::env_absolute("WINDIR") {
            push_abs(&mut candidates, windir.join("System32").join(lib));
        }
    }

    candidates
}

fn bind_path(path: &Path) -> std::result::Result<Pdfium, String> {
    crate::dll::harden();
    let path = crate::dll::require_absolute(path).ok_or_else(|| "relative path".to_string())?;
    #[cfg(windows)]
    {
        use libloading::os::windows::{
            Library, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        let flags = LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR;
        let _pre = unsafe { Library::load_with_flags(&path, flags) }.map_err(|e| e.to_string())?;
        match Pdfium::bind_to_library(&path) {
            Ok(bindings) => Ok(Pdfium::new(bindings)),
            Err(e) => Err(e.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        match Pdfium::bind_to_library(&path) {
            Ok(bindings) => Ok(Pdfium::new(bindings)),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Resolve and load PDFium from absolute paths only (no CWD / bare name).
pub fn bind_pdfium(bundled: Option<&Path>) -> Result<Pdfium> {
    let candidates = pdfium_search_paths(bundled);

    let mut last = None;
    for c in candidates {
        let lib = if c.is_dir() {
            Pdfium::pdfium_platform_library_name_at_path(&c)
        } else {
            c
        };
        match bind_path(&lib) {
            Ok(pdfium) => return Ok(pdfium),
            Err(e) => last = Some(e),
        }
    }

    Err(Error::pdfium_load(
        last.unwrap_or_else(|| "none".into()),
        "no absolute PDFium library",
    ))
}

pub fn render_tile(
    document: &PdfDocument<'_>,
    page_index: u32,
    zoom: f32,
    tile_x: u32,
    tile_y: u32,
    tile_size: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let page_w = page.width().value;
    let page_h = page.height().value;
    let (full_w, full_h) = super::page_pixel_size(page_w, page_h, zoom);
    // Reject keys outside the tile grid: tile_rect clamps to 1 px, which would
    // otherwise return a 1-px sliver of nothing instead of an error.
    let (cols, rows) = super::tile_grid(full_w, full_h, tile_size);
    if tile_size == 0 || tile_x >= cols || tile_y >= rows {
        return Err(Error::invalid(crate::error::InvalidKind::TileOutOfRange));
    }
    let (tw, th, x0, y0) = super::tile_rect(full_w, full_h, tile_x, tile_y, tile_size);

    let config = PdfRenderConfig::new()
        .scale_page_by_factor(zoom)
        .set_origin(-(x0 as i32), -(y0 as i32))
        .set_reverse_byte_order(true)
        .render_annotations(true);

    let mut bitmap = PdfBitmap::empty(tw as i32, th as i32, PdfBitmapFormat::BGRA)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    match page.render_into_bitmap_with_config(&mut bitmap, &config) {
        Ok(()) => {
            let rgba = take_rgba(&bitmap, tw, th);
            Ok((tw, th, rgba))
        }
        Err(_) => {
            // Fallback: render the full page and crop. Used if origin+small bitmap is rejected.
            let full = page
                .render_with_config(
                    &PdfRenderConfig::new()
                        .scale_page_by_factor(zoom)
                        .set_reverse_byte_order(true)
                        .render_annotations(true),
                )
                .map_err(|e| Error::pdfium(e.to_string()))?;
            let raw = take_rgba(&full, full_w, full_h);
            Ok((tw, th, crop_rgba(&raw, full_w, full_h, x0, y0, tw, th)))
        }
    }
}

pub fn render_page(
    document: &PdfDocument<'_>,
    page_index: u32,
    zoom: f32,
) -> Result<(u32, u32, Vec<u8>)> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let bitmap = page
        .render_with_config(
            &PdfRenderConfig::new()
                .scale_page_by_factor(zoom)
                .set_reverse_byte_order(true)
                .render_annotations(true),
        )
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let w = bitmap.width() as u32;
    let h = bitmap.height() as u32;
    Ok((w, h, take_rgba(&bitmap, w, h)))
}

pub fn page_text(document: &PdfDocument<'_>, page_index: u32) -> Result<String> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let text = page.text().map_err(|e| Error::pdfium(e.to_string()))?;
    Ok(text.all())
}

fn force_rgba(mut bytes: Vec<u8>, w: u32, h: u32) -> Vec<u8> {
    let expected = (w as usize) * (h as usize) * 4;
    if bytes.len() == expected {
        return bytes;
    }
    if bytes.len() > expected {
        bytes.truncate(expected);
        return bytes;
    }
    bytes.resize(expected, 255);
    bytes
}

fn crop_rgba(src: &[u8], src_w: u32, src_h: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h {
        let sy = y + row;
        if sy >= src_h {
            break;
        }
        let src_off = ((sy * src_w + x) as usize) * 4;
        let dst_off = (row as usize) * (w as usize) * 4;
        let count = ((w.min(src_w.saturating_sub(x))) as usize) * 4;
        if src_off + count <= src.len() {
            out[dst_off..dst_off + count].copy_from_slice(&src[src_off..src_off + count]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_name_matches_os() {
        let name = pdfium_library_name();
        if cfg!(windows) {
            assert_eq!(name, "pdfium.dll");
        } else if cfg!(target_os = "macos") {
            assert_eq!(name, "libpdfium.dylib");
        } else {
            assert_eq!(name, "libpdfium.so");
        }
    }

    #[test]
    fn candidates_are_absolute_never_cwd_or_bare_name() {
        let lib = pdfium_library_name();
        for p in pdfium_search_paths(None) {
            assert!(p.is_absolute(), "relative PDFium candidate: {p:?}");
            assert_ne!(p, PathBuf::from("."));
            assert_ne!(p, PathBuf::from(lib));
            assert_ne!(p.as_os_str(), ".");
        }
        for p in pdfium_search_paths(Some(Path::new(lib))) {
            assert!(p.is_absolute(), "relative bundled path leaked: {p:?}");
        }
        for p in pdfium_search_paths(Some(Path::new("."))) {
            assert!(p.is_absolute(), "CWD leaked: {p:?}");
        }
    }
}
